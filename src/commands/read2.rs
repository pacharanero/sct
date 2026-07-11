// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct read2` - import final NHS Data Migration Read v2 maps into SQLite.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use csv::StringRecord;
use indicatif::{ProgressBar, ProgressStyle};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAP_SOURCE: &str = "nhs_data_migration_item9";
const MAP_REFSET: &str = "nhs-data-migration-rcsctmap2";

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: Read2Command,
}

#[derive(Subcommand, Debug)]
pub enum Read2Command {
    /// Import the final TRUD item 9 Read v2 -> SNOMED CT maps into SQLite.
    Import(ImportArgs),
}

#[derive(Parser, Debug)]
pub struct ImportArgs {
    /// NHS Data Migration item 9 archive, e.g. nhs_datamigration_29.0.0_20200401000001.zip.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub archive: PathBuf,

    /// SNOMED CT SQLite database to update. Discovered via path resolution when omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct Read2MapRow {
    map_id: String,
    read_code: String,
    term_code: String,
    concept_id: String,
    description_id: Option<String>,
    is_assured: bool,
    effective_date: String,
    map_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSummary {
    pub latest_rows: usize,
    pub active_rows: usize,
    pub assured_rows: usize,
    pub unassured_rows: usize,
    pub distinct_source_keys: usize,
    pub distinct_target_concepts: usize,
    pub missing_target_concepts: usize,
}

pub fn run(args: Args) -> Result<()> {
    match args.subcommand {
        Read2Command::Import(a) => {
            let db = crate::paths::resolve_db(a.db.as_deref())?.path;
            let summary = import_archive(&db, &a.archive)?;
            print_summary(&db, &a.archive, &summary);
            Ok(())
        }
    }
}

pub fn import_archive(db: &Path, archive: &Path) -> Result<ImportSummary> {
    let mut conn =
        Connection::open(db).with_context(|| format!("opening database {}", db.display()))?;
    import_archive_conn(&mut conn, archive)
}

pub fn import_archive_conn(conn: &mut Connection, archive: &Path) -> Result<ImportSummary> {
    ensure_target_database(conn)?;
    ensure_map_schema(conn)?;

    let rows = read_primary_map(archive)?;
    let latest = latest_by_map_id(rows);
    let mut rows: Vec<_> = latest.into_values().collect();
    rows.sort_by(|a, b| {
        (
            &a.read_code,
            &a.term_code,
            &a.concept_id,
            &a.description_id,
            &a.map_id,
        )
            .cmp(&(
                &b.read_code,
                &b.term_code,
                &b.concept_id,
                &b.description_id,
                &b.map_id,
            ))
    });

    let latest_rows = rows.len();
    let active_rows = rows.iter().filter(|r| r.active()).count();
    let assured_rows = rows.iter().filter(|r| r.active() && r.is_assured).count();
    let unassured_rows = rows.iter().filter(|r| r.active() && !r.is_assured).count();

    let pb = ProgressBar::new(rows.len() as u64);
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} [{elapsed_precise}] {pos}/{len} Read v2 maps imported")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(120));

    let tx = conn
        .transaction()
        .context("beginning Read v2 import transaction")?;
    tx.execute(
        "DELETE FROM crossmaps WHERE source_system = 'read2' AND map_source = ?1",
        [MAP_SOURCE],
    )?;
    tx.execute("DELETE FROM concept_maps WHERE terminology = 'read2'", [])?;

    let mut insert_crossmap = tx.prepare(
        "INSERT OR REPLACE INTO crossmaps
         (source_system, source_code, source_term_code,
          target_system, target_code, target_description_id,
          map_refset, map_source, map_id, effective_date, active, map_status,
          map_group, map_priority, is_assured, metadata_json)
         VALUES
         ('read2', ?1, ?2, 'snomed', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
          ?11, 1, ?12, ?13)",
    )?;
    let mut insert_legacy = tx.prepare(
        "INSERT OR IGNORE INTO concept_maps (code, terminology, concept_id)
         VALUES (?1, 'read2', ?2)",
    )?;

    let mut occurrence: HashMap<(String, String), i64> = HashMap::new();
    let mut distinct_sources = std::collections::HashSet::new();
    let mut distinct_targets = std::collections::HashSet::new();
    let mut missing_targets = std::collections::HashSet::new();

