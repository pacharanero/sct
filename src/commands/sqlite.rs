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
//!   - payload-refset tables (Complex Map, Extended Map, Attribute Value)
//!   - `concepts_fts` FTS5 virtual table over id, preferred_term, synonyms, fsn
//!   - `concept_ancestors` table (optional, --transitive-closure) - precomputed TCT

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::{params, Connection};
use std::io::BufRead;
use std::path::PathBuf;

use crate::humanize::plural_count;
use crate::provenance;
use crate::schema::{
    ConceptRecord, RefsetMemberRecord, RefsetSidecarProvenance, REFSET_SIDECAR_SCHEMA_VERSION,
    REFSET_SIDECAR_TYPE_TAG,
};

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
    ///
    /// Defaults to the input's name with a `.db` extension
    /// (`uk-monolith-42.ndjson` → `uk-monolith-42.db`), written to the working
    /// directory. Reading from stdin gives `snomed.db`.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub output: Option<PathBuf>,

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

#[derive(Default)]
struct PayloadRefsetCounts {
    complex_maps: usize,
    extended_maps: usize,
    attribute_values: usize,
}

impl PayloadRefsetCounts {
    fn total(&self) -> usize {
        self.complex_maps + self.extended_maps + self.attribute_values
    }
}

pub fn run(args: Args) -> Result<()> {
    let output = crate::commands::resolve_output(
        args.output.as_deref(),
        &args.input,
        crate::paths::suffix::DB,
    );

    let (reader, pb) = crate::progress::ndjson_reader(&args.input)?;

    pb.set_message(format!("Opening database {}...", output.display()));
    let mut conn = Connection::open(&output)
        .with_context(|| format!("opening database {}", output.display()))?;

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
    let mut fingerprint = provenance::ContentFingerprint::new();
    let payload_refset_counts;
    let history_n;
    {
        let tx = conn.transaction().context("beginning transaction")?;
        clear_derived_data(&tx)?;

        let mut insert_concept = tx.prepare(
            "INSERT OR REPLACE INTO concepts
             (id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
               parents, children_count, attributes, active, definition_status, module, effective_time,
               ctv3_codes, read2_codes, schema_version)
              VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
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
            "INSERT INTO crossmaps
             (source_system, source_code, target_system, target_code, map_refset,
              map_group, map_priority, map_source, active, metadata_json)
             VALUES (?1, ?2, 'snomed', ?3, 'rf2-simplemap', 1, 1,
                     'rf2_simple_map', 1, '{}')",
        )?;

        let mut insert_crossmap = tx.prepare(
            "INSERT INTO crossmaps
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
            fingerprint.update(line.as_bytes());

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
                record.definition_status,
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
                pb.set_message(format!("{} loaded...", plural_count(n as u64, "concept")));
            }
        }

        provenance::verify_or_set_content_fingerprint(
            &mut captured_provenance,
            fingerprint.finish(),
        )?;

        drop(insert_concept);
        drop(insert_isa);
        drop(insert_rel);
        drop(insert_map);
        drop(insert_simple_crossmap);
        drop(insert_refset_member);
        drop(insert_crossmap);
        payload_refset_counts =
            load_refset_sidecar(&tx, &args.input, captured_provenance.as_ref())?;
        history_n = load_history_sidecar(&tx, &args.input, captured_provenance.as_ref())?;

        pb.set_message("Creating indexes...");
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_concepts_hierarchy ON concepts(hierarchy);
             CREATE INDEX IF NOT EXISTS idx_concepts_schema_version ON concepts(schema_version);
             CREATE INDEX IF NOT EXISTS idx_concept_isa_parent ON concept_isa(parent_id);
             CREATE INDEX IF NOT EXISTS idx_concept_isa_child  ON concept_isa(child_id);
             CREATE INDEX IF NOT EXISTS idx_rel_source ON concept_relationships(source_id);
             CREATE INDEX IF NOT EXISTS idx_rel_type_dest ON concept_relationships(type_id, destination_id);
             CREATE INDEX IF NOT EXISTS idx_concept_maps_concept ON concept_maps(concept_id);
             CREATE INDEX IF NOT EXISTS idx_refset_members_by_concept
                 ON refset_members(referenced_component_id);
             CREATE INDEX IF NOT EXISTS idx_complex_map_refset_component
                 ON complex_map_refset_members(refset_id, referenced_component_id);
             CREATE INDEX IF NOT EXISTS idx_extended_map_refset_component
                 ON extended_map_refset_members(refset_id, referenced_component_id);
             CREATE INDEX IF NOT EXISTS idx_attribute_value_refset_component
                 ON attribute_value_refset_members(refset_id, referenced_component_id);
             CREATE INDEX IF NOT EXISTS idx_crossmaps_src ON crossmaps(source_system, source_code);
             CREATE INDEX IF NOT EXISTS idx_crossmaps_tgt ON crossmaps(target_system, target_code);
             CREATE INDEX IF NOT EXISTS idx_history_source ON concept_history(source_id);
             CREATE INDEX IF NOT EXISTS idx_history_target ON concept_history(target_id);",
        )?;

        pb.set_message("Building FTS index...");
        tx.execute_batch("INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild')")?;

        if let Some(ref provenance) = captured_provenance {
            provenance::write_sqlite(&tx, provenance)?;
        }
        tx.commit().context("committing transaction")?;
    }

    pb.finish_and_clear();

    if payload_refset_counts.total() > 0 {
        eprintln!(
            "Loaded {} payload refset rows ({} Complex Map, {} Extended Map, {} Attribute Value)",
            payload_refset_counts.total(),
            payload_refset_counts.complex_maps,
            payload_refset_counts.extended_maps,
            payload_refset_counts.attribute_values,
        );
    }
    if history_n > 0 {
        eprintln!(
            "Loaded {}",
            plural_count(history_n as u64, "concept-history row")
        );
    }

    if args.transitive_closure {
        crate::commands::tct::build(&mut conn, args.include_self)?;
    }

    eprintln!(
        "Done. {} → {}",
        plural_count(n as u64, "concept"),
        output.display()
    );

    Ok(())
}

