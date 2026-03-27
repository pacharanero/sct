//! `sct sqlite` — Load a SNOMED CT NDJSON artefact into a SQLite database with FTS5.
//!
//! Creates:
//!   - `concepts` table (all fields)
//!   - `concept_isa` table (child_id, parent_id) — indexed for fast children/ancestor queries
//!   - `concepts_fts` FTS5 virtual table over id, preferred_term, synonyms, fsn

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
              parents, children_count, attributes, active, module, effective_time, schema_version)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        )?;

        let mut insert_isa = tx.prepare(
            "INSERT INTO concept_isa (child_id, parent_id) VALUES (?1, ?2)",
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
                record.schema_version as i64,
            ])?;

            for parent in &record.parents {
                insert_isa.execute(params![record.id, parent.id])?;
            }

            n += 1;
            if n % 50_000 == 0 {
                pb.set_message(format!("{} concepts loaded...", n));
            }
        }

        drop(insert_concept);
        drop(insert_isa);
        tx.commit().context("committing transaction")?;
    }

    pb.set_message(format!("{} concepts committed; creating indexes...", n));

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_concepts_hierarchy ON concepts(hierarchy);
         CREATE INDEX IF NOT EXISTS idx_concept_isa_parent ON concept_isa(parent_id);
         CREATE INDEX IF NOT EXISTS idx_concept_isa_child  ON concept_isa(child_id);",
    )?;

    pb.set_message("Building FTS index...");
    conn.execute_batch("INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild')")?;

    pb.finish_with_message(format!(
        "Done. {} concepts → {}",
        n,
        args.output.display()
    ));
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
            schema_version INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE IF NOT EXISTS concept_isa (
            child_id  TEXT NOT NULL,
            parent_id TEXT NOT NULL
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