    for row in &rows {
        let source_code = row.source_code();
        let active = row.active();
        if active {
            distinct_sources.insert(source_code.clone());
            distinct_targets.insert(row.concept_id.clone());
            if !concept_exists(&tx, &row.concept_id)? {
                missing_targets.insert(row.concept_id.clone());
            }
        }

        let key = (source_code.clone(), row.concept_id.clone());
        let next = occurrence.entry(key).or_insert(0);
        *next += 1;

        let metadata = serde_json::json!({
            "read_code": row.read_code,
            "term_code": row.term_code,
            "archive": archive.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        })
        .to_string();

        insert_crossmap.execute(params![
            source_code,
            row.term_code,
            row.concept_id,
            row.description_id.as_deref(),
            MAP_REFSET,
            MAP_SOURCE,
            row.map_id,
            row.effective_date,
            active as i32,
            row.map_status,
            *next,
            row.is_assured as i32,
            metadata,
        ])?;

        if active {
            insert_legacy.execute(params![row.source_code(), row.concept_id])?;
        }
        pb.inc(1);
    }

    drop(insert_crossmap);
    drop(insert_legacy);
    tx.commit().context("committing Read v2 import")?;
    pb.finish_and_clear();

    Ok(ImportSummary {
        latest_rows,
        active_rows,
        assured_rows,
        unassured_rows,
        distinct_source_keys: distinct_sources.len(),
        distinct_target_concepts: distinct_targets.len(),
        missing_target_concepts: missing_targets.len(),
    })
}

fn print_summary(db: &Path, archive: &Path, s: &ImportSummary) {
    eprintln!("Imported Read v2 maps from {}", archive.display());
    eprintln!("  database: {}", db.display());
    eprintln!("  latest MapId rows: {}", s.latest_rows);
    eprintln!("  active latest rows: {}", s.active_rows);
    eprintln!("  active assured rows: {}", s.assured_rows);
    eprintln!("  active unassured rows: {}", s.unassured_rows);
    eprintln!(
        "  distinct ReadCode+TermCode keys: {}",
        s.distinct_source_keys
    );
    eprintln!(
        "  distinct target SNOMED concepts: {}",
        s.distinct_target_concepts
    );
    if s.missing_target_concepts > 0 {
        eprintln!(
            "  target concepts absent from this SNOMED DB: {}",
            s.missing_target_concepts
        );
    }
}

fn read_primary_map(archive: &Path) -> Result<Vec<Read2MapRow>> {
    let file =
        std::fs::File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("reading zip archive {}", archive.display()))?;

    let entry_index = find_primary_map_entry(&mut zip)?;
    let mut entry = zip.by_index(entry_index)?;
    let mut data = Vec::new();
    entry
        .read_to_end(&mut data)
        .with_context(|| format!("reading {}", entry.name()))?;

    parse_primary_map(&data[..])
}

fn find_primary_map_entry<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
) -> Result<usize> {
    let mut fallback = None;
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/").to_lowercase();
        if !name.ends_with(".txt") || !name.contains("rcsctmap2") {
            continue;
        }
        if name.contains("mapping tables/updated/clinically assured/") {
            return Ok(i);
        }
        fallback.get_or_insert(i);
    }
    fallback.ok_or_else(|| {
        anyhow::anyhow!(
            "could not find primary Read v2 map file rcsctmap2_*.txt in {}",
            "TRUD item 9 archive"
        )
    })
}

fn parse_primary_map<R: std::io::Read>(reader: R) -> Result<Vec<Read2MapRow>> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(reader);
    let headers = rdr.headers().context("reading Read v2 map header")?.clone();
    let indexes = HeaderIndexes::from_headers(&headers)?;
    let mut rows = Vec::new();
    for result in rdr.records() {
        let record = result.context("reading Read v2 map row")?;
        rows.push(indexes.row(&record)?);
    }
    Ok(rows)
}

struct HeaderIndexes {
    map_id: usize,
    read_code: usize,
    term_code: usize,
    concept_id: usize,
    description_id: usize,
    is_assured: usize,
    effective_date: usize,
    map_status: usize,
}

impl HeaderIndexes {
    fn from_headers(headers: &StringRecord) -> Result<Self> {
        let idx = |name: &str| {
            headers
                .iter()
                .position(|h| h.eq_ignore_ascii_case(name))
                .ok_or_else(|| anyhow::anyhow!("Read v2 map is missing required column {name}"))
        };
        Ok(Self {
            map_id: idx("MapId")?,
            read_code: idx("ReadCode")?,
            term_code: idx("TermCode")?,
            concept_id: idx("ConceptId")?,
            description_id: idx("DescriptionId")?,
            is_assured: idx("IS_ASSURED")?,
            effective_date: idx("EffectiveDate")?,
            map_status: idx("MapStatus")?,
        })
    }

