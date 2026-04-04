//! `sct mcp` — Local MCP server over stdio backed by a SNOMED CT SQLite database.
//!
//! Transport: JSON-RPC 2.0 with Content-Length framing (same as LSP / MCP stdio spec).
//! Protocol version: 2024-11-05
//!
//! Tools exposed:
//!   snomed_search          — FTS5 free-text search
//!   snomed_concept         — Full concept detail by SCTID
//!   snomed_children        — Immediate children of a concept
//!   snomed_ancestors       — Full ancestor chain to root
//!   snomed_hierarchy       — All concepts in a named top-level hierarchy
//!   snomed_semantic_search — Nearest-neighbour semantic search (optional; requires --embeddings)
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
use std::path::PathBuf;

use crate::commands::codelist::{
    export_csv, export_markdown, export_opencodelists_csv, lookup_concept_row,
    lookup_hierarchy_and_children, lookup_preferred_term, read_codelist, today, write_codelist,
    CodelistFile, ConceptLine, FrontMatter, Warning,
};
use crate::commands::semantic;
use crate::schema::SCHEMA_VERSION;

#[derive(Parser, Debug)]
pub struct Args {
    /// Path to the SNOMED CT SQLite database produced by `sct sqlite`.
    #[arg(long)]
    pub db: PathBuf,

    /// Arrow IPC embeddings file produced by `sct embed`.
    /// When supplied, the `snomed_semantic_search` tool is registered.
    #[arg(long)]
    pub embeddings: Option<PathBuf>,

    /// Ollama embedding model (used by `snomed_semantic_search`).
    #[arg(long, default_value = "nomic-embed-text")]
    pub model: String,

    /// Ollama API base URL (used by `snomed_semantic_search`).
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,
}

/// Configuration for the optional semantic search tool.
struct SemanticConfig {
    embeddings: PathBuf,
    model: String,
    ollama_url: String,
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

    let semantic_cfg = args.embeddings.map(|embeddings| SemanticConfig {
        embeddings,
        model: args.model,
        ollama_url: args.ollama_url,
    });

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    loop {
        match read_message(&mut reader) {
            Ok(Some(raw)) => {
                if let Ok(msg) = serde_json::from_str::<Value>(&raw) {
                    if let Some(response) = handle_message(&conn, &msg, semantic_cfg.as_ref()) {
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

fn handle_message(
    conn: &Connection,
    msg: &Value,
    semantic_cfg: Option<&SemanticConfig>,
) -> Option<Value> {
    let req: Request = serde_json::from_value(msg.clone()).ok()?;

    if req.jsonrpc != "2.0" {
        return None;
    }

    // Notifications have no id — process but don't respond
    let id = match &req.id {
        Some(id) => id.clone(),
        None => return None,
    };

    let result = match req.method.as_str() {
        "initialize" => handle_initialize(&req.params),
        "tools/list" => handle_tools_list(semantic_cfg),
        "tools/call" => match handle_tools_call(conn, &req.params, semantic_cfg) {
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

fn handle_tools_list(semantic_cfg: Option<&SemanticConfig>) -> Value {
    let mut tools = vec![
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
            "name": "snomed_map",
            "description": "Cross-map between SNOMED CT and legacy UK terminologies (CTV3 / Read v2). \
                            Given a SNOMED CT SCTID, returns all mapped CTV3 and Read v2 codes. \
                            Given a CTV3 or Read v2 code, returns the mapped SNOMED CT concept(s). \
                            Use the 'from' field to specify the input code and 'terminology' to \
                            select which system it belongs to ('snomed', 'ctv3', or 'read2'). \
                            Only available when the database was built from a UK Monolith RF2 release.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "The code to look up (SCTID, CTV3 code, or Read v2 code)"
                    },
                    "terminology": {
                        "type": "string",
                        "enum": ["snomed", "ctv3", "read2"],
                        "description": "Which terminology the input code belongs to"
                    }
                },
                "required": ["code", "terminology"]
            }
        }),
    ];

    if semantic_cfg.is_some() {
        tools.push(json!({
            "name": "snomed_semantic_search",
            "description": "Semantic nearest-neighbour search over SNOMED CT concepts using vector embeddings. \
                            Finds conceptually similar concepts even when exact terms don't match — useful for \
                            natural-language queries, typos, and synonym gaps. Requires Ollama running locally.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural-language search query"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default 10)"
                    }
                },
                "required": ["query"]
            }
        }));
    }

