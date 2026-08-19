// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

mod batch;
pub mod bench;
pub mod codelist;
pub mod completions;
pub mod crosswalk;
pub mod diagram;
pub mod diff;
#[cfg(feature = "dmwb")]
pub mod dmwb;
pub mod ecl;
pub mod embed;
mod embedding_profile;
pub mod fst;
pub mod history;
pub mod info;
pub mod lexical;
pub mod lookup;
pub mod map;
pub mod markdown;
pub mod mcp;
pub mod ndjson;
mod ollama;
pub mod parquet;
pub mod paths;
pub mod proximal_primitives;
pub mod read2;
pub mod refset;
pub mod sayt;
pub mod semantic;
pub mod semantic_benchmark;
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
use std::path::{Path, PathBuf};

/// Settle a build command's output path: the `--output` flag if given, else a
/// name derived from the input (see [`crate::paths::derived_output`]).
///
/// A derived name is announced on stderr, because a filename the user did not
/// type should never be a surprise - and because the next command in the
/// pipeline needs to know what to consume. An explicit `--output` is not
/// announced; they already know where it went.
pub(crate) fn resolve_output(flag: Option<&Path>, input: &Path, suffix: &str) -> PathBuf {
    match flag {
        Some(path) => path.to_path_buf(),
        None => {
            let derived = crate::paths::derived_output(input, suffix);
            eprintln!("Output: {}", derived.display());
            derived
        }
    }
}

/// Open a SNOMED CT SQLite database in read-only query mode.
///
/// Sets `PRAGMA query_only = ON` so any accidental write attempt fails fast,
/// and applies an optional cache size hint (KiB; pass `None` for SQLite's
/// default page-based cache). Used by every read-side subcommand
/// (`sct lookup`, `sct lexical`, `sct refset`, `sct codelist`, `sct info`,
/// `sct mcp`) so they share one consistent connection profile.
pub(crate) fn open_db_readonly(path: &Path, cache_size_kib: Option<u32>) -> Result<Connection> {
    crate::sdk::open_db_readonly(path, cache_size_kib).map_err(Into::into)
}

/// Ensure the shared cross-terminology projection can retain every map row.
///
/// Older databases used a composite primary key that omitted map priority and
/// member identity, so valid RF2 alternatives could collide. A surrogate key
/// leaves the source-specific columns available for querying without imposing
/// false uniqueness on them.
pub(crate) fn ensure_crossmaps_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS crossmaps (
            row_id          INTEGER PRIMARY KEY,
            source_system   TEXT NOT NULL,
            source_code     TEXT NOT NULL,
            source_term_code TEXT,
            target_system   TEXT NOT NULL,
            target_code     TEXT NOT NULL,
            target_description_id TEXT,
            map_refset      TEXT NOT NULL,
            map_source      TEXT NOT NULL DEFAULT 'rf2',
            map_id          TEXT,
            effective_date  TEXT,
            active          INTEGER NOT NULL DEFAULT 1,
            map_status      TEXT,
            map_group       INTEGER,
            map_priority    INTEGER,
            map_rule        TEXT,
            map_advice      TEXT,
            correlation     TEXT,
            is_assured      INTEGER,
            metadata_json   TEXT NOT NULL DEFAULT '{}'
        );",
    )?;

    let columns = [
        ("source_term_code", "TEXT"),
        ("target_description_id", "TEXT"),
        ("map_source", "TEXT NOT NULL DEFAULT 'rf2'"),
        ("map_id", "TEXT"),
        ("effective_date", "TEXT"),
        ("active", "INTEGER NOT NULL DEFAULT 1"),
        ("map_status", "TEXT"),
        ("map_group", "INTEGER"),
        ("map_priority", "INTEGER"),
        ("map_rule", "TEXT"),
        ("map_advice", "TEXT"),
        ("correlation", "TEXT"),
        ("is_assured", "INTEGER"),
        ("metadata_json", "TEXT NOT NULL DEFAULT '{}'"),
    ];
    for (name, definition) in columns {
        if !sqlite_column_exists(conn, "crossmaps", name)? {
            conn.execute(
                &format!("ALTER TABLE crossmaps ADD COLUMN {name} {definition}"),
                [],
            )?;
        }
    }

    if !sqlite_column_exists(conn, "crossmaps", "row_id")? {
        conn.execute_batch("SAVEPOINT migrate_crossmaps_key")?;
        let migration = conn.execute_batch(
            "ALTER TABLE crossmaps RENAME TO crossmaps_legacy;
             CREATE TABLE crossmaps (
                row_id          INTEGER PRIMARY KEY,
                source_system   TEXT NOT NULL,
                source_code     TEXT NOT NULL,
                source_term_code TEXT,
                target_system   TEXT NOT NULL,
                target_code     TEXT NOT NULL,
                target_description_id TEXT,
                map_refset      TEXT NOT NULL,
                map_source      TEXT NOT NULL DEFAULT 'rf2',
                map_id          TEXT,
                effective_date  TEXT,
                active          INTEGER NOT NULL DEFAULT 1,
                map_status      TEXT,
                map_group       INTEGER,
                map_priority    INTEGER,
                map_rule        TEXT,
                map_advice      TEXT,
                correlation     TEXT,
                is_assured      INTEGER,
                metadata_json   TEXT NOT NULL DEFAULT '{}'
             );
             INSERT INTO crossmaps (
                source_system, source_code, source_term_code, target_system,
                target_code, target_description_id, map_refset, map_source,
                map_id, effective_date, active, map_status, map_group,
                map_priority, map_rule, map_advice, correlation, is_assured,
                metadata_json
             )
             SELECT source_system, source_code, source_term_code, target_system,
                target_code, target_description_id, map_refset, map_source,
                map_id, effective_date, active, map_status, map_group,
                map_priority, map_rule, map_advice, correlation, is_assured,
                metadata_json
             FROM crossmaps_legacy;
             DROP TABLE crossmaps_legacy;",
        );
        if let Err(error) = migration {
            let _ = conn.execute_batch(
                "ROLLBACK TO SAVEPOINT migrate_crossmaps_key;
                 RELEASE SAVEPOINT migrate_crossmaps_key;",
            );
            return Err(error).context("migrating crossmaps primary key");
        }
        conn.execute_batch("RELEASE SAVEPOINT migrate_crossmaps_key")?;
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_crossmaps_src ON crossmaps(source_system, source_code);
         CREATE INDEX IF NOT EXISTS idx_crossmaps_tgt ON crossmaps(target_system, target_code);",
    )?;
    Ok(())
}