    fn row(&self, r: &StringRecord) -> Result<Read2MapRow> {
        let get = |i: usize, name: &str| {
            r.get(i)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Read v2 map row has empty {name}"))
                .map(str::to_string)
        };
        let get_optional = |i: usize| {
            r.get(i)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let assured = get(self.is_assured, "IS_ASSURED")?;
        Ok(Read2MapRow {
            map_id: get(self.map_id, "MapId")?,
            read_code: get(self.read_code, "ReadCode")?,
            term_code: get(self.term_code, "TermCode")?,
            concept_id: get(self.concept_id, "ConceptId")?,
            description_id: get_optional(self.description_id),
            is_assured: matches!(assured.as_str(), "1" | "true" | "TRUE"),
            effective_date: get(self.effective_date, "EffectiveDate")?,
            map_status: get(self.map_status, "MapStatus")?,
        })
    }
}

fn latest_by_map_id(rows: Vec<Read2MapRow>) -> HashMap<String, Read2MapRow> {
    let mut latest = HashMap::new();
    for row in rows {
        latest
            .entry(row.map_id.clone())
            .and_modify(|existing: &mut Read2MapRow| {
                if row.effective_date >= existing.effective_date {
                    *existing = row.clone();
                }
            })
            .or_insert(row);
    }
    latest
}

impl Read2MapRow {
    fn source_code(&self) -> String {
        format!("{}{}", self.read_code, self.term_code)
    }

    fn active(&self) -> bool {
        self.map_status
            .parse::<i64>()
            .map(|s| s > 0)
            .unwrap_or(false)
    }
}

fn ensure_target_database(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "concepts")? {
        bail!("target database does not look like an sct SQLite database: missing concepts table");
    }
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get(0))?;
    if count == 0 {
        bail!("target database has an empty concepts table; build SNOMED first");
    }
    Ok(())
}

fn ensure_map_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS concept_maps (
            code        TEXT NOT NULL,
            terminology TEXT NOT NULL,
            concept_id  TEXT NOT NULL,
            PRIMARY KEY (code, terminology)
        );

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
        );",
    )
    .context("creating Read v2 map schema")?;

    ensure_crossmap_columns(conn)?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_concept_maps_concept ON concept_maps(concept_id);
         CREATE INDEX IF NOT EXISTS idx_crossmaps_src ON crossmaps(source_system, source_code);
         CREATE INDEX IF NOT EXISTS idx_crossmaps_tgt ON crossmaps(target_system, target_code);",
    )?;
    Ok(())
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
        ("map_group", "INTEGER"),
        ("map_priority", "INTEGER"),
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

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?;
    Ok(exists != 0)
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

fn concept_exists(conn: &Connection, concept_id: &str) -> Result<bool> {
    let exists: Option<i64> = conn
        .query_row("SELECT 1 FROM concepts WHERE id = ?1", [concept_id], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(exists.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::io::Write;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (
                id TEXT PRIMARY KEY,
                preferred_term TEXT NOT NULL
            );
             INSERT INTO concepts (id, preferred_term) VALUES
                ('22298006', 'Myocardial infarction'),
                ('195967001', 'Asthma');",
        )
        .unwrap();
        conn
    }

    fn item9_zip() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("nhs_datamigration_29.0.0_20200401000001.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file(
            "Mapping Tables/Updated/Clinically Assured/rcsctmap2_uk_20200401000001.txt",
            opts,
        )
        .unwrap();
        write!(
            zip,
            "MapId\tReadCode\tTermCode\tConceptId\tDescriptionId\tIS_ASSURED\tEffectiveDate\tMapStatus\r\n\
             m1\t0111.\t00\t22298006\t1001\t1\t20190101\t1\r\n\
             m1\t0111.\t00\t195967001\t1002\t1\t20200101\t0\r\n\
             m2\tH33..\t11\t195967001\t1003\t0\t20200101\t1\r\n\
             m3\tH33..\t11\t195967001\t1004\t1\t20200101\t1\r\n"
        )
        .unwrap();
        zip.finish().unwrap();
        (dir, path)
    }

    #[test]
    fn imports_latest_rows_and_preserves_metadata() {
        let mut conn = db();
        let (_dir, archive) = item9_zip();
        let summary = import_archive_conn(&mut conn, &archive).unwrap();

        assert_eq!(summary.latest_rows, 3);
        assert_eq!(summary.active_rows, 2);
        assert_eq!(summary.assured_rows, 1);
        assert_eq!(summary.unassured_rows, 1);
        assert_eq!(summary.distinct_source_keys, 1);
        assert_eq!(summary.distinct_target_concepts, 1);

        let inactive: i64 = conn
            .query_row(
                "SELECT active FROM crossmaps WHERE source_code='0111.00'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(inactive, 0);

        let active: Vec<(String, String, i64)> = conn
            .prepare(
                "SELECT source_code, target_description_id, is_assured
                 FROM crossmaps
                 WHERE source_code='H33..11' AND active=1
                 ORDER BY target_description_id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            active,
            vec![
                ("H33..11".into(), "1003".into(), 0),
                ("H33..11".into(), "1004".into(), 1),
            ]
        );

        let legacy: String = conn
            .query_row(
                "SELECT concept_id FROM concept_maps WHERE code='H33..11' AND terminology='read2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(legacy, "195967001");
    }
}