/// Reset every table derived from the NDJSON input before rebuilding it.
/// Keeping this in the load transaction preserves the previous database if
/// parsing or fingerprint verification fails part-way through the input.
fn clear_derived_data(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS tct_invalidate_isa_insert;
         DROP TRIGGER IF EXISTS tct_invalidate_isa_update;
         DROP TRIGGER IF EXISTS tct_invalidate_isa_delete;
         DROP TRIGGER IF EXISTS tct_invalidate_concepts_insert;
         DROP TRIGGER IF EXISTS tct_invalidate_concepts_update;
         DROP TRIGGER IF EXISTS tct_invalidate_concepts_delete;
         DROP TRIGGER IF EXISTS tct_invalidate_ca_insert;
         DROP TRIGGER IF EXISTS tct_invalidate_ca_update;
         DROP TRIGGER IF EXISTS tct_invalidate_ca_delete;
         DELETE FROM concepts;
         DELETE FROM concept_isa;
         DELETE FROM concept_relationships;
          DELETE FROM concept_maps;
          DELETE FROM refset_members;
          DELETE FROM complex_map_refset_members;
          DELETE FROM extended_map_refset_members;
          DELETE FROM attribute_value_refset_members;
          DELETE FROM crossmaps;
         DELETE FROM concept_history;
         DELETE FROM metadata;
         DROP TABLE IF EXISTS concept_ancestors_meta;
         DROP TABLE IF EXISTS concept_ancestors;",
    )
    .context("clearing existing derived data")
}

