//! `sct tct` — Build a transitive closure table over the IS-A hierarchy.
//!
//! Computes all (ancestor, descendant, depth) triples from the `concept_isa`
//! table and stores them in `concept_ancestors`. This is an optional
//! optimisation that enables O(1) subsumption queries at query time.
//!
//! Can be applied to any existing `sct sqlite` database without re-reading
//! the original NDJSON input. Also called by `sct sqlite --transitive-closure`.

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
pub struct Args {
    /// SQLite database produced by `sct sqlite`.
    #[arg(long)]
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

    // Performance pragmas — safe for a build-time write operation.
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
        conn.execute_batch(
            "CREATE TABLE concept_ancestors (
                ancestor_id   TEXT NOT NULL,
                descendant_id TEXT NOT NULL,
                depth         INTEGER NOT NULL
            );",
        )
        .context("creating concept_ancestors table")?;
    }

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} [{elapsed_precise}] {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(120));

    pb.set_message("Loading IS-A edges into memory...");

    // Load all concept_isa edges: child_id → [parent_id, …]
    // The whole table fits comfortably in memory (~500k rows for UK Clinical,
    // ~1M for the Monolith).
    let mut parents_of: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT child_id, parent_id FROM concept_isa")
            .context("preparing concept_isa query")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .context("querying concept_isa")?;
        for row in rows {
            let (child, parent) = row.context("reading concept_isa row")?;
            parents_of.entry(child).or_default().push(parent);
        }
    }

    pb.set_message("Loading concept IDs...");

    let mut concepts_stmt = conn
        .prepare("SELECT id FROM concepts ORDER BY id")
        .context("preparing concepts query")?;
    let concepts: Vec<String> = concepts_stmt
        .query_map([], |r| r.get::<_, String>(0))
        .context("querying concepts")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting concept IDs")?;
    drop(concepts_stmt);

    let total = concepts.len();
    pb.set_message(format!("Building TCT for {} concepts (0/{})...", total, total));

    {
        let tx = conn.transaction().context("beginning TCT transaction")?;

        {
            let mut insert_stmt = tx
                .prepare(
                    "INSERT INTO concept_ancestors (ancestor_id, descendant_id, depth)
                     VALUES (?1, ?2, ?3)",
                )
                .context("preparing insert statement")?;

            for (i, concept_id) in concepts.iter().enumerate() {
                // BFS upward from this concept through all its ancestors.
                //
                // Because this is BFS, the first time we encounter any given
                // ancestor is always via the shortest path — no deduplication
                // or MIN(depth) logic is needed beyond the visited set.
                let mut visited: HashSet<String> = HashSet::new();
                visited.insert(concept_id.clone());

                let mut queue: VecDeque<(String, i32)> = VecDeque::new();
                queue.push_back((concept_id.clone(), 0));

                while let Some((node, depth)) = queue.pop_front() {
                    if let Some(parents) = parents_of.get(&node) {
                        for parent in parents {
                            if visited.insert(parent.clone()) {
                                insert_stmt
                                    .execute(params![parent, concept_id, depth + 1])
                                    .context("inserting ancestor row")?;
                                queue.push_back((parent.clone(), depth + 1));
                            }
                        }
                    }
                }

                if include_self {
                    insert_stmt
                        .execute(params![concept_id, concept_id, 0])
                        .context("inserting self row")?;
                }

                if (i + 1) % 5_000 == 0 {
                    pb.set_message(format!(
                        "Building TCT for {} concepts ({}/{})...",
                        total,
                        i + 1,
                        total
                    ));
                }
            }
        } // insert_stmt dropped, releasing borrow on tx

        tx.commit().context("committing TCT transaction")?;
    }

    pb.set_message("Creating indexes...");

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_ca_ancestor
             ON concept_ancestors(ancestor_id);
         CREATE INDEX IF NOT EXISTS idx_ca_descendant
             ON concept_ancestors(descendant_id);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_ca_pair
             ON concept_ancestors(ancestor_id, descendant_id);",
    )
    .context("creating concept_ancestors indexes")?;

    let row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM concept_ancestors", [], |r| r.get(0))
        .unwrap_or(0);

    pb.finish_with_message(format!(
        "Done. {} ancestor-descendant pairs in concept_ancestors.",
        row_count
    ));

    Ok(())
}