fn sqlite_column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Get the total size of a concept's subtree (including itself).
/// Uses the transitive closure table if available, falling back to a recursive query.
pub(crate) fn get_subtree_size(conn: &Connection, concept_id: &str) -> Result<u64> {
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn)?;
    get_subtree_size_with_tct(conn, concept_id, crate::ecl::eval::has_tct(conn)?)
}

pub(crate) fn get_subtree_size_with_tct(
    conn: &Connection,
    concept_id: &str,
    tct: bool,
) -> Result<u64> {
    let count: u64 = if tct {
        let cnt: i64 = conn.query_row(
            "SELECT COUNT(*) FROM (
                SELECT descendant_id FROM concept_ancestors WHERE ancestor_id = ?1
                UNION
                SELECT CAST(?1 AS INTEGER)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_open_does_not_create_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.db");

        let err = open_db_readonly(&path, None).unwrap_err();

        assert!(!path.exists());
        assert!(err.to_string().contains("read-only"));
    }

    #[test]
    fn readonly_open_rejects_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE example (id INTEGER)", [])
            .unwrap();
        drop(conn);

        let conn = open_db_readonly(&path, None).unwrap();
        assert!(conn
            .execute("INSERT INTO example (id) VALUES (1)", [])
            .is_err());
    }

    #[test]
    fn crossmaps_schema_migrates_lossy_composite_key() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE crossmaps (
                source_system TEXT NOT NULL,
                source_code TEXT NOT NULL,
                target_system TEXT NOT NULL,
                target_code TEXT NOT NULL,
                map_refset TEXT NOT NULL,
                map_group INTEGER,
                map_priority INTEGER,
                PRIMARY KEY (
                    source_system, source_code, target_system, target_code,
                    map_refset, map_group
                )
             );
             INSERT INTO crossmaps (
                source_system, source_code, target_system, target_code,
                map_refset, map_group, map_priority
             ) VALUES ('snomed', '1', 'icd10', 'A01', 'map', 1, 1);",
        )
        .unwrap();

        ensure_crossmaps_schema(&conn).unwrap();

        assert!(sqlite_column_exists(&conn, "crossmaps", "row_id").unwrap());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM crossmaps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        conn.execute(
            "INSERT INTO crossmaps (
                source_system, source_code, target_system, target_code,
                map_refset, map_group, map_priority
             ) VALUES ('snomed', '1', 'icd10', 'A01', 'map', 1, 2)",
            [],
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM crossmaps", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