fn load_refset_sidecar(
    conn: &Connection,
    input: &std::path::Path,
    source_provenance: Option<&provenance::Provenance>,
) -> Result<PayloadRefsetCounts> {
    let manifest = source_provenance
        .and_then(|source| source.companion(provenance::COMPANION_PAYLOAD_REFSETS));
    if input.as_os_str() == "-" {
        anyhow::ensure!(
            manifest.is_none(),
            "concept NDJSON declares a payload-refset companion; pass the NDJSON file path instead of stdin"
        );
        return Ok(PayloadRefsetCounts::default());
    }
    let sidecar = crate::commands::ndjson::refset_sidecar_path(input);
    if !sidecar.exists() {
        anyhow::ensure!(
            manifest.is_none(),
            "concept NDJSON declares a payload-refset companion but {} is missing",
            sidecar.display()
        );
        return Ok(PayloadRefsetCounts::default());
    }
    let manifest = manifest.context(
        "payload-refset sidecar exists but the concept NDJSON does not declare that companion",
    )?;

    let f = std::fs::File::open(&sidecar)
        .with_context(|| format!("opening refset sidecar {}", sidecar.display()))?;
    let mut lines = std::io::BufReader::new(f).lines();
    let header_line = loop {
        let line = lines
            .next()
            .context("refset sidecar is missing its provenance header")??;
        if !line.trim().is_empty() {
            break line;
        }
    };
    let header: RefsetSidecarProvenance = serde_json::from_str(&header_line)
        .with_context(|| format!("parsing refset provenance from {}", sidecar.display()))?;
    anyhow::ensure!(
        header.type_tag == REFSET_SIDECAR_TYPE_TAG,
        "unexpected refset sidecar record type {:?} in {}",
        header.type_tag,
        sidecar.display()
    );
    anyhow::ensure!(
        header.schema_version == REFSET_SIDECAR_SCHEMA_VERSION,
        "unsupported refset sidecar schema version {} in {}; expected {}",
        header.schema_version,
        sidecar.display(),
        REFSET_SIDECAR_SCHEMA_VERSION
    );
    anyhow::ensure!(
        manifest.schema_version == REFSET_SIDECAR_SCHEMA_VERSION
            && manifest.content_fingerprint == header.refset_fingerprint,
        "payload-refset companion manifest does not match {}",
        sidecar.display()
    );

    let source_provenance = source_provenance.context(
        "refset sidecar cannot be verified because the concept NDJSON has no provenance",
    )?;
    anyhow::ensure!(
        &header.source == source_provenance,
        "refset sidecar {} does not belong to the supplied concept NDJSON",
        sidecar.display()
    );

    let mut insert_complex = conn.prepare(
        "INSERT OR REPLACE INTO complex_map_refset_members
         (id, effective_time, active, module_id, refset_id, referenced_component_id,
          map_group, map_priority, map_rule, map_advice, map_target, correlation_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    let mut insert_extended = conn.prepare(
        "INSERT OR REPLACE INTO extended_map_refset_members
         (id, effective_time, active, module_id, refset_id, referenced_component_id,
          map_group, map_priority, map_rule, map_advice, map_target, correlation_id,
          map_category_id, map_block)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?;
    let mut insert_attribute = conn.prepare(
        "INSERT OR REPLACE INTO attribute_value_refset_members
         (id, effective_time, active, module_id, refset_id, referenced_component_id, value_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;

    let mut counts = PayloadRefsetCounts::default();
    let mut fingerprint = provenance::ContentFingerprint::new();
    for line in lines {
        let line = line.context("reading refset sidecar")?;
        if line.trim().is_empty() {
            continue;
        }
        fingerprint.update(line.as_bytes());
        let record: RefsetMemberRecord = serde_json::from_str(&line)
            .with_context(|| format!("parsing refset member from {}", sidecar.display()))?;
        match record {
            RefsetMemberRecord::ComplexMap(member) => {
                insert_complex.execute(params![
                    member.id,
                    member.effective_time,
                    member.active as i32,
                    member.module_id,
                    member.refset_id,
                    member.referenced_component_id,
                    member.map_group as i64,
                    member.map_priority as i64,
                    member.map_rule,
                    member.map_advice,
                    member.map_target,
                    member.correlation_id,
                ])?;
                counts.complex_maps += 1;
            }
            RefsetMemberRecord::ExtendedMap(member) => {
                insert_extended.execute(params![
                    member.id,
                    member.effective_time,
                    member.active as i32,
                    member.module_id,
                    member.refset_id,
                    member.referenced_component_id,
                    member.map_group as i64,
                    member.map_priority as i64,
                    member.map_rule,
                    member.map_advice,
                    member.map_target,
                    member.correlation_id,
                    member.map_category_id,
                    member.map_block.map(i64::from),
                ])?;
                counts.extended_maps += 1;
            }
            RefsetMemberRecord::AttributeValue(member) => {
                insert_attribute.execute(params![
                    member.id,
                    member.effective_time,
                    member.active as i32,
                    member.module_id,
                    member.refset_id,
                    member.referenced_component_id,
                    member.value_id,
                ])?;
                counts.attribute_values += 1;
            }
        }
    }
    let actual_fingerprint = fingerprint.finish();
    anyhow::ensure!(
        actual_fingerprint == header.refset_fingerprint,
        "refset sidecar fingerprint mismatch in {}: expected {}, calculated {}",
        sidecar.display(),
        header.refset_fingerprint,
        actual_fingerprint
    );
    anyhow::ensure!(
        counts.total() as u64 == manifest.record_count,
        "payload-refset companion row count mismatch in {}: expected {}, loaded {}",
        sidecar.display(),
        manifest.record_count,
        counts.total()
    );
    Ok(counts)
}

/// Load the optional `<stem>.history.ndjson` sidecar next to `input` into the
/// `concept_history` table. Returns the number of rows loaded (0 if absent).
fn load_history_sidecar(
    conn: &Connection,
    input: &std::path::Path,
    source_provenance: Option<&provenance::Provenance>,
) -> Result<usize> {
    let manifest =
        source_provenance.and_then(|source| source.companion(provenance::COMPANION_HISTORY));
    if input.as_os_str() == "-" {
        anyhow::ensure!(
            manifest.is_none(),
            "concept NDJSON declares a history companion; pass the NDJSON file path instead of stdin"
        );
        return Ok(0);
    }
    let sidecar = crate::commands::ndjson::history_sidecar_path(input);
    if !sidecar.exists() {
        anyhow::ensure!(
            manifest.is_none(),
            "concept NDJSON declares a history companion but {} is missing",
            sidecar.display()
        );
        return Ok(0);
    }
    let f = std::fs::File::open(&sidecar)
        .with_context(|| format!("opening history sidecar {}", sidecar.display()))?;
    let reader = std::io::BufReader::new(f);
    let mut n = 0usize;
    let mut fingerprint = provenance::ContentFingerprint::new();
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO concept_history (source_id, association, target_id)
         VALUES (?1, ?2, ?3)",
    )?;
    for line in reader.lines() {
        let line = line.context("reading history sidecar")?;
        if line.trim().is_empty() {
            continue;
        }
        fingerprint.update(line.as_bytes());
        let rec: crate::schema::HistoryRecord =
            serde_json::from_str(&line).context("parsing history record")?;
        stmt.execute(params![rec.source, rec.association, rec.target])?;
        n += 1;
    }
    if let Some(manifest) = manifest {
        anyhow::ensure!(
            manifest.schema_version == crate::schema::HISTORY_SIDECAR_SCHEMA_VERSION,
            "unsupported history companion schema version {}",
            manifest.schema_version
        );
        let actual_fingerprint = fingerprint.finish();
        anyhow::ensure!(
            actual_fingerprint == manifest.content_fingerprint,
            "history companion fingerprint mismatch in {}: expected {}, calculated {}",
            sidecar.display(),
            manifest.content_fingerprint,
            actual_fingerprint
        );
        anyhow::ensure!(
            n as u64 == manifest.record_count,
            "history companion row count mismatch in {}: expected {}, loaded {}",
            sidecar.display(),
            manifest.record_count,
            n
        );
    }
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
            definition_status TEXT,          -- primitive / fully-defined RF2 SCTID
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

        -- Full RF2 Complex Map members. Kept separate from `refset_members`,
        -- whose two-column contract remains concept-set membership only.
        CREATE TABLE IF NOT EXISTS complex_map_refset_members (
            id                       TEXT PRIMARY KEY,
            effective_time           TEXT NOT NULL,
            active                   INTEGER NOT NULL,
            module_id                TEXT NOT NULL,
            refset_id                TEXT NOT NULL,
            referenced_component_id  TEXT NOT NULL,
            map_group                INTEGER NOT NULL,
            map_priority             INTEGER NOT NULL,
            map_rule                 TEXT NOT NULL,
            map_advice               TEXT NOT NULL,
            map_target               TEXT NOT NULL,
            correlation_id           TEXT NOT NULL
        );

        -- Full RF2 Extended Map members, including inactive, null-map and
        -- unclassified rows. `crossmaps` remains the active known-system query
        -- projection; this table is the lossless source record.
        CREATE TABLE IF NOT EXISTS extended_map_refset_members (
            id                       TEXT PRIMARY KEY,
            effective_time           TEXT NOT NULL,
            active                   INTEGER NOT NULL,
            module_id                TEXT NOT NULL,
            refset_id                TEXT NOT NULL,
            referenced_component_id  TEXT NOT NULL,
            map_group                INTEGER NOT NULL,
            map_priority             INTEGER NOT NULL,
            map_rule                 TEXT NOT NULL,
            map_advice               TEXT NOT NULL,
            map_target               TEXT NOT NULL,
            correlation_id           TEXT NOT NULL,
            map_category_id          TEXT,
            map_block                INTEGER
        );

        -- Full RF2 Attribute Value members. Concept inactivation indicators
        -- live here and are consumed by the later coherent history surface.
        CREATE TABLE IF NOT EXISTS attribute_value_refset_members (
            id                       TEXT PRIMARY KEY,
            effective_time           TEXT NOT NULL,
            active                   INTEGER NOT NULL,
            module_id                TEXT NOT NULL,
            refset_id                TEXT NOT NULL,
            referenced_component_id  TEXT NOT NULL,
            value_id                 TEXT NOT NULL
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

    super::ensure_crossmaps_schema(conn).context("migrating crossmaps schema")
}
