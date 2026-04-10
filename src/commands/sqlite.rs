//! `sct sqlite` — Load a SNOMED CT NDJSON artefact into a SQLite database with FTS5.
//!
//! Creates:
//!   - `concepts` table (all fields)
//!   - `concept_isa` table (child_id, parent_id) — indexed for fast children/ancestor queries
//!   - `concept_maps` table (code → concept reverse lookup for CTV3 / Read v2)
//!   - `refset_members` table (refset_id → concept_id) — refset membership
//!   - `concepts_fts` FTS5 virtual table over id, preferred_term, synonyms, fsn
//!   - `concept_ancestors` table (optional, --transitive-closure) — precomputed TCT

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rusqlite::{params, Connection};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::Duration;

use crate::schema::ConceptRecord;

#[derive(Parser, Debug)]
pub struct Args {
    /// Input NDJSON file produced by `sct ndjson`. Use `-` for stdin.
    #[arg(long, short)]
    pub input: PathBuf,

    /// Output SQLite database file.
    #[arg(long, short, default_value = "snomed.db")]
    pub output: PathBuf,

    /// Build the transitive closure table (concept_ancestors) after loading.
    ///
    /// Equivalent to running `sct tct --db <output>` immediately after.
    /// Adds significant build time and database size; only needed for
    /// subsumption-heavy workloads or the SCT-QL compiler.
    #[arg(long)]
    pub transitive_closure: bool,

    /// Include self-referential rows in the TCT (ancestor_id = descendant_id, depth = 0).
    /// Only meaningful when --transitive-closure is also set.
    #[arg(long)]
    pub include_self: bool,
}

pub fn run(args: Args) -> Result<()> {
    let input: Box<dyn std::io::Read> = if args.input.as_os_str() == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(
            std::fs::File::open(&args.input)
                .with_context(|| format!("opening {}", args.input.display()))?,
        )
    };

    let reader = BufReader::new(input);

    eprintln!("Opening database {}...", args.output.display());
    let mut conn = Connection::open(&args.output)
        .with_context(|| format!("opening database {}", args.output.display()))?;

    // Performance pragmas — safe for a build-time operation
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA cache_size = -65536;
         PRAGMA temp_store = MEMORY;",
    )?;

    create_schema(&conn)?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} [{elapsed_precise}] {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(120));
    pb.set_message("Loading concepts...");

    let mut n = 0usize;
    {
        let tx = conn.transaction().context("beginning transaction")?;

        let mut insert_concept = tx.prepare(
            "INSERT OR REPLACE INTO concepts
             (id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
              parents, children_count, attributes, active, module, effective_time,
              ctv3_codes, read2_codes, schema_version)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        )?;

        let mut insert_isa =
            tx.prepare("INSERT INTO concept_isa (child_id, parent_id) VALUES (?1, ?2)")?;

        let mut insert_map = tx.prepare(
            "INSERT OR IGNORE INTO concept_maps (code, terminology, concept_id) VALUES (?1, ?2, ?3)",
        )?;

        let mut insert_refset_member = tx.prepare(
            "INSERT OR IGNORE INTO refset_members (refset_id, referenced_component_id) VALUES (?1, ?2)",
        )?;

        for line in reader.lines() {
            let line = line.context("reading input")?;
            if line.trim().is_empty() {
                continue;
            }

            let record: ConceptRecord =
                serde_json::from_str(&line).context("parsing NDJSON record")?;

            let synonyms_json = serde_json::to_string(&record.synonyms)?;
            let hierarchy_path_json = serde_json::to_string(&record.hierarchy_path)?;
            let parents_json = serde_json::to_string(&record.parents)?;
            let attributes_json = serde_json::to_string(&record.attributes)?;
            let ctv3_json = serde_json::to_string(&record.ctv3_codes)?;
            let read2_json = serde_json::to_string(&record.read2_codes)?;

            insert_concept.execute(params![
                record.id,
                record.fsn,
                record.preferred_term,
                synonyms_json,
                record.hierarchy,
                hierarchy_path_json,
                parents_json,
                record.children_count as i64,
                attributes_json,
                record.active as i32,
                record.module,
                record.effective_time,
                ctv3_json,
                read2_json,
                record.schema_version as i64,
            ])?;

            for parent in &record.parents {
                insert_isa.execute(params![record.id, parent.id])?;
            }

            for code in &record.ctv3_codes {
                insert_map.execute(params![code, "ctv3", record.id])?;
            }
            for code in &record.read2_codes {
                insert_map.execute(params![code, "read2", record.id])?;
            }

            for refset_id in &record.refsets {
                insert_refset_member.execute(params![refset_id, record.id])?;
            }

            n += 1;
            if n.is_multiple_of(50_000) {
                pb.set_message(format!("{} concepts loaded...", n));
            }
        }

        drop(insert_concept);
        drop(insert_isa);
        drop(insert_map);
        drop(insert_refset_member);
        tx.commit().context("committing transaction")?;
    }

    pb.set_message(format!("{} concepts committed; creating indexes...", n));

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_concepts_hierarchy ON concepts(hierarchy);
         CREATE INDEX IF NOT EXISTS idx_concept_isa_parent ON concept_isa(parent_id);
         CREATE INDEX IF NOT EXISTS idx_concept_isa_child  ON concept_isa(child_id);
         CREATE INDEX IF NOT EXISTS idx_concept_maps_concept ON concept_maps(concept_id);
         CREATE INDEX IF NOT EXISTS idx_refset_members_by_concept
             ON refset_members(referenced_component_id);",
    )?;

    pb.set_message("Building FTS index...");
    conn.execute_batch("INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild')")?;

    pb.finish_with_message(format!("Done. {} concepts → {}", n, args.output.display()));

    if args.transitive_closure {
        crate::commands::tct::build(&mut conn, args.include_self)?;
    }

    Ok(())
}

fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS concepts (
            id             TEXT PRIMARY KEY,
            fsn            TEXT NOT NULL,
            preferred_term TEXT NOT NULL,
            synonyms       TEXT,            -- JSON array of strings
            hierarchy      TEXT,
            hierarchy_path TEXT,            -- JSON array of strings
            parents        TEXT,            -- JSON array of {id, fsn}
            children_count INTEGER,
            attributes     TEXT,            -- JSON object
            active         INTEGER NOT NULL,
            module         TEXT,
            effective_time TEXT,
            ctv3_codes     TEXT,            -- JSON array of CTV3 code strings
            read2_codes    TEXT,            -- JSON array of Read v2 code strings
            schema_version INTEGER NOT NULL DEFAULT 3
        );

        CREATE TABLE IF NOT EXISTS concept_isa (
            child_id  TEXT NOT NULL,
            parent_id TEXT NOT NULL
        );

        -- Reverse-lookup table: code → SNOMED CT concept.
        -- terminology: 'ctv3' | 'read2'
        CREATE TABLE IF NOT EXISTS concept_maps (
            code        TEXT NOT NULL,
            terminology TEXT NOT NULL,
            concept_id  TEXT NOT NULL,
            PRIMARY KEY (code, terminology)
        );

        -- Simple refset membership. Each row asserts that a concept belongs to
        -- a refset. The refset itself is a concept — JOIN to `concepts` on
        -- refset_id to get its preferred term, module, and other metadata.
        CREATE TABLE IF NOT EXISTS refset_members (
            refset_id                TEXT NOT NULL,
            referenced_component_id  TEXT NOT NULL,
            PRIMARY KEY (refset_id, referenced_component_id)
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
    .context("creating schema")
}
