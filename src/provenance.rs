// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Release provenance metadata for sct-produced artefacts.
//!
//! Every artefact (`NDJSON`, SQLite DB, Parquet, Markdown export, Arrow embeddings)
//! should be able to answer: **which SNOMED CT release did this come from?** -
//! edition (International / UK Clinical / UK Drug / Monolith / extension),
//! release date, the full release identifier, and the sct version that built it.
//!
//! Storage:
//!   - **NDJSON**: first line is a JSON object tagged `"_type": "sct_provenance"`.
//!     Older v3 artefacts without a header still parse (the line-parse helper
//!     returns `None` and consumers fall through to the ConceptRecord path).
//!   - **SQLite**: a `metadata` key/value table (`key TEXT PRIMARY KEY`).
//!   - Consumers read it and may surface it as a footer on query output.
//!
//! The struct is intentionally a plain bag of strings rather than a tight
//! schema: releases we haven't seen yet (future UK extensions, national
//! editions, custom local extensions) should round-trip through unchanged.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Discriminator for NDJSON metadata lines. See `try_parse_ndjson_line`.
pub const NDJSON_TYPE_TAG: &str = "sct_provenance";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    /// Sentinel used to distinguish provenance records from concept records
    /// when they share an NDJSON file.
    #[serde(rename = "_type", default = "default_type_tag")]
    pub type_tag: String,

    /// Human-readable edition label: "International", "UK Clinical",
    /// "UK Drug", "UK Monolith", or the original directory name if
    /// we can't classify it.
    pub edition_label: String,

    /// YYYY-MM-DD release date, extracted from the release identifier
    /// (the 8-digit run in the directory name) or left empty.
    pub release_date: String,

    /// Full release identifier as supplied by SNOMED International / NHS TRUD,
    /// e.g. `SnomedCT_InternationalRF2_PRODUCTION_20260301T120000Z`.
    pub release_id: String,

    /// The original paths the user passed to `sct ndjson --rf2 …`. When more
    /// than one is present, the artefact is a composite (e.g. base +
    /// extension). The first entry is treated as the primary edition.
    pub source_paths: Vec<String>,

    /// sct version that produced the artefact (`CARGO_PKG_VERSION`).
    pub sct_version: String,

    /// RFC-3339 timestamp of when the artefact was produced.
    pub created_at: String,
}

fn default_type_tag() -> String {
    NDJSON_TYPE_TAG.to_string()
}

impl Provenance {
    /// Derive a provenance record from one or more RF2 input paths.
    ///
    /// The first path is treated as primary. Edition classification is best-effort
    /// via directory-name heuristics; unknown layouts fall back to the dir name.
    pub fn from_rf2_paths(paths: &[std::path::PathBuf]) -> Self {
        let primary = paths.first().map(|p| p.as_path());
        let release_id = primary.map(release_id_from_path).unwrap_or_default();
        let edition_label = classify_edition(&release_id);
        let release_date = extract_release_date(&release_id).unwrap_or_default();

        Self {
            type_tag: NDJSON_TYPE_TAG.to_string(),
            edition_label,
            release_date,
            release_id,
            source_paths: paths.iter().map(|p| p.display().to_string()).collect(),
            sct_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Render a compact, human-readable one-or-two-line footer for TTY output.
    pub fn human_footer(&self) -> String {
        let mut s = String::new();
        s.push_str("─ Provenance ─\n");
        if !self.edition_label.is_empty() {
            s.push_str(&format!("  Edition:      {}\n", self.edition_label));
        }
        if !self.release_date.is_empty() {
            s.push_str(&format!("  Release date: {}\n", self.release_date));
        }
        if !self.release_id.is_empty() {
            s.push_str(&format!("  Release id:   {}\n", self.release_id));
        }
        if !self.sct_version.is_empty() {
            s.push_str(&format!("  Built by:     sct {}\n", self.sct_version));
        }
        s
    }

    /// JSON shape for embedding inside another JSON response under
    /// a `_provenance` key. Identical to the NDJSON representation
    /// minus the `_type` discriminator.
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "edition_label": self.edition_label,
            "release_date": self.release_date,
            "release_id": self.release_id,
            "source_paths": self.source_paths,
            "sct_version": self.sct_version,
            "created_at": self.created_at,
        })
    }
}

