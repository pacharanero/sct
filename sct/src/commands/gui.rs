//! `sct gui` — Browser-based UI for SNOMED CT exploration.
//!
//! Starts a local Axum HTTP server bound to 127.0.0.1 only, serves an
//! embedded single-page app, and opens the browser automatically.
//!
//! API routes:
//!   GET /                    → embedded index.html
//!   GET /api/search?q=&limit= → FTS5 search results (JSON)
//!   GET /api/concept/:id      → full concept detail (JSON)
//!   GET /api/children/:id     → immediate IS-A children (JSON)
//!   GET /api/parents/:id      → direct parents (JSON)
//!   GET /api/hierarchy        → list of top-level hierarchy names (JSON)
//!
//! Requires the `gui` Cargo feature: `cargo build --features gui`

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    response::{Html, Json},
    routing::get,
    Router,
};
use clap::Parser;
use rusqlite::{params, Connection, OpenFlags};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::net::TcpListener;

static INDEX_HTML: &str = include_str!("../../assets/index.html");

#[derive(Parser, Debug)]
pub struct Args {
    /// Path to the SNOMED CT SQLite database produced by `sct sqlite`.
    /// Falls back to ./snomed.db then $SCT_DB.
    #[arg(long)]
    pub db: Option<PathBuf>,

    /// TCP port to listen on (default 8420).
    #[arg(long, default_value_t = 8420)]
    pub port: u16,

    /// Do not open a browser window automatically.
    #[arg(long)]
    pub no_open: bool,
}

pub fn run(args: Args) -> Result<()> {
    let db_path = resolve_db_path(args.db)?;
    let port = args.port;
    let no_open = args.no_open;

    // Validate we can open the database before starting the server
    {
        let conn = open_db(&db_path)?;
        drop(conn);
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(serve(db_path, port, no_open))
}

// ---------------------------------------------------------------------------
// DB path resolution
// ---------------------------------------------------------------------------

fn resolve_db_path(arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = arg {
        return Ok(p);
    }
    let default = PathBuf::from("snomed.db");
    if default.exists() {
        return Ok(default);
    }
    if let Ok(env_path) = std::env::var("SCT_DB") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Ok(p);
        }
    }
    anyhow::bail!(
        "No database found. Specify --db <path>, place snomed.db in the current directory, \
         or set $SCT_DB.\nBuild a database first with: sct sqlite --input snomed.ndjson"
    )
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    db_path: Arc<PathBuf>,
}

async fn serve(db_path: PathBuf, port: u16, no_open: bool) -> Result<()> {
    let state = AppState {
        db_path: Arc::new(db_path),
    };

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/search", get(api_search))
        .route("/api/concept/:id", get(api_concept))
        .route("/api/children/:id", get(api_children))
        .route("/api/parents/:id", get(api_parents))
        .route("/api/hierarchy", get(api_hierarchy))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding to 127.0.0.1:{}", port))?;

    let url = format!("http://127.0.0.1:{}", port);
    eprintln!("sct gui: listening on {}  (Ctrl-C to stop)", url);

    if !no_open {
        let url_clone = url.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            if let Err(e) = open::that(&url_clone) {
                eprintln!(
                    "sct gui: could not open browser ({}). Visit {} manually.",
                    e, url_clone
                );
            }
        });
    } else {
        eprintln!("sct gui: open {} in your browser", url);
    }

    axum::serve(listener, app)
        .await
        .context("HTTP server error")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn serve_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<usize>,
}

