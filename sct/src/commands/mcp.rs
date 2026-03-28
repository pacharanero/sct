//! `sct mcp` — Local MCP server over stdio backed by a SNOMED CT SQLite database.
//!
//! Transport: JSON-RPC 2.0 with Content-Length framing (same as LSP / MCP stdio spec).
//! Protocol version: 2024-11-05
//!
//! Tools exposed:
//!   snomed_search        — FTS5 free-text search
//!   snomed_concept       — Full concept detail by SCTID
//!   snomed_children      — Immediate children of a concept
//!   snomed_ancestors     — Full ancestor chain to root
//!   snomed_hierarchy     — All concepts in a named top-level hierarchy
//!
//! Claude Desktop config:
//!   {
//!     "mcpServers": {
//!       "snomed": {
//!         "command": "sct",
//!         "args": ["mcp", "--db", "/path/to/snomed.db"]
//!       }
//!     }
//!   }

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};

use crate::schema::SCHEMA_VERSION;

#[derive(Parser, Debug)]
pub struct Args {
    /// Path to the SNOMED CT SQLite database produced by `sct sqlite`.
    #[arg(long)]
    pub db: std::path::PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let conn = Connection::open(&args.db)
        .with_context(|| format!("opening database {}", args.db.display()))?;
    conn.execute_batch(
        "PRAGMA query_only = ON;
         PRAGMA cache_size = -32768;",
    )?;

    // Validate the database schema_version before serving.
    validate_schema_version(&conn)?;

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    loop {
        match read_message(&mut reader) {
            Ok(Some(raw)) => {
                if let Ok(msg) = serde_json::from_str::<Value>(&raw) {
                    if let Some(response) = handle_message(&conn, &msg) {
                        let text = serde_json::to_string(&response)?;
                        write_message(&mut writer, &text)?;
                    }
                }
            }
            Ok(None) => break, // EOF
            Err(_) => break,
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Schema version validation
// ---------------------------------------------------------------------------

/// How many schema versions ahead we will tolerate before refusing to start.
///
/// * db_version == SCHEMA_VERSION  → OK, no warning
/// * db_version in (SCHEMA_VERSION, SCHEMA_VERSION + WARN_THRESHOLD]  → warn to stderr, continue
/// * db_version > SCHEMA_VERSION + WARN_THRESHOLD  → hard error, refuse to start
const SCHEMA_WARN_THRESHOLD: u32 = 5;

fn validate_schema_version(conn: &Connection) -> Result<()> {
    // The schema_version column is stored per-concept; take the max.
    let db_version: Option<u32> = conn
        .query_row("SELECT MAX(schema_version) FROM concepts", [], |row| {
            row.get(0)
        })
        .unwrap_or(None);

    let db_version = match db_version {
        Some(v) => v,
        None => {
            // Empty database — nothing to serve but not an error.
            return Ok(());
        }
    };

    if db_version == SCHEMA_VERSION {
        return Ok(());
    }

    if db_version < SCHEMA_VERSION {
        // Older database: we can likely still read it.
        eprintln!(
            "sct mcp: database schema_version {} is older than this binary expects ({}).\n\
             Consider regenerating with `sct ndjson` + `sct sqlite`.",
            db_version, SCHEMA_VERSION
        );
        return Ok(());
    }

    // db_version > SCHEMA_VERSION
    let gap = db_version - SCHEMA_VERSION;
    if gap <= SCHEMA_WARN_THRESHOLD {
        eprintln!(
            "sct mcp: WARNING — database schema_version {} is newer than this binary ({}).\n\
             Some fields may not be served correctly. Upgrade sct to remove this warning.",
            db_version, SCHEMA_VERSION
        );
        Ok(())
    } else {
        anyhow::bail!(
            "database schema_version {} is too new for this binary (expects {}).\n\
             Please upgrade sct: https://github.com/your-org/sct/releases",
            db_version,
            SCHEMA_VERSION
        )
    }
}

// ---------------------------------------------------------------------------
// Transport: dual-mode stdio (MCP spec changed in 2025)
//
// MCP 2024-11-05 used Content-Length framing (LSP-style).
// MCP 2025-03-26+ uses plain newline-delimited JSON (one object per line).
//
// We detect the format from the first byte of each message and handle both,
// so we work with Claude Desktop (old) and Claude Code 2.1.86+ (new).
// Responses are always written as newline-delimited JSON (current spec).
// ---------------------------------------------------------------------------

fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<String>> {
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            continue; // skip blank lines between messages
        }

        // New spec (≥ 2025-03-26): bare JSON object on a single line.
        if trimmed.starts_with('{') {
            return Ok(Some(trimmed.to_owned()));
        }

        // Old spec (2024-11-05): Content-Length framing, like LSP.
        if let Some(rest) = trimmed.strip_prefix("Content-Length: ") {
            let len: usize = rest.trim().parse().unwrap_or(0);
            // Consume remaining headers until blank line.
            loop {
                let mut hdr = String::new();
                let hn = reader.read_line(&mut hdr)?;
                if hn == 0 || hdr.trim_end_matches(['\r', '\n']).is_empty() {
                    break;
                }
            }
            if len == 0 {
                return Ok(None);
            }
            let mut buf = vec![0u8; len];
            reader
                .read_exact(&mut buf)
                .context("reading message body")?;
            return Ok(Some(
                String::from_utf8(buf).context("message is not UTF-8")?,
            ));
        }

        // Unrecognised line — skip it.
    }
}

fn write_message<W: Write>(writer: &mut W, msg: &str) -> Result<()> {
    // Always write newline-delimited JSON (current MCP spec).
    // JSON-RPC objects must not contain embedded newlines — serde_json compact
    // output never does, so this is safe.
    writeln!(writer, "{}", msg)?;
    writer.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 message handling
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Request {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct Response {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

impl Response {
    fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }
    fn err(id: Value, code: i64, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(json!({"code": code, "message": message})),
        }
    }
}

fn handle_message(conn: &Connection, msg: &Value) -> Option<Value> {
    let req: Request = serde_json::from_value(msg.clone()).ok()?;

    if req.jsonrpc != "2.0" {
        return None;
    }

    // Notifications have no id — process but don't respond
    let id = match &req.id {
        Some(id) => id.clone(),
        None => {
            // Handle notifications
            return None;
        }
    };

    let result = match req.method.as_str() {
        "initialize" => handle_initialize(&req.params),
        "tools/list" => handle_tools_list(),
        "tools/call" => match handle_tools_call(conn, &req.params) {
            Ok(v) => v,
            Err(e) => {
                return Some(
                    serde_json::to_value(Response::err(id, -32603, &e.to_string())).unwrap(),
                );
            }
        },
        "ping" => json!({}),
        _ => {
            return Some(
                serde_json::to_value(Response::err(id, -32601, "Method not found")).unwrap(),
            );
        }
    };

    Some(serde_json::to_value(Response::ok(id, result)).unwrap())
}

fn handle_initialize(params: &Option<Value>) -> Value {
    // Echo back the client's requested protocol version so that newer clients
    // (e.g. Claude Code ≥ 2.x using 2025-03-26) don't reject us.  We support
    // any version ≥ "2024-11-05"; fall back to that minimum if none is given.
    const MIN_VERSION: &str = "2024-11-05";
    let protocol_version = params
        .as_ref()
        .and_then(|p| p.get("protocolVersion"))
        .and_then(|v| v.as_str())
        .filter(|v| v.as_bytes() >= MIN_VERSION.as_bytes())
        .unwrap_or(MIN_VERSION);

    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "sct-mcp",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn handle_tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "snomed_search",
                "description": "Free-text search over SNOMED CT concepts using FTS5. Returns id, preferred_term, fsn, and hierarchy.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search terms (words or phrases)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default 10, max 100)"
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "snomed_concept",
                "description": "Retrieve full detail for a single SNOMED CT concept by SCTID.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "SNOMED CT concept identifier (SCTID)"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "snomed_children",
                "description": "List the immediate IS-A children of a SNOMED CT concept.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "SNOMED CT concept identifier (SCTID)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of children to return (default 50)"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "snomed_ancestors",
                "description": "Return the full ancestor chain from a concept to the SNOMED CT root.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "SNOMED CT concept identifier (SCTID)"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "snomed_hierarchy",
                "description": "List concepts in a named top-level SNOMED CT hierarchy (e.g. 'Clinical finding', 'Procedure').",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "hierarchy": {
                            "type": "string",
                            "description": "Top-level hierarchy name (e.g. 'Clinical finding', 'Procedure', 'Substance')"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default 100)"
                        }
                    },
                    "required": ["hierarchy"]
                }
            }
        ]
    })
}