    // Codelist tools — always registered
    tools.push(json!({
        "name": "codelist_list",
        "description": "List .codelist files in a directory. Returns file paths with title, status, and concept count from each file's front-matter.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "Directory to search for .codelist files (default: current directory)"
                }
            }
        }
    }));
    tools.push(json!({
        "name": "codelist_read",
        "description": "Read a .codelist file and return its metadata and concept lists (active, excluded, pending review).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the .codelist file" }
            },
            "required": ["file"]
        }
    }));
    tools.push(json!({
        "name": "codelist_new",
        "description": "Scaffold a new .codelist file with YAML front-matter template.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file":        { "type": "string", "description": "Path for the new .codelist file" },
                "title":       { "type": "string", "description": "Human-readable title" },
                "description": { "type": "string", "description": "What this codelist is for" },
                "terminology": { "type": "string", "description": "Terminology (default: SNOMED CT)" },
                "author":      { "type": "string", "description": "Author name" }
            },
            "required": ["file", "title"]
        }
    }));
    tools.push(json!({
        "name": "codelist_add",
        "description": "Add one or more SNOMED CT concepts to a .codelist file. Resolves preferred terms from the database. Deduplicates silently.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file":    { "type": "string", "description": "Path to the .codelist file" },
                "sctids":  { "type": "array", "items": { "type": "string" }, "description": "SCTIDs to add" },
                "comment": { "type": "string", "description": "Optional inline annotation for added lines" }
            },
            "required": ["file", "sctids"]
        }
    }));
    tools.push(json!({
        "name": "codelist_remove",
        "description": "Move a concept from active to explicitly excluded in a .codelist file, preserving the audit trail.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file":    { "type": "string", "description": "Path to the .codelist file" },
                "sctid":   { "type": "string", "description": "SCTID to exclude" },
                "comment": { "type": "string", "description": "Reason for exclusion (appended as inline comment)" }
            },
            "required": ["file", "sctid"]
        }
    }));
    tools.push(json!({
        "name": "codelist_validate",
        "description": "Validate a .codelist file against the SNOMED CT database. Returns warnings and errors: inactive concepts, term drift, pending review items, missing required fields.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the .codelist file" }
            },
            "required": ["file"]
        }
    }));
    tools.push(json!({
        "name": "codelist_stats",
        "description": "Return statistics for a .codelist file: concept count, hierarchy breakdown, leaf/intermediate ratio, excluded count, SNOMED release age.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path to the .codelist file" }
            },
            "required": ["file"]
        }
    }));
    tools.push(json!({
        "name": "codelist_export",
        "description": "Export a .codelist file as a string in the requested format.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file":   { "type": "string", "description": "Path to the .codelist file" },
                "format": { "type": "string", "enum": ["csv", "opencodelists-csv", "markdown"], "description": "Export format (default: csv)" }
            },
            "required": ["file"]
        }
    }));

    json!({ "tools": tools })
}

fn handle_tools_call(
    conn: &Connection,
    params: &Option<Value>,
    semantic_cfg: Option<&SemanticConfig>,
) -> Result<Value> {
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
        "snomed_map" => tool_map(conn, args)?,
        "snomed_semantic_search" => tool_semantic_search(args, semantic_cfg)?,
        "codelist_list" => tool_codelist_list(args)?,
        "codelist_read" => tool_codelist_read(args)?,
        "codelist_new" => tool_codelist_new(args)?,
        "codelist_add" => tool_codelist_add(conn, args)?,
        "codelist_remove" => tool_codelist_remove(args)?,
        "codelist_validate" => tool_codelist_validate(conn, args)?,
        "codelist_stats" => tool_codelist_stats(conn, args)?,
        "codelist_export" => tool_codelist_export(args)?,
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
                parents, children_count, attributes, active, module, effective_time,
                ctv3_codes, read2_codes
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
                "effective_time": row.get::<_, String>(11)?,
                "ctv3_codes": serde_json::from_str::<Value>(&row.get::<_, String>(12).unwrap_or_default()).unwrap_or(json!([])),
                "read2_codes": serde_json::from_str::<Value>(&row.get::<_, String>(13).unwrap_or_default()).unwrap_or(json!([]))
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
        "SELECT DISTINCT c.id, c.preferred_term, c.fsn
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
        "WITH RECURSIVE anc AS (
             SELECT DISTINCT parent_id AS id FROM concept_isa WHERE child_id = ?1
             UNION
             SELECT ci.parent_id FROM concept_isa ci
             JOIN anc a ON ci.child_id = a.id
         )
         SELECT c.id, c.preferred_term, c.fsn,
                json_array_length(c.hierarchy_path) AS depth
         FROM anc a
         JOIN concepts c ON c.id = a.id
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

