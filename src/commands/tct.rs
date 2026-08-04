// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct tct` - Build a transitive closure table over the IS-A hierarchy.
//!
//! Computes all (ancestor, descendant, depth) triples from the `concept_isa`
//! table and stores them in `concept_ancestors`. This is an optional
//! optimisation that enables O(1) subsumption queries at query time.
//!
//! Can be applied to any existing `sct sqlite` database without re-reading
//! the original NDJSON input. Also called by `sct sqlite --transitive-closure`.

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct Args {
    /// SQLite database produced by `sct sqlite`.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: PathBuf,

    /// Also insert self-referential rows (ancestor_id = descendant_id, depth = 0).
    ///
    /// Off by default. When present, "descendants including self" queries can
    /// use a single JOIN against concept_ancestors instead of a UNION.
    #[arg(long)]
    pub include_self: bool,
}

pub fn run(args: Args) -> Result<()> {
    let mut conn = Connection::open_with_flags(
        &args.db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening database {}", args.db.display()))?;

    // Performance pragmas - safe for a build-time write operation.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA cache_size = -65536;
         PRAGMA temp_store = MEMORY;",
    )
    .context("setting pragmas")?;

    build(&mut conn, args.include_self)
}

/// Build the transitive closure table.
///
/// Called directly by `sct tct` and also by `sct sqlite --transitive-closure`.
///
/// Refuses to replace a usable closure, repairs its indexes or self rows when
/// requested, and transactionally rebuilds any legacy or incomplete closure.
pub fn build(conn: &mut Connection, include_self: bool) -> Result<()> {
    // Acquire the write lock before reading the hierarchy. A concurrent
    // `sct sqlite` rebuild can therefore run wholly before or after this build,
    // but cannot leave a closure derived from a different hierarchy snapshot.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("beginning TCT transaction")?;
    let include_self = include_self || crate::ecl::eval::tct_marker_includes_self(&tx)?;

    if crate::ecl::eval::has_tct_completion_marker(&tx)? {
        let indexes_usable = crate::ecl::eval::has_tct_indexes(&tx)?;
        let includes_self = crate::ecl::eval::tct_includes_self(&tx)?;
        if indexes_usable && (!include_self || includes_self) {
            let row_count: i64 = tx
                .query_row("SELECT COUNT(*) FROM concept_ancestors", [], |row| {
                    row.get(0)
                })
                .context("counting existing transitive-closure rows")?;
            anyhow::bail!("concept_ancestors already exists with {row_count} rows and is usable");
        }

        if !indexes_usable {
            create_indexes(&tx)?;
        }
        let self_rows = if include_self && !includes_self {
            tx.execute(
                "INSERT INTO concept_ancestors (ancestor_id, descendant_id, depth)
                 SELECT CAST(c.id AS INTEGER), CAST(c.id AS INTEGER), 0
                 FROM concepts c
                 WHERE NOT EXISTS (
                     SELECT 1 FROM concept_ancestors ca
                     WHERE ca.ancestor_id = CAST(c.id AS INTEGER)
                       AND ca.descendant_id = CAST(c.id AS INTEGER)
                 )",
                [],
            )
            .context("adding missing self pairs during TCT repair")?
        } else {
            0
        };
        if include_self && !includes_self {
            write_completion_marker(&tx, true)?;
        }
        tx.commit().context("committing TCT repair")?;

        match (indexes_usable, self_rows) {
            (false, 0) => eprintln!("Done. Repaired transitive-closure indexes."),
            (true, rows) => eprintln!("Done. Added {rows} missing self pairs."),
            (false, rows) => {
                eprintln!("Done. Repaired TCT indexes and added {rows} missing self pairs.")
            }
        }
        return Ok(());
    }

    let pb = crate::progress::spinner("Loading IS-A edges into memory...");

    // Load all concept_isa edges: child_id → [parent_id, …]
    // The whole table fits comfortably in memory (~500k rows for UK Clinical,
    // ~1M for the Monolith). SCTIDs are held as u64: the BFS below hashes and
    // clones them millions of times, and integers hash/copy far more cheaply
    // than the equivalent Strings. concept_isa is TEXT, so CAST at the SQL
    // boundary hands back integers directly.
    let mut parents_of: HashMap<u64, Vec<u64>> = HashMap::new();
    {
        let mut stmt = tx
            .prepare(
                "SELECT CAST(child_id AS INTEGER), CAST(parent_id AS INTEGER) FROM concept_isa",
            )
            .context("preparing concept_isa query")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
            })
            .context("querying concept_isa")?;
        for row in rows {
            let (child, parent) = row.context("reading concept_isa row")?;
            parents_of.entry(child).or_default().push(parent);
        }
    }

    pb.set_message("Loading concept IDs...");

    let mut concepts_stmt = tx
        .prepare("SELECT CAST(id AS INTEGER) FROM concepts ORDER BY id")
        .context("preparing concepts query")?;
    let concepts: Vec<u64> = concepts_stmt
        .query_map([], |r| r.get::<_, i64>(0).map(|v| v as u64))
        .context("querying concepts")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting concept IDs")?;
    drop(concepts_stmt);

    let total = concepts.len();
    pb.finish_and_clear();
    let bar = crate::progress::count_bar(total as u64);
    bar.set_message("Building transitive closure");

    // INTEGER SCTIDs make the table and its indexes substantially cheaper than
    // their TEXT equivalents. Dropping the old table, creating the replacement,
    // filling it, indexing it, and publishing the completion marker all happen
    // in this transaction, so readers see either the old state or the complete
    // replacement.
    drop_invalidation_triggers(&tx)?;
    drop_tct_object(
        &tx,
        "concept_ancestors_meta",
        "DROP TABLE concept_ancestors_meta",
        "DROP VIEW concept_ancestors_meta",
    )?;
    drop_tct_object(
        &tx,
        "concept_ancestors",
        "DROP TABLE concept_ancestors",
        "DROP VIEW concept_ancestors",
    )?;
    tx.execute_batch(
        "CREATE TABLE concept_ancestors (
             ancestor_id   INTEGER NOT NULL,
             descendant_id INTEGER NOT NULL,
             depth         INTEGER NOT NULL
         );",
    )
    .context("creating concept_ancestors table")?;

    {
        let mut insert_stmt = tx
            .prepare(
                "INSERT INTO concept_ancestors (ancestor_id, descendant_id, depth)
                 VALUES (?1, ?2, ?3)",
            )
            .context("preparing insert statement")?;

        for &concept_id in &concepts {
            // BFS upward from this concept through all its ancestors. Because
            // this is BFS, the first encounter is always the shortest path.
            let mut visited: HashSet<u64> = HashSet::new();
            visited.insert(concept_id);

            let mut queue: VecDeque<(u64, i32)> = VecDeque::new();
            queue.push_back((concept_id, 0));

            while let Some((node, depth)) = queue.pop_front() {
                if let Some(parents) = parents_of.get(&node) {
                    for &parent in parents {
                        if visited.insert(parent) {
                            insert_stmt
                                .execute(params![parent as i64, concept_id as i64, depth + 1])
                                .context("inserting ancestor row")?;
                            queue.push_back((parent, depth + 1));
                        }
                    }
                }
            }

            if include_self {
                insert_stmt
                    .execute(params![concept_id as i64, concept_id as i64, 0])
                    .context("inserting self row")?;
            }

            bar.inc(1);
        }
    }

    bar.finish_and_clear();
    create_indexes(&tx)?;
    tx.execute_batch(
        "CREATE TABLE concept_ancestors_meta (
             schema_version INTEGER NOT NULL,
             include_self   INTEGER NOT NULL CHECK (include_self IN (0, 1))
         );",
    )
    .context("creating the transitive-closure completion marker")?;
    create_invalidation_triggers(&tx)?;
    write_completion_marker(&tx, include_self)?;

    let row_count: i64 = tx
        .query_row("SELECT COUNT(*) FROM concept_ancestors", [], |row| {
            row.get(0)
        })
        .context("counting transitive-closure rows")?;
    tx.commit().context("committing TCT transaction")?;

    eprintln!(
        "Done. {} ancestor-descendant pairs in concept_ancestors.",
        row_count
    );

    Ok(())
}

