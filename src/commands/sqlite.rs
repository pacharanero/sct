// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct sqlite` - Load a SNOMED CT NDJSON artefact into a SQLite database with FTS5.
//!
//! Creates:
//!   - `concepts` table (all fields)
//!   - `concept_isa` table (child_id, parent_id) - indexed for fast children/ancestor queries
//!   - `concept_relationships` table (source, type, destination, group) - typed attributes for ECL
//!   - `concept_maps` table (legacy code → concept reverse lookup for CTV3 / Read v2)
//!   - `crossmaps` table (general source-system/code → target-system/code maps)
//!   - `refset_members` table (refset_id → concept_id) - refset membership
//!   - `concepts_fts` FTS5 virtual table over id, preferred_term, synonyms, fsn
//!   - `concept_ancestors` table (optional, --transitive-closure) - precomputed TCT

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::{params, Connection};
use std::io::BufRead;
use std::path::PathBuf;

use crate::provenance;
use crate::schema::ConceptRecord;

#[derive(Parser, Debug)]
pub struct Args {
    /// NDJSON artefact produced by `sct ndjson`. Use `-` for stdin.
    #[arg(
        long = "ndjson",
        alias = "input",
        short = 'i',
        value_hint = clap::ValueHint::FilePath,
        value_name = "NDJSON",
        value_parser = crate::paths::tilde_pathbuf
    )]
    pub input: PathBuf,

    /// Output SQLite database file.
    #[arg(long, short, default_value = "snomed.db", value_parser = crate::paths::tilde_pathbuf)]
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
    let (reader, pb) = crate::progress::ndjson_reader(&args.input)?;

    pb.set_message(format!("Opening database {}...", args.output.display()));
    let mut conn = Connection::open(&args.output)
        .with_context(|| format!("opening database {}", args.output.display()))?;

    // Performance pragmas - safe for a build-time operation
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA cache_size = -65536;
         PRAGMA temp_store = MEMORY;",
    )?;

    create_schema(&conn)?;

    pb.set_message("Loading concepts...");

    let mut n = 0usize;
    let mut captured_provenance: Option<provenance::Provenance> = None;
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

        let mut insert_rel = tx.prepare(
            "INSERT INTO concept_relationships (source_id, type_id, destination_id, group_num)
             VALUES (?1, ?2, ?3, ?4)",
        )?;

        let mut insert_map = tx.prepare(
            "INSERT OR IGNORE INTO concept_maps (code, terminology, concept_id) VALUES (?1, ?2, ?3)",
        )?;

        let mut insert_refset_member = tx.prepare(
            "INSERT OR IGNORE INTO refset_members (refset_id, referenced_component_id) VALUES (?1, ?2)",
        )?;

        let mut insert_simple_crossmap = tx.prepare(
            "INSERT OR IGNORE INTO crossmaps
             (source_system, source_code, target_system, target_code, map_refset,
              map_group, map_priority, map_source, active, metadata_json)
             VALUES (?1, ?2, 'snomed', ?3, 'rf2-simplemap', 1, 1,
                     'rf2_simple_map', 1, '{}')",
        )?;

        let mut insert_crossmap = tx.prepare(
            "INSERT OR IGNORE INTO crossmaps
             (source_system, source_code, target_system, target_code, map_refset,
              map_group, map_priority, map_rule, map_advice, correlation,
              map_source, active, metadata_json)
             VALUES ('snomed', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     'rf2_extended_map', 1, '{}')",
        )?;

        for line in reader.lines() {
            let line = line.context("reading input")?;
            if line.trim().is_empty() {
                continue;
            }

            // Provenance header line (if present) - capture and skip.
            if let Some(p) = provenance::try_parse_ndjson_line(&line) {
                captured_provenance = Some(p);
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

            for rel in &record.relationships {
                insert_rel.execute(params![
                    record.id,
                    rel.type_id,
                    rel.destination_id,
                    rel.group as i64,
                ])?;
            }

            for code in &record.ctv3_codes {
                insert_map.execute(params![code, "ctv3", record.id])?;
                insert_simple_crossmap.execute(params!["ctv3", code, record.id])?;
            }
            for code in &record.read2_codes {
                insert_map.execute(params![code, "read2", record.id])?;
                insert_simple_crossmap.execute(params!["read2", code, record.id])?;
            }

            for refset_id in &record.refsets {
                insert_refset_member.execute(params![refset_id, record.id])?;
            }

            for m in &record.crossmaps {
                insert_crossmap.execute(params![
                    record.id,
                    m.system,
                    m.code,
                    m.refset,
                    m.group as i64,
                    m.priority as i64,
                    m.rule,
                    m.advice,
                    m.correlation,
                ])?;
            }

            n += 1;
            if n.is_multiple_of(50_000) {
                pb.set_message(format!("{} concepts loaded...", n));
            }
        }

        drop(insert_concept);
        drop(insert_isa);
        drop(insert_rel);
        drop(insert_map);
        drop(insert_simple_crossmap);
        drop(insert_refset_member);
        drop(insert_crossmap);
        tx.commit().context("committing transaction")?;
    }

    // The load byte bar is now at 100%; retire it and switch to a spinner for
    // the index + FTS rebuild, which are single opaque SQL statements with no
    // knowable total. The spinner's steady tick keeps animating (and the
    // elapsed clock advancing) through the blocking FTS rebuild, so the build
    // never looks hung.
    pb.finish_and_clear();
    let pb = crate::progress::spinner(format!("{} concepts committed; creating indexes...", n));

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_concepts_hierarchy ON concepts(hierarchy);
         CREATE INDEX IF NOT EXISTS idx_concept_isa_parent ON concept_isa(parent_id);
         CREATE INDEX IF NOT EXISTS idx_concept_isa_child  ON concept_isa(child_id);
         CREATE INDEX IF NOT EXISTS idx_rel_source ON concept_relationships(source_id);
         CREATE INDEX IF NOT EXISTS idx_rel_type_dest ON concept_relationships(type_id, destination_id);
         CREATE INDEX IF NOT EXISTS idx_concept_maps_concept ON concept_maps(concept_id);
         CREATE INDEX IF NOT EXISTS idx_refset_members_by_concept
             ON refset_members(referenced_component_id);
         CREATE INDEX IF NOT EXISTS idx_crossmaps_src ON crossmaps(source_system, source_code);
         CREATE INDEX IF NOT EXISTS idx_crossmaps_tgt ON crossmaps(target_system, target_code);
         CREATE INDEX IF NOT EXISTS idx_history_source ON concept_history(source_id);
         CREATE INDEX IF NOT EXISTS idx_history_target ON concept_history(target_id);",
    )?;

    pb.set_message("Building FTS index...");
    conn.execute_batch("INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild')")?;

    // Persist provenance (if the NDJSON had a header line). Older v3 NDJSONs
    // without a header leave the metadata table empty - downstream commands
    // treat that as "provenance not recorded" and degrade gracefully.
    provenance::create_sqlite_table(&conn)?;
    if let Some(ref p) = captured_provenance {
        provenance::write_sqlite(&conn, p)?;
    }

    // --- Concept history sidecar (`<input-stem>.history.ndjson`, if present) ---
    let history_n = load_history_sidecar(&conn, &args.input)?;
    if history_n > 0 {
        pb.println(format!("Loaded {history_n} concept-history rows"));
    }

    pb.finish_with_message(format!("Done. {} concepts → {}", n, args.output.display()));

    if args.transitive_closure {
        crate::commands::tct::build(&mut conn, args.include_self)?;
    }

    Ok(())
}