fn handle_tools_call(conn: &Connection, params: &Option<Value>) -> Result<Value> {
    let params = params.as_ref().context("tools/call requires params")?;
    let name = params["name"]
        .as_str()
        .context("tools/call requires name")?;
    let args = &params["arguments"];

    let text = match name {
        "snomed_search" => tool_search(conn, args)?,
        "snomed_concept" => tool_concept(conn, args)?,
        "snomed_children" => tool_children(conn, args)?,
        "snomed_ancestors" => tool_ancestors(conn, args)?,
        "snomed_hierarchy" => tool_hierarchy(conn, args)?,
        _ => anyhow::bail!("Unknown tool: {}", name),
    };

    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "isError": false
    }))
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

fn tool_search(conn: &Connection, args: &Value) -> Result<String> {
    let query = args["query"]
        .as_str()
        .context("snomed_search requires query")?;
    let limit = args["limit"].as_u64().unwrap_or(10).min(100) as usize;

    // Sanitise query: FTS5 doesn't like unmatched quotes or reserved words
    let safe_query = sanitise_fts_query(query);

    let mut stmt = conn.prepare(
        "SELECT f.id, f.preferred_term, f.fsn, c.hierarchy
         FROM concepts_fts f
         JOIN concepts c ON c.id = f.id
         WHERE concepts_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2",
    )?;

    let rows: Vec<Value> = stmt
        .query_map(params![safe_query, limit as i64], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "preferred_term": row.get::<_, String>(1)?,
                "fsn": row.get::<_, String>(2)?,
                "hierarchy": row.get::<_, String>(3)?
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        return Ok(format!("No results found for query: {}", query));
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

fn tool_concept(conn: &Connection, args: &Value) -> Result<String> {
    let id = args["id"].as_str().context("snomed_concept requires id")?;

    let result = conn.query_row(
        "SELECT id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
                parents, children_count, attributes, active, module, effective_time
         FROM concepts WHERE id = ?1",
        params![id],
        |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "fsn": row.get::<_, String>(1)?,
                "preferred_term": row.get::<_, String>(2)?,
                "synonyms": serde_json::from_str::<Value>(&row.get::<_, String>(3).unwrap_or_default()).unwrap_or(Value::Null),
                "hierarchy": row.get::<_, String>(4)?,
                "hierarchy_path": serde_json::from_str::<Value>(&row.get::<_, String>(5).unwrap_or_default()).unwrap_or(Value::Null),
                "parents": serde_json::from_str::<Value>(&row.get::<_, String>(6).unwrap_or_default()).unwrap_or(Value::Null),
                "children_count": row.get::<_, i64>(7)?,
                "attributes": serde_json::from_str::<Value>(&row.get::<_, String>(8).unwrap_or_default()).unwrap_or(Value::Null),
                "active": row.get::<_, bool>(9)?,
                "module": row.get::<_, String>(10)?,
                "effective_time": row.get::<_, String>(11)?
            }))
        },
    );

    match result {
        Ok(v) => Ok(serde_json::to_string_pretty(&v)?),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(format!("Concept {} not found", id)),
        Err(e) => Err(e.into()),
    }
}

