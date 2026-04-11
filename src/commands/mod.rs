pub mod codelist;
pub mod completions;
pub mod diff;
pub mod embed;
pub mod info;
pub mod lexical;
pub mod lookup;
pub mod markdown;
pub mod mcp;
pub mod ndjson;
pub mod parquet;
pub mod refset;
pub mod semantic;
pub mod sqlite;
pub mod tct;
pub mod trud;

#[cfg(feature = "tui")]
pub mod tui;

#[cfg(feature = "gui")]
pub mod gui;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// Open a SNOMED CT SQLite database in read-only query mode.
///
/// Sets `PRAGMA query_only = ON` so any accidental write attempt fails fast,
/// and applies an optional cache size hint (KiB; pass `None` for SQLite's
/// default page-based cache). Used by every read-side subcommand
/// (`sct lookup`, `sct lexical`, `sct refset`, `sct codelist`, `sct info`,
/// `sct mcp`) so they share one consistent connection profile.
pub(crate) fn open_db_readonly(path: &Path, cache_size_kib: Option<u32>) -> Result<Connection> {
    let conn =
        Connection::open(path).with_context(|| format!("opening database {}", path.display()))?;
    let mut pragmas = String::from("PRAGMA query_only = ON;");
    if let Some(kib) = cache_size_kib {
        pragmas.push_str(&format!("PRAGMA cache_size = -{kib};"));
    }
    conn.execute_batch(&pragmas)?;
    Ok(conn)
}