/// Load the optional `<stem>.history.ndjson` sidecar next to `input` into the
/// `concept_history` table. Returns the number of rows loaded (0 if absent).
fn load_history_sidecar(conn: &Connection, input: &std::path::Path) -> Result<usize> {
    let sidecar = crate::commands::ndjson::history_sidecar_path(input);
    if !sidecar.exists() {
        return Ok(0);
    }
    let f = std::fs::File::open(&sidecar)
        .with_context(|| format!("opening history sidecar {}", sidecar.display()))?;
    let reader = std::io::BufReader::new(f);
    let tx = conn.unchecked_transaction()?;
    let mut n = 0usize;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO concept_history (source_id, association, target_id)
             VALUES (?1, ?2, ?3)",
        )?;
        for line in reader.lines() {
            let line = line.context("reading history sidecar")?;
            if line.trim().is_empty() {
                continue;
            }
            let rec: crate::schema::HistoryRecord =
                serde_json::from_str(&line).context("parsing history record")?;
            stmt.execute(params![rec.source, rec.association, rec.target])?;
            n += 1;
        }
    }
    tx.commit()?;
    Ok(n)
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

        -- Typed attribute relationships (non-IS-A), preserving the attribute
        -- type SCTID and relationship group. Backs ECL attribute refinement
        -- (`<<X : type = value`). See spec/ecl.md §4.
        CREATE TABLE IF NOT EXISTS concept_relationships (
            source_id      TEXT NOT NULL,
            type_id        TEXT NOT NULL,
            destination_id TEXT NOT NULL,
            group_num      INTEGER NOT NULL
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
        -- a refset. The refset itself is a concept - JOIN to `concepts` on
        -- refset_id to get its preferred term, module, and other metadata.
        CREATE TABLE IF NOT EXISTS refset_members (
            refset_id                TEXT NOT NULL,
            referenced_component_id  TEXT NOT NULL,
            PRIMARY KEY (refset_id, referenced_component_id)
        );

        -- General cross-terminology map table. RF2 SimpleMap rows are stored as
        -- external source -> SNOMED CT target (for CTV3 / Read v2), while RF2
        -- ExtendedMap rows are stored as SNOMED CT source -> external target
        -- (for ICD-10 / OPCS-4). Source-specific fields that matter for query
        -- semantics are promoted to nullable columns; metadata_json is only an
        -- escape hatch for low-use provenance details.
        CREATE TABLE IF NOT EXISTS crossmaps (
            source_system  TEXT NOT NULL,
            source_code    TEXT NOT NULL,
            source_term_code TEXT,
            target_system  TEXT NOT NULL,
            target_code    TEXT NOT NULL,
            target_description_id TEXT,
            map_refset     TEXT NOT NULL,
            map_source     TEXT NOT NULL DEFAULT 'rf2',
            map_id         TEXT,
            effective_date TEXT,
            active         INTEGER NOT NULL DEFAULT 1,
            map_status     TEXT,
            map_group      INTEGER,
            map_priority   INTEGER,
            map_rule       TEXT,
            map_advice     TEXT,
            correlation    TEXT,
            is_assured     INTEGER,
            metadata_json  TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (source_system, source_code, target_system, target_code, map_refset, map_group)
        );

        -- Concept history: maps an inactivated concept to its replacement(s),
        -- from the RF2 Association refsets (loaded with `--refsets all`, via the
        -- `<stem>.history.ndjson` sidecar). Lets old records referencing retired
        -- SCTIDs be forwarded. `source_id` is usually inactive and absent from
        -- `concepts`. See spec/cross-terminology-mapping.md.
        CREATE TABLE IF NOT EXISTS concept_history (
            source_id    TEXT NOT NULL,   -- the inactivated concept
            association  TEXT NOT NULL,   -- 'replaced_by' | 'same_as' | ...
            target_id    TEXT NOT NULL,   -- the replacement / related concept
            PRIMARY KEY (source_id, association, target_id)
        );

        -- Release provenance as a flat key/value store. Written once at
        -- `sct sqlite` time and read by every downstream query command.
        CREATE TABLE IF NOT EXISTS metadata (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
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
    .context("creating schema")?;

    ensure_crossmap_columns(conn).context("migrating crossmaps schema")
}

fn ensure_crossmap_columns(conn: &Connection) -> Result<()> {
    let columns = [
        ("source_term_code", "TEXT"),
        ("target_description_id", "TEXT"),
        ("map_source", "TEXT NOT NULL DEFAULT 'rf2'"),
        ("map_id", "TEXT"),
        ("effective_date", "TEXT"),
        ("active", "INTEGER NOT NULL DEFAULT 1"),
        ("map_status", "TEXT"),
        ("is_assured", "INTEGER"),
        ("metadata_json", "TEXT NOT NULL DEFAULT '{}'"),
    ];

    for (name, definition) in columns {
        if !column_exists(conn, "crossmaps", name)? {
            conn.execute(
                &format!("ALTER TABLE crossmaps ADD COLUMN {name} {definition}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}