fn drop_tct_object(conn: &Connection, name: &str, drop_table: &str, drop_view: &str) -> Result<()> {
    let object_type = conn
        .query_row(
            "SELECT type FROM sqlite_master WHERE name = ?1",
            [name],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .with_context(|| format!("inspecting existing {name} object"))?;
    match object_type.as_deref() {
        Some("table") => conn.execute_batch(drop_table)?,
        Some("view") => conn.execute_batch(drop_view)?,
        Some(other) => anyhow::bail!("cannot replace {name}: it is a SQLite {other}"),
        None => {}
    }
    Ok(())
}

fn create_indexes(conn: &Connection) -> Result<()> {
    let bar = crate::progress::count_bar(3);
    bar.set_message("Creating transitive closure indexes");

    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_ca_ancestor;
         DROP INDEX IF EXISTS idx_ca_descendant;
         DROP INDEX IF EXISTS idx_ca_pair;",
    )
    .context("removing unusable transitive-closure indexes")?;

    conn.execute(
        "CREATE INDEX idx_ca_ancestor ON concept_ancestors(ancestor_id)",
        [],
    )
    .context("creating ancestor index")?;
    bar.inc(1);

    conn.execute(
        "CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id)",
        [],
    )
    .context("creating descendant index")?;
    bar.inc(1);

    conn.execute(
        "CREATE UNIQUE INDEX idx_ca_pair ON concept_ancestors(ancestor_id, descendant_id)",
        [],
    )
    .context("creating pair index")?;
    bar.inc(1);
    bar.finish_and_clear();
    Ok(())
}