fn tool_map(conn: &Connection, args: &Value) -> Result<String> {
    let code = args["code"].as_str().context("snomed_map requires code")?;
    let terminology = args["terminology"]
        .as_str()
        .context("snomed_map requires terminology")?;

    match terminology {
        "snomed" => {
            // SNOMED SCTID → CTV3 and Read v2 codes
            let mut ctv3_stmt = conn.prepare(
                "SELECT code FROM concept_maps WHERE concept_id = ?1 AND terminology = 'ctv3' ORDER BY code",
            )?;
            let ctv3_codes: Vec<String> = ctv3_stmt
                .query_map(params![code], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            let mut read2_stmt = conn.prepare(
                "SELECT code FROM concept_maps WHERE concept_id = ?1 AND terminology = 'read2' ORDER BY code",
            )?;
            let read2_codes: Vec<String> = read2_stmt
                .query_map(params![code], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            if ctv3_codes.is_empty() && read2_codes.is_empty() {
                return Ok(format!(
                    "No CTV3 or Read v2 mappings found for SNOMED CT concept {}. \
                     Mappings are only present when the database was built from a UK Monolith RF2 release.",
                    code
                ));
            }

            Ok(serde_json::to_string_pretty(&json!({
                "snomed_id": code,
                "ctv3_codes": ctv3_codes,
                "read2_codes": read2_codes
            }))?)
        }

        "ctv3" | "read2" => {
            // CTV3 or Read v2 code → SNOMED CT concept(s)
            let mut stmt = conn.prepare(
                "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy
                 FROM concept_maps m
                 JOIN concepts c ON c.id = m.concept_id
                 WHERE m.code = ?1 AND m.terminology = ?2
                 ORDER BY c.id",
            )?;

            let rows: Vec<Value> = stmt
                .query_map(params![code, terminology], |row| {
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
                return Ok(format!(
                    "No SNOMED CT mapping found for {} code '{}'. \
                     Mappings are only present when the database was built from a UK Monolith RF2 release.",
                    terminology.to_uppercase(),
                    code
                ));
            }

            Ok(serde_json::to_string_pretty(&json!({
                "code": code,
                "terminology": terminology,
                "snomed_concepts": rows
            }))?)
        }

        other => anyhow::bail!(
            "Unknown terminology '{}'. Use 'snomed', 'ctv3', or 'read2'.",
            other
        ),
    }
}

// ---------------------------------------------------------------------------
// Codelist tool implementations
// ---------------------------------------------------------------------------

fn cl_path(args: &Value) -> Result<std::path::PathBuf> {
    let s = args["file"].as_str().context("requires file")?;
    Ok(std::path::PathBuf::from(s))
}

fn tool_codelist_list(args: &Value) -> Result<String> {
    let dir = args["directory"].as_str().unwrap_or(".");
    let base = std::path::Path::new(dir);
    anyhow::ensure!(base.is_dir(), "directory not found: {}", dir);

    let mut entries: Vec<Value> = Vec::new();
    for entry in walkdir::WalkDir::new(base)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("codelist"))
    {
        let path = entry.path();
        let item = match read_codelist(path) {
            Ok(cl) => {
                let active = cl
                    .body
                    .iter()
                    .filter(|l| matches!(l, ConceptLine::Active { .. }))
                    .count();
                json!({
                    "file": path.to_string_lossy(),
                    "id": cl.front_matter.id,
                    "title": cl.front_matter.title,
                    "status": cl.front_matter.status,
                    "version": cl.front_matter.version,
                    "active_concepts": active,
                    "updated": cl.front_matter.updated,
                })
            }
            Err(e) => json!({ "file": path.to_string_lossy(), "error": e.to_string() }),
        };
        entries.push(item);
    }

    if entries.is_empty() {
        return Ok(format!("No .codelist files found in {}", dir));
    }
    Ok(serde_json::to_string_pretty(&entries)?)
}

fn tool_codelist_read(args: &Value) -> Result<String> {
    let path = cl_path(args)?;
    let cl = read_codelist(&path)?;
    let fm = &cl.front_matter;

    let active: Vec<Value> = cl
        .body
        .iter()
        .filter_map(|l| {
            if let ConceptLine::Active { id, term, comment } = l {
                Some(json!({ "id": id, "term": term, "comment": comment }))
            } else {
                None
            }
        })
        .collect();

    let excluded: Vec<Value> = cl
        .body
        .iter()
        .filter_map(|l| {
            if let ConceptLine::Excluded { id, term, comment } = l {
                Some(json!({ "id": id, "term": term, "comment": comment }))
            } else {
                None
            }
        })
        .collect();

    let pending: Vec<Value> = cl
        .body
        .iter()
        .filter_map(|l| {
            if let ConceptLine::PendingReview { id, term } = l {
                Some(json!({ "id": id, "term": term }))
            } else {
                None
            }
        })
        .collect();

    Ok(serde_json::to_string_pretty(&json!({
        "file": path.to_string_lossy(),
        "id": fm.id,
        "title": fm.title,
        "description": fm.description,
        "terminology": fm.terminology,
        "status": fm.status,
        "version": fm.version,
        "updated": fm.updated,
        "snomed_release": fm.snomed_release,
        "active_concepts": active,
        "excluded_concepts": excluded,
        "pending_review": pending,
    }))?)
}

fn tool_codelist_new(args: &Value) -> Result<String> {
    let path = cl_path(args)?;
    if path.exists() {
        anyhow::bail!("{} already exists", path.display());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }

    let title = args["title"]
        .as_str()
        .context("codelist_new requires title")?
        .to_string();
    let terminology = args["terminology"]
        .as_str()
        .unwrap_or("SNOMED CT")
        .to_string();
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_lowercase()
        .replace(' ', "-");
    let today = today();

    let fm = FrontMatter {
        id,
        description: args["description"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| format!("{title} codes")),
        title,
        terminology: terminology.clone(),
        created: today.clone(),
        updated: today,
        version: 1,
        status: "draft".to_string(),
        licence: "CC-BY-4.0".to_string(),
        copyright: "Copyright holder. SNOMED CT content © IHTSDO.".to_string(),
        appropriate_use: "Describe appropriate use here.".to_string(),
        misuse: "Describe misuse here.".to_string(),
        snomed_release: None,
        authors: args["author"].as_str().map(|name| {
            vec![crate::commands::codelist::Author {
                name: name.to_string(),
                orcid: None,
                affiliation: None,
                role: Some("author".to_string()),
            }]
        }),
        organisation: None,
        methodology: None,
        signoffs: None,
        warnings: Some(vec![
            Warning {
                code: "not-universal-definition".to_string(),
                severity: "info".to_string(),
                message: "Developed for a specific purpose — may not suit all uses.".to_string(),
            },
            Warning {
                code: "draft-not-reviewed".to_string(),
                severity: "info".to_string(),
                message: "Not yet reviewed. Check status before use.".to_string(),
            },
        ]),
        population: None,
        care_setting: None,
        tags: None,
        opencodelists_id: None,
        opencodelists_url: None,
    };

    let cl = CodelistFile {
        front_matter: fm,
        body: vec![
            ConceptLine::Blank,
            ConceptLine::Comment("# concepts".to_string()),
            ConceptLine::Blank,
        ],
    };
    write_codelist(&cl, &path)?;
    Ok(format!("Created {}", path.display()))
}

fn tool_codelist_add(conn: &Connection, args: &Value) -> Result<String> {
    let path = cl_path(args)?;
    let sctids: Vec<String> = args["sctids"]
        .as_array()
        .context("codelist_add requires sctids array")?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if sctids.is_empty() {
        anyhow::bail!("sctids array is empty");
    }
    let comment = args["comment"].as_str().map(String::from);

    let mut cl = read_codelist(&path)?;
    let existing: std::collections::HashSet<String> = cl
        .body
        .iter()
        .filter_map(|l| {
            if matches!(l, ConceptLine::Active { .. }) {
                l.sctid().map(String::from)
            } else {
                None
            }
        })
        .collect();

    let mut added = 0usize;
    let mut not_found: Vec<String> = Vec::new();
    for id in &sctids {
        if existing.contains(id) {
            continue;
        }
        match lookup_preferred_term(conn, id) {
            Ok(term) => {
                cl.body.push(ConceptLine::Active {
                    id: id.clone(),
                    term,
                    comment: comment.clone(),
                });
                added += 1;
            }
            Err(_) => not_found.push(id.clone()),
        }
    }

    if added > 0 {
        cl.front_matter.updated = today();
        cl.front_matter.version += 1;
        write_codelist(&cl, &path)?;
    }

    let mut result = json!({ "added": added, "file": path.to_string_lossy() });
    if !not_found.is_empty() {
        result["not_found"] = json!(not_found);
    }
    Ok(serde_json::to_string_pretty(&result)?)
}

fn tool_codelist_remove(args: &Value) -> Result<String> {
    let path = cl_path(args)?;
    let sctid = args["sctid"]
        .as_str()
        .context("codelist_remove requires sctid")?;
    let comment = args["comment"].as_str().map(String::from);

    let mut cl = read_codelist(&path)?;
    let mut found = false;
    for line in &mut cl.body {
        if let ConceptLine::Active { id, term, .. } = line {
            if id == sctid {
                *line = ConceptLine::Excluded {
                    id: id.clone(),
                    term: term.clone(),
                    comment,
                };
                found = true;
                break;
            }
        }
    }
    if !found {
        anyhow::bail!(
            "SCTID {} not found as an active concept in {}",
            sctid,
            path.display()
        );
    }
    cl.front_matter.updated = today();
    cl.front_matter.version += 1;
    write_codelist(&cl, &path)?;
    Ok(format!("Moved {} to excluded in {}", sctid, path.display()))
}

fn tool_codelist_validate(conn: &Connection, args: &Value) -> Result<String> {
    let path = cl_path(args)?;
    let cl = read_codelist(&path)?;
    let fm = &cl.front_matter;

    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for (field, val) in [
        ("appropriate_use", fm.appropriate_use.as_str()),
        ("misuse", fm.misuse.as_str()),
    ] {
        if val.trim().is_empty() || val.starts_with("Describe") {
            if fm.status == "published" {
                errors.push(format!(
                    "`{field}` must be filled in for published codelists"
                ));
            } else {
                warnings.push(format!("`{field}` is a placeholder"));
            }
        }
    }
    if fm.status == "published" && fm.signoffs.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
        errors.push("published codelist requires at least one signoff".to_string());
    }

    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for line in &cl.body {
        if let Some(id) = line.sctid() {
            *seen.entry(id).or_insert(0) += 1;
        }
    }
    for (id, count) in &seen {
        if *count > 1 {
            errors.push(format!("SCTID {id} appears {count} times"));
        }
    }

    for line in &cl.body {
        match line {
            ConceptLine::Active { id, term, .. } => match lookup_concept_row(conn, id)? {
                None => errors.push(format!("{id}: not found in database")),
                Some((db_term, false)) => {
                    errors.push(format!("{id}: inactive in database ({db_term})"))
                }
                Some((db_term, true)) if db_term != *term => warnings.push(format!(
                    "{id}: stored term {term:?} differs from database {db_term:?}"
                )),
                _ => {}
            },
            ConceptLine::PendingReview { id, term } => {
                warnings.push(format!("{id} ({term}): pending review"))
            }
            _ => {}
        }
    }

    let active_count = cl
        .body
        .iter()
        .filter(|l| matches!(l, ConceptLine::Active { .. }))
        .count();
    Ok(serde_json::to_string_pretty(&json!({
        "file": path.to_string_lossy(),
        "active_concepts": active_count,
        "warnings": warnings,
        "errors": errors,
        "valid": errors.is_empty(),
    }))?)
}

fn tool_codelist_stats(conn: &Connection, args: &Value) -> Result<String> {
    let path = cl_path(args)?;
    let cl = read_codelist(&path)?;
    let fm = &cl.front_matter;

    let active: Vec<&str> = cl
        .body
        .iter()
        .filter_map(|l| {
            if matches!(l, ConceptLine::Active { .. }) {
                l.sctid()
            } else {
                None
            }
        })
        .collect();
    let excluded_count = cl
        .body
        .iter()
        .filter(|l| matches!(l, ConceptLine::Excluded { .. }))
        .count();
    let pending_count = cl
        .body
        .iter()
        .filter(|l| matches!(l, ConceptLine::PendingReview { .. }))
        .count();

    let mut by_hierarchy: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut leaf_count = 0usize;
    let mut intermediate_count = 0usize;
    for id in &active {
        if let Some((hierarchy, children_count)) = lookup_hierarchy_and_children(conn, id)? {
            *by_hierarchy.entry(hierarchy).or_insert(0) += 1;
            if children_count == 0 {
                leaf_count += 1;
            } else {
                intermediate_count += 1;
            }
        }
    }

    let mut hierarchy_list: Vec<Value> = by_hierarchy
        .into_iter()
        .map(|(h, n)| json!({"hierarchy": h, "count": n}))
        .collect();
    hierarchy_list.sort_by(|a, b| b["count"].as_u64().cmp(&a["count"].as_u64()));

    Ok(serde_json::to_string_pretty(&json!({
        "file": path.to_string_lossy(),
        "title": fm.title,
        "terminology": fm.terminology,
        "status": fm.status,
        "version": fm.version,
        "updated": fm.updated,
        "snomed_release": fm.snomed_release,
        "active_concepts": active.len(),
        "excluded_concepts": excluded_count,
        "pending_review": pending_count,
        "by_hierarchy": hierarchy_list,
        "leaf_nodes": leaf_count,
        "intermediate_nodes": intermediate_count,
    }))?)
}

fn tool_codelist_export(args: &Value) -> Result<String> {
    let path = cl_path(args)?;
    let cl = read_codelist(&path)?;
    let active: Vec<(&str, &str)> = cl
        .body
        .iter()
        .filter_map(|l| {
            if let ConceptLine::Active { id, term, .. } = l {
                Some((id.as_str(), term.as_str()))
            } else {
                None
            }
        })
        .collect();

    match args["format"].as_str().unwrap_or("csv") {
        "csv" => Ok(export_csv(&active)),
        "opencodelists-csv" => Ok(export_opencodelists_csv(&active)),
        "markdown" => Ok(export_markdown(&cl.front_matter, &active)),
        other => {
            anyhow::bail!("unsupported format: {other}. Use csv, opencodelists-csv, or markdown")
        }
    }
}

fn tool_semantic_search(args: &Value, semantic_cfg: Option<&SemanticConfig>) -> Result<String> {
    let cfg = semantic_cfg.context(
        "snomed_semantic_search is not available: start sct mcp with --embeddings <file>",
    )?;
    let query = args["query"]
        .as_str()
        .context("snomed_semantic_search requires query")?;
    let limit = args["limit"].as_u64().unwrap_or(10).min(100) as usize;

    let results =
        semantic::semantic_search(&cfg.embeddings, &cfg.ollama_url, &cfg.model, query, limit)?;

    if results.is_empty() {
        return Ok(format!("No results found for query: {}", query));
    }

    let rows: Vec<Value> = results
        .iter()
        .map(|r| json!({ "id": r.id, "preferred_term": r.preferred_term, "similarity": r.score }))
        .collect();

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::time::Instant;

    // -----------------------------------------------------------------------
    // Test database helpers
    // -----------------------------------------------------------------------

    fn create_test_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS concepts (
                id             TEXT PRIMARY KEY,
                fsn            TEXT NOT NULL,
                preferred_term TEXT NOT NULL,
                synonyms       TEXT,
                hierarchy      TEXT,
                hierarchy_path TEXT,
                parents        TEXT,
                children_count INTEGER,
                attributes     TEXT,
                active         INTEGER NOT NULL,
                module         TEXT,
                effective_time TEXT,
                ctv3_codes     TEXT,
                read2_codes    TEXT,
                schema_version INTEGER NOT NULL DEFAULT 2
            );
            CREATE TABLE IF NOT EXISTS concept_isa (
                child_id  TEXT NOT NULL,
                parent_id TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS concept_maps (
                code        TEXT NOT NULL,
                terminology TEXT NOT NULL,
                concept_id  TEXT NOT NULL,
                PRIMARY KEY (code, terminology)
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS concepts_fts USING fts5(
                id,
                preferred_term,
                synonyms,
                fsn,
                content='concepts',
                content_rowid='rowid'
            );",
        )
        .unwrap();
    }

    /// Insert a concept with minimal required fields.
    /// `hierarchy_path` should be a JSON array like `["ROOT","CFIND"]`.
    fn insert_concept(
        conn: &Connection,
        id: &str,
        preferred_term: &str,
        fsn: &str,
        hierarchy: &str,
        hierarchy_path: &str,
        synonyms: &str, // JSON array string, e.g. `["syn1","syn2"]`
    ) {
        conn.execute(
            "INSERT INTO concepts
             (id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
              parents, children_count, attributes, active, module, effective_time,
              ctv3_codes, read2_codes, schema_version)
             VALUES (?1,?2,?3,?4,?5,?6,'[]',0,'{}',1,'900000000000207008','20240101','[]','[]',2)",
            params![id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path],
        )
        .unwrap();
    }

    /// Insert `n` duplicate IS-A rows (simulating real RF2 data which has ~6 per relationship).
    fn insert_isa(conn: &Connection, child_id: &str, parent_id: &str, n: usize) {
        for _ in 0..n {
            conn.execute(
                "INSERT INTO concept_isa (child_id, parent_id) VALUES (?1, ?2)",
                params![child_id, parent_id],
            )
            .unwrap();
        }
    }

    fn insert_map(conn: &Connection, code: &str, terminology: &str, concept_id: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO concept_maps (code, terminology, concept_id) VALUES (?1,?2,?3)",
            params![code, terminology, concept_id],
        )
        .unwrap();
    }

    fn rebuild_fts(conn: &Connection) {
        conn.execute_batch("INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild')")
            .unwrap();
    }

    /// Build a representative test database.
    ///
    /// Hierarchy:
    ///   ROOT (1000000)
    ///   ├── CFIND (2000000)  [clinical_finding]
    ///   │   ├── DM (3000000)
    ///   │   │   ├── DM1 (4000000)
    ///   │   │   └── DM2 (5000000)
    ///   │   └── HEART (6000000)
    ///   │       ├── MI  (7000000)  ctv3=X200E
    ///   │       └── HF  (8000000)
    ///   └── PROC (9000000)  [procedure]
    ///       └── CPROC (10000000)
    ///
    /// Each IS-A relationship has 6 duplicate rows (real RF2 characteristic).
    fn build_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_test_schema(&conn);

        // Concepts
        insert_concept(&conn, "1000000", "Root concept", "Root concept (SNOMED CT concept)", "root", r#"["Root concept"]"#, "[]");
        insert_concept(&conn, "2000000", "Clinical finding", "Clinical finding (finding)", "clinical_finding", r#"["Root concept","Clinical finding"]"#, r#"["Finding"]"#);
        insert_concept(&conn, "3000000", "Diabetes mellitus", "Diabetes mellitus (disorder)", "clinical_finding", r#"["Root concept","Clinical finding","Diabetes mellitus"]"#, r#"["DM","Diabetes"]"#);
        insert_concept(&conn, "4000000", "Type 1 diabetes mellitus", "Type 1 diabetes mellitus (disorder)", "clinical_finding", r#"["Root concept","Clinical finding","Diabetes mellitus","Type 1 diabetes mellitus"]"#, "[]");
        insert_concept(&conn, "5000000", "Type 2 diabetes mellitus", "Type 2 diabetes mellitus (disorder)", "clinical_finding", r#"["Root concept","Clinical finding","Diabetes mellitus","Type 2 diabetes mellitus"]"#, "[]");
        insert_concept(&conn, "6000000", "Heart disease", "Heart disease (disorder)", "clinical_finding", r#"["Root concept","Clinical finding","Heart disease"]"#, r#"["Cardiac disease"]"#);
        insert_concept(&conn, "7000000", "Myocardial infarction", "Myocardial infarction (disorder)", "clinical_finding", r#"["Root concept","Clinical finding","Heart disease","Myocardial infarction"]"#, r#"["Heart attack","MI"]"#);
        insert_concept(&conn, "8000000", "Heart failure", "Heart failure (disorder)", "clinical_finding", r#"["Root concept","Clinical finding","Heart disease","Heart failure"]"#, "[]");
        insert_concept(&conn, "9000000", "Procedure", "Procedure (procedure)", "procedure", r#"["Root concept","Procedure"]"#, "[]");
        insert_concept(&conn, "10000000", "Cardiac procedure", "Cardiac procedure (procedure)", "procedure", r#"["Root concept","Procedure","Cardiac procedure"]"#, "[]");

        // IS-A relationships (6 duplicates each, simulating real RF2 data)
        insert_isa(&conn, "2000000", "1000000", 6);
        insert_isa(&conn, "3000000", "2000000", 6);
        insert_isa(&conn, "4000000", "3000000", 6);
        insert_isa(&conn, "5000000", "3000000", 6);
        insert_isa(&conn, "6000000", "2000000", 6);
        insert_isa(&conn, "7000000", "6000000", 6);
        insert_isa(&conn, "8000000", "6000000", 6);
        insert_isa(&conn, "9000000", "1000000", 6);
        insert_isa(&conn, "10000000", "9000000", 6);

        // CTV3 mapping for MI
        insert_map(&conn, "X200E", "ctv3", "7000000");

        rebuild_fts(&conn);
        conn
    }

    /// Build a linear chain of `depth` concepts with `dup` IS-A rows each.
    /// Used to detect recursion explosion (UNION ALL) as a timing regression.
    fn build_chain_db(depth: usize, dup: usize) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_test_schema(&conn);

        for i in 0..depth {
            let id = format!("{}", 1_000_000 + i);
            let term = format!("Concept {i}");
            let fsn = format!("Concept {i} (disorder)");
            let path: Vec<String> = (0..=i).map(|j| format!("Concept {j}")).collect();
            let path_json = serde_json::to_string(&path).unwrap();
            insert_concept(&conn, &id, &term, &fsn, "clinical_finding", &path_json, "[]");
            if i > 0 {
                let parent = format!("{}", 1_000_000 + i - 1);
                insert_isa(&conn, &id, &parent, dup);
            }
        }

        rebuild_fts(&conn);
        conn
    }

    // -----------------------------------------------------------------------
    // tool_children tests
    // -----------------------------------------------------------------------

    #[test]
    fn children_no_duplicates() {
        // With 6 duplicate IS-A rows per relationship, tool_children must still
        // return exactly one row per child (SELECT DISTINCT).
        let conn = build_test_db();
        let args = json!({"id": "3000000", "limit": 100});
        let result = tool_children(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(rows.len(), 2, "DM should have exactly 2 children, not {}", rows.len());
    }

    #[test]
    fn children_alphabetical_order() {
        let conn = build_test_db();
        let args = json!({"id": "3000000", "limit": 100});
        let result = tool_children(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        let terms: Vec<&str> = rows.iter().map(|r| r["preferred_term"].as_str().unwrap()).collect();
        assert_eq!(terms, vec!["Type 1 diabetes mellitus", "Type 2 diabetes mellitus"],
            "children should be sorted alphabetically");
    }

    #[test]
    fn children_empty_for_leaf() {
        let conn = build_test_db();
        let args = json!({"id": "4000000", "limit": 100});
        let result = tool_children(&conn, &args).unwrap();
        assert!(result.contains("No children found"), "leaf node should return no-children message");
    }

    // -----------------------------------------------------------------------
    // tool_ancestors tests
    // -----------------------------------------------------------------------

    #[test]
    fn ancestors_no_duplicates() {
        // With 6 duplicate IS-A rows, ancestors must still return each ancestor once.
        let conn = build_test_db();
        let args = json!({"id": "4000000"});
        let result = tool_ancestors(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        // DM1 → DM → CFIND → ROOT  (3 ancestors)
        assert_eq!(rows.len(), 3, "DM1 should have 3 ancestors, got {}: {}", rows.len(), result);
    }

    #[test]
    fn ancestors_depth_order() {
        // Ancestors should be ordered by depth descending (deepest first = closest to root last).
        // Wait — ORDER BY depth DESC means the deepest hierarchy_path is last alphabetically,
        // but in SNOMED depth is measured from root, so ROOT has depth 1 and leaves have max depth.
        // depth DESC = leaves first, root last.
        let conn = build_test_db();
        let args = json!({"id": "4000000"});
        let result = tool_ancestors(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        // Returned in ORDER BY depth DESC: DM (depth 3) → CFIND (depth 2) → ROOT (depth 1)
        assert_eq!(rows[0]["preferred_term"].as_str().unwrap(), "Diabetes mellitus");
        assert_eq!(rows[2]["preferred_term"].as_str().unwrap(), "Root concept");
    }

    #[test]
    fn ancestors_timing_regression() {
        // A 25-deep linear chain with 6 duplicate IS-A rows would take astronomically long
        // with UNION ALL (6^25 row operations). With UNION it must complete quickly.
        let conn = build_chain_db(25, 6);
        let leaf_id = format!("{}", 1_000_000 + 24);
        let args = json!({"id": leaf_id});

        let start = Instant::now();
        let result = tool_ancestors(&conn, &args).unwrap();
        let elapsed = start.elapsed();

        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(rows.len(), 24, "chain of 25 should have 24 ancestors, got {}", rows.len());
        assert!(
            elapsed.as_millis() < 500,
            "ancestors on 25-deep chain with 6× duplicates took {}ms — UNION ALL explosion?",
            elapsed.as_millis()
        );
    }

    // -----------------------------------------------------------------------
    // tool_search tests
    // -----------------------------------------------------------------------

    #[test]
    fn search_by_preferred_term() {
        let conn = build_test_db();
        let args = json!({"query": "myocardial", "limit": 10});
        let result = tool_search(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"].as_str().unwrap(), "7000000");
    }

    #[test]
    fn search_by_synonym() {
        // "Heart attack" is a synonym of Myocardial infarction in the test DB.
        let conn = build_test_db();
        let args = json!({"query": "Heart attack", "limit": 10});
        let result = tool_search(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert!(
            rows.iter().any(|r| r["id"].as_str() == Some("7000000")),
            "search for synonym 'Heart attack' should find MI (7000000); got: {result}"
        );
    }

    #[test]
    fn search_no_results() {
        let conn = build_test_db();
        let args = json!({"query": "ZZZNOTFOUND", "limit": 10});
        let result = tool_search(&conn, &args).unwrap();
        assert!(result.contains("No results found"), "expected no-results message");
    }

    // -----------------------------------------------------------------------
    // tool_concept tests
    // -----------------------------------------------------------------------

    #[test]
    fn concept_found_by_id() {
        let conn = build_test_db();
        let args = json!({"id": "7000000"});
        let result = tool_concept(&conn, &args).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["preferred_term"].as_str().unwrap(), "Myocardial infarction");
        assert_eq!(v["hierarchy"].as_str().unwrap(), "clinical_finding");
    }

    #[test]
    fn concept_not_found() {
        let conn = build_test_db();
        let args = json!({"id": "9999999999"});
        let result = tool_concept(&conn, &args).unwrap();
        assert!(result.contains("not found"));
    }

    // -----------------------------------------------------------------------
    // tool_hierarchy tests
    // -----------------------------------------------------------------------

    #[test]
    fn hierarchy_filter() {
        let conn = build_test_db();
        let args = json!({"hierarchy": "procedure", "limit": 100});
        let result = tool_hierarchy(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(rows.len(), 2, "procedure hierarchy should have 2 concepts");
        assert!(rows.iter().all(|r| {
            let term = r["preferred_term"].as_str().unwrap_or("");
            term.contains("Procedure") || term.contains("procedure")
        }));
    }

    #[test]
    fn hierarchy_not_found() {
        let conn = build_test_db();
        let args = json!({"hierarchy": "nonexistent", "limit": 100});
        let result = tool_hierarchy(&conn, &args).unwrap();
        assert!(result.contains("No concepts found in hierarchy"));
    }

    // -----------------------------------------------------------------------
    // tool_map tests
    // -----------------------------------------------------------------------

    #[test]
    fn map_snomed_to_ctv3() {
        let conn = build_test_db();
        let args = json!({"code": "7000000", "terminology": "snomed"});
        let result = tool_map(&conn, &args).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let ctv3: Vec<&str> = v["ctv3_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert_eq!(ctv3, vec!["X200E"]);
    }

    #[test]
    fn map_ctv3_to_snomed() {
        let conn = build_test_db();
        let args = json!({"code": "X200E", "terminology": "ctv3"});
        let result = tool_map(&conn, &args).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let concepts = v["snomed_concepts"].as_array().unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0]["id"].as_str().unwrap(), "7000000");
    }

    #[test]
    fn map_no_mappings() {
        // DM has no CTV3 mappings in the test DB.
        let conn = build_test_db();
        let args = json!({"code": "3000000", "terminology": "snomed"});
        let result = tool_map(&conn, &args).unwrap();
        assert!(result.contains("No CTV3 or Read v2 mappings found"));
    }

    #[test]
    fn map_unknown_terminology() {
        let conn = build_test_db();
        let args = json!({"code": "7000000", "terminology": "icd10"});
        assert!(tool_map(&conn, &args).is_err());
    }

    // -----------------------------------------------------------------------
    // sanitise_fts_query tests
    // -----------------------------------------------------------------------

    #[test]
    fn sanitise_single_word_passthrough() {
        assert_eq!(sanitise_fts_query("diabetes"), "diabetes");
        assert_eq!(sanitise_fts_query("  asthma  "), "asthma");
    }

    #[test]
    fn sanitise_multi_word_quoted() {
        assert_eq!(sanitise_fts_query("heart attack"), "\"heart attack\"");
        assert_eq!(sanitise_fts_query("type 2 diabetes"), "\"type 2 diabetes\"");
    }

    #[test]
    fn sanitise_internal_quotes_escaped() {
        assert_eq!(sanitise_fts_query(r#"he said "yes""#), r#""he said ""yes""""#);
    }

    #[test]
    fn sanitise_empty_returns_empty() {
        assert_eq!(sanitise_fts_query(""), "");
        assert_eq!(sanitise_fts_query("   "), "");
    }
}