async fn api_search(
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Json<Value> {
    let query = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(20).min(100);
    match inner_search(&state.db_path, &query, limit) {
        Ok(v) => Json(v),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

fn inner_search(db_path: &PathBuf, query: &str, limit: usize) -> Result<Value> {
    let safe = sanitise_fts(query.trim());
    if safe.is_empty() {
        return Ok(json!({"query": query, "total": 0, "results": []}));
    }
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT f.id, f.preferred_term, f.fsn, c.hierarchy \
         FROM concepts_fts f \
         JOIN concepts c ON c.id = f.id \
         WHERE concepts_fts MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows: Vec<Value> = stmt
        .query_map(params![safe, limit as i64], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "preferred_term": row.get::<_, String>(1)?,
                "fsn": row.get::<_, String>(2)?,
                "hierarchy": row.get::<_, Option<String>>(3)?
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(json!({"query": query, "total": rows.len(), "results": rows}))
}

async fn api_concept(State(state): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    match inner_concept(&state.db_path, &id) {
        Ok(v) => Json(v),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

fn inner_concept(db_path: &PathBuf, id: &str) -> Result<Value> {
    let conn = open_db(db_path)?;
    let result = conn.query_row(
        "SELECT id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path, \
                parents, children_count, attributes \
         FROM concepts WHERE id = ?1",
        params![id],
        |row| {
            let parse_json = |s: Option<String>| -> Value {
                serde_json::from_str(&s.unwrap_or_default()).unwrap_or(Value::Null)
            };
            Ok(json!({
                "id":             row.get::<_, String>(0)?,
                "fsn":            row.get::<_, String>(1)?,
                "preferred_term": row.get::<_, String>(2)?,
                "synonyms":       parse_json(row.get::<_, Option<String>>(3)?),
                "hierarchy":      row.get::<_, Option<String>>(4)?,
                "hierarchy_path": parse_json(row.get::<_, Option<String>>(5)?),
                "parents":        parse_json(row.get::<_, Option<String>>(6)?),
                "children_count": row.get::<_, i64>(7)?,
                "attributes":     parse_json(row.get::<_, Option<String>>(8)?)
            }))
        },
    );
    match result {
        Ok(v) => Ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            anyhow::bail!("concept {} not found", id)
        }
        Err(e) => Err(e.into()),
    }
}

async fn api_children(State(state): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    match inner_children(&state.db_path, &id) {
        Ok(v) => Json(v),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

fn inner_children(db_path: &PathBuf, id: &str) -> Result<Value> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT c.id, c.preferred_term, c.fsn \
         FROM concepts c JOIN concept_isa ci ON ci.child_id = c.id \
         WHERE ci.parent_id = ?1 ORDER BY c.preferred_term LIMIT 200",
    )?;
    let rows: Vec<Value> = stmt
        .query_map(params![id], |row| {
            Ok(json!({
                "id":             row.get::<_, String>(0)?,
                "preferred_term": row.get::<_, String>(1)?,
                "fsn":            row.get::<_, String>(2)?
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(json!({"id": id, "children": rows}))
}

async fn api_parents(State(state): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    match inner_parents(&state.db_path, &id) {
        Ok(v) => Json(v),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

fn inner_parents(db_path: &PathBuf, id: &str) -> Result<Value> {
    let conn = open_db(db_path)?;
    let result = conn.query_row(
        "SELECT parents FROM concepts WHERE id = ?1",
        params![id],
        |row| row.get::<_, Option<String>>(0),
    );
    match result {
        Ok(Some(s)) => {
            let v: Value = serde_json::from_str(&s).unwrap_or(Value::Array(vec![]));
            Ok(json!({"id": id, "parents": v}))
        }
        _ => Ok(json!({"id": id, "parents": []})),
    }
}

async fn api_hierarchy(State(state): State<AppState>) -> Json<Value> {
    match inner_hierarchy(&state.db_path) {
        Ok(v) => Json(v),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

fn inner_hierarchy(db_path: &PathBuf) -> Result<Value> {
    let conn = open_db(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT hierarchy FROM concepts \
         WHERE hierarchy IS NOT NULL AND hierarchy != '' \
         ORDER BY hierarchy",
    )?;
    let hierarchies: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(json!({"hierarchies": hierarchies}))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_db(path: &PathBuf) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening database {}", path.display()))?;
    conn.execute_batch("PRAGMA cache_size = -32768;")?;
    Ok(conn)
}

fn sanitise_fts(q: &str) -> String {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.contains(" OR ")
        || trimmed.contains(" AND ")
        || trimmed.contains('*')
        || trimmed.contains('"')
    {
        return trimmed.to_string();
    }
    if trimmed.contains(' ') {
        format!("\"{}\"", trimmed)
    } else {
        format!("{}*", trimmed)
    }
}