/// Serialise this provenance into a `HashMap<String,String>` suitable for
/// embedding in an Arrow schema's metadata slot. Round-trips through
/// `from_arrow_metadata` losslessly.
pub fn to_arrow_metadata(p: &Provenance) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    m.insert("sct.edition_label".into(), p.edition_label.clone());
    m.insert("sct.release_date".into(), p.release_date.clone());
    m.insert("sct.release_id".into(), p.release_id.clone());
    if let Ok(v) = serde_json::to_string(&p.source_paths) {
        m.insert("sct.source_paths".into(), v);
    }
    m.insert("sct.sct_version".into(), p.sct_version.clone());
    m.insert("sct.created_at".into(), p.created_at.clone());
    m
}

/// Reconstruct a `Provenance` from an Arrow schema metadata map. Returns
/// `None` if the map has no `sct.edition_label` key (i.e. the file was
/// produced before this provenance scheme existed).
pub fn from_arrow_metadata(m: &std::collections::HashMap<String, String>) -> Option<Provenance> {
    let edition_label = m.get("sct.edition_label")?.clone();
    let source_paths: Vec<String> = m
        .get("sct.source_paths")
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    Some(Provenance {
        type_tag: NDJSON_TYPE_TAG.to_string(),
        edition_label,
        release_date: m.get("sct.release_date").cloned().unwrap_or_default(),
        release_id: m.get("sct.release_id").cloned().unwrap_or_default(),
        source_paths,
        sct_version: m.get("sct.sct_version").cloned().unwrap_or_default(),
        created_at: m.get("sct.created_at").cloned().unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------
// Display flags + helpers for query commands
// ---------------------------------------------------------------------------

/// `--provenance` / `--no-provenance` flag pair, flattened into each query
/// command's Args struct so the user can override the per-format default.
#[derive(clap::Args, Debug, Clone, Copy, Default)]
pub struct ProvenanceFlags {
    /// Show release provenance (edition, release date) on this query's output.
    /// Default: on for TTY text output, off for JSON or piped output.
    #[arg(long, conflicts_with = "no_provenance")]
    pub provenance: bool,

    /// Suppress release provenance even in interactive output.
    #[arg(long)]
    pub no_provenance: bool,
}

/// Output mode for a query command - drives the default provenance behaviour
/// when neither `--provenance` nor `--no-provenance` is set.
#[derive(Debug, Clone, Copy)]
pub enum OutputMode {
    /// Plain human-readable text (the default for most query commands).
    HumanText,
    /// Structured JSON (the `--json` mode).
    Json,
}

/// Should this command surface provenance, given the user's flags and the
/// chosen output mode?
///
/// Defaults: text-to-TTY → on; text-to-pipe → off; JSON → off.
/// `--provenance` forces on; `--no-provenance` forces off.
pub fn should_show(flags: ProvenanceFlags, mode: OutputMode) -> bool {
    if flags.no_provenance {
        return false;
    }
    if flags.provenance {
        return true;
    }
    match mode {
        OutputMode::HumanText => {
            use std::io::IsTerminal;
            std::io::stdout().is_terminal()
        }
        OutputMode::Json => false,
    }
}

/// Print the provenance footer if `show` is true and the database had
/// provenance recorded. Called at the end of a query command's text output.
pub fn print_human_footer(prov: Option<&Provenance>, show: bool) {
    if !show {
        return;
    }
    let Some(p) = prov else { return };
    println!();
    print!("{}", p.human_footer());
}

/// Inject `_provenance` into a top-level JSON object under that key
/// if `show` is true. No-op if the value is not an object.
pub fn inject_into_json(value: &mut serde_json::Value, prov: Option<&Provenance>, show: bool) {
    if !show {
        return;
    }
    let Some(p) = prov else { return };
    if let Some(obj) = value.as_object_mut() {
        obj.insert("_provenance".to_string(), p.to_json_value());
    }
}

/// If a line is a `sct_provenance` metadata record, parse and return it.
/// Otherwise return `None` - the caller should treat the line as a ConceptRecord.
///
/// Callers that want to tolerate older (v3) NDJSONs without a header line simply
/// iterate as usual: this helper returns `None` for concept records, so the
/// caller's existing `serde_json::from_str::<ConceptRecord>` path still runs.
pub fn try_parse_ndjson_line(line: &str) -> Option<Provenance> {
    // Cheap prefilter: avoid a full serde parse on every concept line. The
    // sentinel "_type":"sct_provenance" is distinctive enough that a
    // substring match is a safe fast path.
    if !line.contains(NDJSON_TYPE_TAG) {
        return None;
    }
    serde_json::from_str::<Provenance>(line).ok()
}

/// Create the `metadata` key/value table if it doesn't exist.
pub fn create_sqlite_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS metadata (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );",
    )
    .context("creating metadata table")?;
    Ok(())
}

/// Persist a provenance record into the `metadata` key/value table,
/// replacing any existing values. Caller is responsible for transaction scoping.
pub fn write_sqlite(conn: &Connection, p: &Provenance) -> Result<()> {
    create_sqlite_table(conn)?;
    let mut stmt = conn.prepare("INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)")?;
    let source_paths_json = serde_json::to_string(&p.source_paths)?;
    let rows = [
        ("edition_label", p.edition_label.as_str()),
        ("release_date", p.release_date.as_str()),
        ("release_id", p.release_id.as_str()),
        ("source_paths", source_paths_json.as_str()),
        ("sct_version", p.sct_version.as_str()),
        ("created_at", p.created_at.as_str()),
    ];
    for (k, v) in rows {
        stmt.execute(params![k, v])?;
    }
    Ok(())
}

/// Load a provenance record from the `metadata` table, or `None` if either
/// the table is absent (older DB) or no edition label was ever written.
pub fn read_sqlite(conn: &Connection) -> Result<Option<Provenance>> {
    let has_table: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='metadata'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if !has_table {
        return Ok(None);
    }

    let get = |key: &str| -> Option<String> {
        conn.query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .ok()
    };

    let edition_label = get("edition_label").unwrap_or_default();
    if edition_label.is_empty() {
        // Table is present but unpopulated - treat as absent.
        return Ok(None);
    }
    let source_paths: Vec<String> = get("source_paths")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    Ok(Some(Provenance {
        type_tag: NDJSON_TYPE_TAG.to_string(),
        edition_label,
        release_date: get("release_date").unwrap_or_default(),
        release_id: get("release_id").unwrap_or_default(),
        source_paths,
        sct_version: get("sct_version").unwrap_or_default(),
        created_at: get("created_at").unwrap_or_default(),
    }))
}

// ---------------------------------------------------------------------------
// Helpers - edition classification and date extraction
// ---------------------------------------------------------------------------

/// Extract the release identifier from an input path - the final path
/// component with any `.zip` extension stripped.
fn release_id_from_path(p: &Path) -> String {
    let name = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let name = name.strip_suffix(".zip").unwrap_or(&name);
    let name = name.strip_suffix(".ZIP").unwrap_or(name);
    name.to_string()
}

/// Classify an edition from its release identifier. Recognises the common
/// SNOMED International and NHS TRUD naming conventions; falls back to the
/// raw id for anything unrecognised.
///
/// ```
/// use sct_rs::provenance::classify_edition;
/// assert_eq!(classify_edition("SnomedCT_MonolithRF2_PRODUCTION_20260701"), "UK Monolith");
/// assert_eq!(classify_edition("SnomedCT_InternationalRF2_PRODUCTION_20240101"), "International");
/// assert_eq!(classify_edition(""), "unknown");
/// ```
pub fn classify_edition(release_id: &str) -> String {
    let lower = release_id.to_lowercase();
    // Order matters: MonolithRF2 must be matched before "uk" since the
    // Monolith is UK-specific but has its own name.
    if lower.contains("monolithrf2") || lower.contains("monolith_rf2") {
        "UK Monolith".to_string()
    } else if lower.contains("ukclinical") {
        "UK Clinical".to_string()
    } else if lower.contains("ukdrug") {
        "UK Drug".to_string()
    } else if lower.contains("ukeditionrf2") || lower.contains("ukedition") {
        "UK Edition".to_string()
    } else if lower.contains("internationalrf2") || lower.contains("international_rf2") {
        "International".to_string()
    } else if release_id.is_empty() {
        "unknown".to_string()
    } else {
        release_id.to_string()
    }
}

/// Pull a YYYY-MM-DD date out of a release identifier by finding the first
/// 8-digit run that reads as a plausible date. Returns `None` if none found.
///
/// ```
/// use sct_rs::provenance::extract_release_date;
/// assert_eq!(
///     extract_release_date("SnomedCT_InternationalRF2_PRODUCTION_20240101T120000Z"),
///     Some("2024-01-01".to_string())
/// );
/// assert_eq!(extract_release_date("no date in here"), None);
/// ```
pub fn extract_release_date(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(7) {
        if bytes[i..i + 8].iter().all(|b| b.is_ascii_digit()) {
            let year: u32 = s[i..i + 4].parse().ok()?;
            let month: u32 = s[i + 4..i + 6].parse().ok()?;
            let day: u32 = s[i + 6..i + 8].parse().ok()?;
            if (1990..=2100).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)
            {
                return Some(format!(
                    "{}-{}-{}",
                    &s[i..i + 4],
                    &s[i + 4..i + 6],
                    &s[i + 6..i + 8]
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_known_editions() {
        assert_eq!(
            classify_edition("SnomedCT_InternationalRF2_PRODUCTION_20260301T120000Z"),
            "International"
        );
        assert_eq!(
            classify_edition("SnomedCT_UKClinicalRF2_PRODUCTION_20250401T000001Z"),
            "UK Clinical"
        );
        assert_eq!(
            classify_edition("SnomedCT_UKDrugRF2_PRODUCTION_20250401T000001Z"),
            "UK Drug"
        );
        assert_eq!(
            classify_edition("SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z"),
            "UK Monolith"
        );
        assert_eq!(classify_edition(""), "unknown");
    }

    #[test]
    fn classify_unknown_falls_back_to_raw() {
        assert_eq!(
            classify_edition("some-custom-local-extension-v1"),
            "some-custom-local-extension-v1"
        );
    }

    #[test]
    fn extract_date_finds_yyyymmdd() {
        assert_eq!(
            extract_release_date("SnomedCT_InternationalRF2_PRODUCTION_20260301T120000Z"),
            Some("2026-03-01".into())
        );
    }

    #[test]
    fn extract_date_rejects_garbage() {
        assert_eq!(extract_release_date("release-99999999"), None);
        assert_eq!(extract_release_date("short"), None);
    }

    #[test]
    fn ndjson_line_roundtrip() {
        let p = Provenance {
            type_tag: NDJSON_TYPE_TAG.to_string(),
            edition_label: "UK Monolith".into(),
            release_date: "2026-03-11".into(),
            release_id: "SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z".into(),
            source_paths: vec!["/tmp/release".into()],
            sct_version: "0.3.10".into(),
            created_at: "2026-04-14T10:00:00Z".into(),
        };
        let line = serde_json::to_string(&p).unwrap();
        let parsed = try_parse_ndjson_line(&line).expect("should parse");
        assert_eq!(parsed.edition_label, "UK Monolith");
        assert_eq!(parsed.release_date, "2026-03-11");
    }

    #[test]
    fn ndjson_line_rejects_concept_record() {
        let concept = r#"{"id":"123","fsn":"foo","preferred_term":"foo"}"#;
        assert!(try_parse_ndjson_line(concept).is_none());
    }
}
