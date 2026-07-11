// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

pub mod codelist;
pub mod completions;
pub mod crosswalk;
pub mod diagram;
pub mod diff;
#[cfg(feature = "dmwb")]
pub mod dmwb;
pub mod ecl;
pub mod embed;
pub mod fst;
pub mod info;
pub mod lexical;
pub mod lookup;
pub mod map;
pub mod markdown;
pub mod mcp;
pub mod ndjson;
pub mod parquet;
pub mod paths;
pub mod read2;
pub mod refset;
pub mod sayt;
pub mod semantic;
pub mod sqlite;
pub mod tct;
pub mod transcode;
pub mod trud;

pub mod size;

#[cfg(feature = "tui")]
pub mod tui;

#[cfg(feature = "gui")]
pub mod gui;

#[cfg(feature = "serve")]
pub mod serve;

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
    // `query_only` makes any write an error; `mmap_size` memory-maps the
    // database so reads come straight from the OS page-mapped file instead of
    // being copied through per-connection buffers - a real win for the
    // read-only query paths and, especially, the `sct serve` connection pool
    // (all pooled connections then share the mapped file rather than each
    // buffering it). SQLite clamps mmap_size to the file size and its
    // compile-time maximum, so an over-large request is harmless.
    let mut pragmas = String::from("PRAGMA query_only = ON; PRAGMA mmap_size = 2147483648;");
    if let Some(kib) = cache_size_kib {
        pragmas.push_str(&format!("PRAGMA cache_size = -{kib};"));
    }
    conn.execute_batch(&pragmas)?;
    Ok(conn)
}

/// Get the total size of a concept's subtree (including itself).
/// Uses the transitive closure table if available, falling back to a recursive query.
pub(crate) fn get_subtree_size(conn: &Connection, concept_id: &str) -> Result<u64> {
    let has_tct = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='concept_ancestors'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    let count: u64 = if has_tct {
        let cnt: i64 = conn.query_row(
            "SELECT COUNT(*) FROM (
                SELECT descendant_id FROM concept_ancestors WHERE ancestor_id = ?1
                UNION
                SELECT ?1
             )",
            rusqlite::params![concept_id],
            |r| r.get(0),
        )?;
        cnt as u64
    } else {
        let cnt: i64 = conn.query_row(
            "WITH RECURSIVE descendants(id) AS (
                SELECT ?1
                UNION
                SELECT child_id FROM concept_isa JOIN descendants ON parent_id = id
             )
             SELECT COUNT(DISTINCT id) FROM descendants",
            rusqlite::params![concept_id],
            |r| r.get(0),
        )?;
        cnt as u64
    };
    Ok(count)
}