fn tool_children(conn: &Connection, args: &Value) -> Result<String> {
    let id = args["id"].as_str().context("snomed_children requires id")?;
    let limit = args["limit"].as_u64().unwrap_or(50).min(500) as usize;

    let mut stmt = conn.prepare(
        "SELECT c.id, c.preferred_term, c.fsn
         FROM concepts c
         JOIN concept_isa ci ON ci.child_id = c.id
         WHERE ci.parent_id = ?1
         ORDER BY c.preferred_term
         LIMIT ?2",
    )?;

    let rows: Vec<Value> = stmt
        .query_map(params![id, limit as i64], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "preferred_term": row.get::<_, String>(1)?,
                "fsn": row.get::<_, String>(2)?
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        return Ok(format!("No children found for concept {}", id));
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

fn tool_ancestors(conn: &Connection, args: &Value) -> Result<String> {
    let id = args["id"]
        .as_str()
        .context("snomed_ancestors requires id")?;

    // Recursive CTE walking up the IS-A graph from the given concept to root.
    // depth is used to order from root down to the immediate parent.
    let mut stmt = conn.prepare(
        "WITH RECURSIVE anc(id, depth) AS (
             SELECT parent_id, 1 FROM concept_isa WHERE child_id = ?1
             UNION ALL
             SELECT ci.parent_id, a.depth + 1
             FROM concept_isa ci
             JOIN anc a ON a.id = ci.child_id
             WHERE a.depth < 25
         )
         SELECT DISTINCT c.id, c.preferred_term, c.fsn, MAX(a.depth) AS depth
         FROM anc a
         JOIN concepts c ON c.id = a.id
         GROUP BY c.id
         ORDER BY depth DESC",
    )?;

    let rows: Vec<Value> = stmt
        .query_map(params![id], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "preferred_term": row.get::<_, String>(1)?,
                "fsn": row.get::<_, String>(2)?
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        return Ok(format!("No ancestors found for concept {}", id));
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

fn tool_hierarchy(conn: &Connection, args: &Value) -> Result<String> {
    let hierarchy = args["hierarchy"]
        .as_str()
        .context("snomed_hierarchy requires hierarchy")?;
    let limit = args["limit"].as_u64().unwrap_or(100).min(1000) as usize;

    let mut stmt = conn.prepare(
        "SELECT id, preferred_term, fsn
         FROM concepts
         WHERE hierarchy = ?1
         ORDER BY preferred_term
         LIMIT ?2",
    )?;

    let rows: Vec<Value> = stmt
        .query_map(params![hierarchy, limit as i64], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "preferred_term": row.get::<_, String>(1)?,
                "fsn": row.get::<_, String>(2)?
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        return Ok(format!("No concepts found in hierarchy: {}", hierarchy));
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

// ---------------------------------------------------------------------------
// FTS5 query sanitisation
// ---------------------------------------------------------------------------

/// Make a user query safe for FTS5 MATCH.
/// Wraps multi-word queries in double quotes to treat them as phrases,
/// and escapes any existing double quotes.
fn sanitise_fts_query(q: &str) -> String {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // If it looks like the caller already wrote an FTS5 expression, pass through
    // simple single-word queries without quoting; wrap everything else.
    if trimmed.split_whitespace().count() == 1 && !trimmed.contains('"') {
        return trimmed.to_string();
    }
    // Escape internal double quotes and wrap in outer quotes for phrase match
    format!("\"{}\"", trimmed.replace('"', "\"\""))
}
