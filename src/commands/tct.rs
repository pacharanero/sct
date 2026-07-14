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
use rusqlite::{params, Connection};
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
    let mut conn = Connection::open(&args.db)
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
/// Errors if `concept_ancestors` already contains rows. To rebuild, drop the
/// table first: `sqlite3 your.db 'DROP TABLE concept_ancestors;'`
pub fn build(conn: &mut Connection, include_self: bool) -> Result<()> {
    // Guard: refuse to overwrite an existing populated TCT.
    let tct_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='table' AND name='concept_ancestors'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .context("checking for existing concept_ancestors table")?;

    if tct_exists {
        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM concept_ancestors", [], |r| r.get(0))
            .unwrap_or(0);
        if row_count > 0 {
            anyhow::bail!(
                "concept_ancestors already exists with {} rows. \
                 Drop it first to rebuild:\n  \
                 sqlite3 your.db 'DROP TABLE concept_ancestors;'",
                row_count,
            );
        }
    } else {
        // INTEGER id columns: SCTIDs are numeric, so the three indexes built
        // below sort integers (cheap) instead of TEXT. On the full UK Monolith
        // that TEXT sort during index creation was the single largest cost of
        // this step (profiled: ~21% of instructions in vdbeSorterCompareText +
        // its memcmp). This is an internal derived table - nothing JOINs it to
        // the TEXT `concepts.id` - so the INTEGER affinity stays self-contained.
        conn.execute_batch(
            "CREATE TABLE concept_ancestors (
                ancestor_id   INTEGER NOT NULL,
                descendant_id INTEGER NOT NULL,
                depth         INTEGER NOT NULL
            );",
        )
        .context("creating concept_ancestors table")?;
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
        let mut stmt = conn
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

    let mut concepts_stmt = conn
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

    {
        let tx = conn.transaction().context("beginning TCT transaction")?;

        {
            let mut insert_stmt = tx
                .prepare(
                    "INSERT INTO concept_ancestors (ancestor_id, descendant_id, depth)
                     VALUES (?1, ?2, ?3)",
                )
                .context("preparing insert statement")?;

            for &concept_id in &concepts {
                // BFS upward from this concept through all its ancestors.
                //
                // Because this is BFS, the first time we encounter any given
                // ancestor is always via the shortest path - no deduplication
                // or MIN(depth) logic is needed beyond the visited set.
                // SCTIDs are u64 (Copy), so nodes move through the queue and
                // visited set without allocation or cloning.
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
        } // insert_stmt dropped, releasing borrow on tx

        tx.commit().context("committing TCT transaction")?;
    }

    bar.finish_and_clear();
    let pb = crate::progress::count_bar(3);
    pb.set_message("Creating transitive closure indexes");

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_ca_ancestor ON concept_ancestors(ancestor_id)",
        [],
    )
    .context("creating ancestor index")?;
    pb.inc(1);

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_ca_descendant ON concept_ancestors(descendant_id)",
        [],
    )
    .context("creating descendant index")?;
    pb.inc(1);

    conn.execute("CREATE UNIQUE INDEX IF NOT EXISTS idx_ca_pair ON concept_ancestors(ancestor_id, descendant_id)", [])
        .context("creating pair index")?;
    pb.inc(1);

    let row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM concept_ancestors", [], |r| r.get(0))
        .unwrap_or(0);

    eprintln!(
        "Done. {} ancestor-descendant pairs in concept_ancestors.",
        row_count
    );

    Ok(())
}