fn drop_invalidation_triggers(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS tct_invalidate_isa_insert;
         DROP TRIGGER IF EXISTS tct_invalidate_isa_update;
         DROP TRIGGER IF EXISTS tct_invalidate_isa_delete;
         DROP TRIGGER IF EXISTS tct_invalidate_concepts_insert;
         DROP TRIGGER IF EXISTS tct_invalidate_concepts_update;
         DROP TRIGGER IF EXISTS tct_invalidate_concepts_delete;
         DROP TRIGGER IF EXISTS tct_invalidate_ca_insert;
         DROP TRIGGER IF EXISTS tct_invalidate_ca_update;
         DROP TRIGGER IF EXISTS tct_invalidate_ca_delete;",
    )
    .context("removing transitive-closure invalidation triggers")
}

fn create_invalidation_triggers(conn: &Connection) -> Result<()> {
    drop_invalidation_triggers(conn)?;
    conn.execute_batch(crate::ecl::eval::TCT_INVALIDATION_TRIGGERS_SQL)
        .context("creating transitive-closure invalidation triggers")
}

fn write_completion_marker(conn: &Connection, include_self: bool) -> Result<()> {
    conn.execute("DELETE FROM concept_ancestors_meta", [])
        .context("clearing the transitive-closure completion marker")?;
    conn.execute(
        "INSERT INTO concept_ancestors_meta (schema_version, include_self)
         VALUES (?1, ?2)",
        params![
            crate::ecl::eval::TCT_SCHEMA_VERSION,
            i64::from(include_self)
        ],
    )
    .context("publishing the transitive-closure completion marker")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hierarchy_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (id TEXT NOT NULL);
             INSERT INTO concepts VALUES ('1'), ('2'), ('3');
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             INSERT INTO concept_isa VALUES ('2', '1'), ('3', '2');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn missing_database_is_not_created() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.db");

        let error = run(Args {
            db: path.clone(),
            include_self: false,
        })
        .unwrap_err();

        assert!(!path.exists());
        assert!(error.to_string().contains("opening database"));
    }

    #[test]
    fn populated_table_with_missing_indexes_is_repaired_in_place() {
        let mut conn = hierarchy_db();
        build(&mut conn, false).unwrap();
        conn.execute("DROP INDEX idx_ca_descendant", []).unwrap();
        assert!(!crate::ecl::eval::has_tct(&conn).unwrap());

        build(&mut conn, false).unwrap();
        assert!(crate::ecl::eval::has_tct(&conn).unwrap());
        assert!(crate::ecl::eval::has_tct_indexes(&conn).unwrap());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM concept_ancestors", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert!(build(&mut conn, false)
            .unwrap_err()
            .to_string()
            .contains("already exists"));
    }

    #[test]
    fn usable_table_can_be_upgraded_to_include_self() {
        let mut conn = hierarchy_db();
        build(&mut conn, false).unwrap();

        build(&mut conn, true).unwrap();
        let self_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM concept_ancestors
                 WHERE ancestor_id = descendant_id AND depth = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(self_rows, 3);
        assert!(crate::ecl::eval::tct_includes_self(&conn).unwrap());
        assert!(crate::ecl::eval::has_tct(&conn).unwrap());

        conn.execute("INSERT INTO concepts VALUES ('4')", [])
            .unwrap();
        assert!(!crate::ecl::eval::has_tct(&conn).unwrap());
        build(&mut conn, false).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM concept_ancestors
                 WHERE ancestor_id = descendant_id AND depth = 0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            4
        );
        assert!(crate::ecl::eval::tct_includes_self(&conn).unwrap());
    }

    #[test]
    fn legacy_partial_table_is_rebuilt_instead_of_trusted() {
        let mut conn = hierarchy_db();
        conn.execute_batch(
            "CREATE TABLE concept_ancestors (
                 ancestor_id INTEGER NOT NULL,
                 descendant_id INTEGER NOT NULL,
                 depth INTEGER NOT NULL
             );
             INSERT INTO concept_ancestors VALUES (1, 2, 1);
             CREATE INDEX idx_ca_ancestor ON concept_ancestors(ancestor_id);
             CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id);
             CREATE UNIQUE INDEX idx_ca_pair
                 ON concept_ancestors(ancestor_id, descendant_id);",
        )
        .unwrap();
        assert!(!crate::ecl::eval::has_tct(&conn).unwrap());

        build(&mut conn, false).unwrap();
        assert!(crate::ecl::eval::has_tct(&conn).unwrap());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM concept_ancestors", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }

    #[test]
    fn view_named_like_the_closure_is_replaced() {
        let mut conn = hierarchy_db();
        conn.execute_batch(
            "CREATE VIEW concept_ancestors AS
             SELECT CAST(ancestor_id AS INTEGER) AS ancestor_id,
                    CAST(descendant_id AS INTEGER) AS descendant_id,
                    CAST(depth AS INTEGER) AS depth
             FROM missing_closure_source;",
        )
        .unwrap();

        build(&mut conn, false).unwrap();

        assert!(crate::ecl::eval::has_tct(&conn).unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT type FROM sqlite_master WHERE name = 'concept_ancestors'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "table"
        );
    }

    #[test]
    fn malformed_named_indexes_are_replaced() {
        let mut conn = hierarchy_db();
        build(&mut conn, false).unwrap();
        conn.execute_batch(
            "DROP INDEX idx_ca_ancestor;
             DROP INDEX idx_ca_descendant;
             DROP INDEX idx_ca_pair;
             CREATE INDEX idx_ca_ancestor ON concept_ancestors(depth);
             CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id DESC);
             CREATE UNIQUE INDEX idx_ca_pair
                 ON concept_ancestors(descendant_id, ancestor_id);",
        )
        .unwrap();
        assert!(!crate::ecl::eval::has_tct_indexes(&conn).unwrap());

        build(&mut conn, false).unwrap();
        assert!(crate::ecl::eval::has_tct_indexes(&conn).unwrap());
        assert!(crate::ecl::eval::has_tct(&conn).unwrap());
    }

    #[test]
    fn closure_and_source_mutations_invalidate_completion() {
        let mut conn = hierarchy_db();
        build(&mut conn, false).unwrap();

        conn.execute(
            "DELETE FROM concept_ancestors WHERE ancestor_id = 1 AND descendant_id = 3",
            [],
        )
        .unwrap();
        assert!(!crate::ecl::eval::has_tct(&conn).unwrap());
        build(&mut conn, false).unwrap();
        assert!(crate::ecl::eval::has_tct(&conn).unwrap());

        conn.execute("INSERT INTO concepts VALUES ('4')", [])
            .unwrap();
        conn.execute("INSERT INTO concept_isa VALUES ('4', '3')", [])
            .unwrap();
        assert!(!crate::ecl::eval::has_tct(&conn).unwrap());
        build(&mut conn, false).unwrap();
        assert!(crate::ecl::eval::has_tct(&conn).unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT depth FROM concept_ancestors
                 WHERE ancestor_id = 1 AND descendant_id = 4",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            3
        );
    }

    #[test]
    fn missing_invalidation_trigger_forces_a_rebuild() {
        let mut conn = hierarchy_db();
        build(&mut conn, false).unwrap();
        conn.execute("DROP TRIGGER tct_invalidate_ca_delete", [])
            .unwrap();
        assert!(!crate::ecl::eval::has_tct(&conn).unwrap());

        build(&mut conn, false).unwrap();
        assert!(crate::ecl::eval::has_tct(&conn).unwrap());
    }

    #[test]
    fn indexes_are_repaired_before_self_pairs_are_added() {
        let mut conn = hierarchy_db();
        build(&mut conn, false).unwrap();
        conn.execute_batch(
            "DROP INDEX idx_ca_pair;
             CREATE UNIQUE INDEX idx_ca_pair ON concept_ancestors(depth) WHERE depth = 0;",
        )
        .unwrap();

        build(&mut conn, true).unwrap();
        assert!(crate::ecl::eval::has_tct(&conn).unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM concept_ancestors
                 WHERE ancestor_id = descendant_id AND depth = 0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            3
        );
    }
}
