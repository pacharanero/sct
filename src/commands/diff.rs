// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct diff` - Compare two SNOMED CT NDJSON artefacts and report what changed.
//!
//! Reports:
//!   - Concepts added (present in NEW, absent from OLD)
//!   - Concepts inactivated (active in OLD, absent or inactive in NEW)
//!   - Preferred term changes
//!   - Hierarchy changes (concept moved to a different top-level hierarchy)
//!
//! Output modes (`--format`):
//!   text (default, alias `summary`) - human-readable report on stdout
//!   ndjson                          - one diff record per line (streaming)
//!   json / yaml                     - all diff records as a single array

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::humanize::{fmt_count, plural_count};
use crate::schema::ConceptRecord;

#[derive(ValueEnum, Debug, Clone, PartialEq)]
pub enum DiffFormat {
    /// Human-readable summary report (default).
    #[value(alias = "summary")]
    Text,
    /// One JSON diff record per line (streaming).
    Ndjson,
    /// A pretty JSON array of all diff records.
    Json,
    /// A YAML list of all diff records.
    #[value(alias = "yml")]
    Yaml,
}

#[derive(Parser, Debug)]
pub struct Args {
    /// The older artefact: the `.ndjson` file produced by `sct ndjson` (the baseline). Not a SQLite `.db` or `.arrow` file.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub old: PathBuf,

    /// The newer artefact: the `.ndjson` file produced by `sct ndjson` (the comparison target). Not a SQLite `.db` or `.arrow` file.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub new: PathBuf,

    /// Output format.
    #[arg(long, short = 'f', default_value = "text")]
    pub format: DiffFormat,

    /// Output file for the structured formats (ndjson/json/yaml). Defaults to stdout.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub output: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Diff record (used by both output modes)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum DiffRecord {
    Added {
        id: String,
        preferred_term: String,
        hierarchy: String,
    },
    Inactivated {
        id: String,
        preferred_term: String,
        hierarchy: String,
    },
    PreferredTermChanged {
        id: String,
        old_preferred_term: String,
        new_preferred_term: String,
        hierarchy: String,
    },
    HierarchyChanged {
        id: String,
        preferred_term: String,
        old_hierarchy: String,
        new_hierarchy: String,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(args: Args) -> Result<()> {
    let old_map = load_ndjson(&args.old)?;
    let new_map = load_ndjson(&args.new)?;

    let mut diffs: Vec<DiffRecord> = Vec::new();

    // Concepts in NEW but not in OLD → added
    for (id, new_rec) in &new_map {
        if !old_map.contains_key(id) && new_rec.active {
            diffs.push(DiffRecord::Added {
                id: id.clone(),
                preferred_term: new_rec.preferred_term.clone(),
                hierarchy: new_rec.hierarchy.clone(),
            });
        }
    }

    // Walk OLD to find inactivations, term changes, hierarchy changes
    for (id, old_rec) in &old_map {
        if !old_rec.active {
            continue; // already inactive in old - not interesting
        }
        match new_map.get(id) {
            None => {
                // Not present at all in new → treat as inactivated
                diffs.push(DiffRecord::Inactivated {
                    id: id.clone(),
                    preferred_term: old_rec.preferred_term.clone(),
                    hierarchy: old_rec.hierarchy.clone(),
                });
            }
            Some(new_rec) if !new_rec.active => {
                diffs.push(DiffRecord::Inactivated {
                    id: id.clone(),
                    preferred_term: old_rec.preferred_term.clone(),
                    hierarchy: old_rec.hierarchy.clone(),
                });
            }
            Some(new_rec) => {
                if old_rec.preferred_term != new_rec.preferred_term {
                    diffs.push(DiffRecord::PreferredTermChanged {
                        id: id.clone(),
                        old_preferred_term: old_rec.preferred_term.clone(),
                        new_preferred_term: new_rec.preferred_term.clone(),
                        hierarchy: new_rec.hierarchy.clone(),
                    });
                }
                if old_rec.hierarchy != new_rec.hierarchy {
                    diffs.push(DiffRecord::HierarchyChanged {
                        id: id.clone(),
                        preferred_term: new_rec.preferred_term.clone(),
                        old_hierarchy: old_rec.hierarchy.clone(),
                        new_hierarchy: new_rec.hierarchy.clone(),
                    });
                }
            }
        }
    }

    // Stable output: sort by change type, then id
    diffs.sort_by_key(|d| (change_order(d), diff_id(d).to_string()));

    match args.format {
        DiffFormat::Text => print_summary(&diffs, &old_map, &new_map),
        DiffFormat::Ndjson => {
            let writer: Box<dyn Write> = match &args.output {
                Some(p) => Box::new(
                    std::fs::File::create(p)
                        .with_context(|| format!("creating {}", p.display()))?,
                ),
                None => Box::new(std::io::stdout()),
            };
            print_ndjson(&diffs, writer)
        }
        DiffFormat::Json | DiffFormat::Yaml => {
            let serialized = if args.format == DiffFormat::Json {
                serde_json::to_string_pretty(&diffs)?
            } else {
                serde_yaml_ng::to_string(&diffs)?
            };
            match &args.output {
                Some(p) => std::fs::write(p, serialized)
                    .with_context(|| format!("writing {}", p.display()))?,
                None => println!("{serialized}"),
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Output: summary
// ---------------------------------------------------------------------------

fn print_summary(
    diffs: &[DiffRecord],
    old_map: &HashMap<String, ConceptRecord>,
    new_map: &HashMap<String, ConceptRecord>,
) -> Result<()> {
    let added: Vec<&DiffRecord> = diffs
        .iter()
        .filter(|d| matches!(d, DiffRecord::Added { .. }))
        .collect();
    let inactivated: Vec<&DiffRecord> = diffs
        .iter()
        .filter(|d| matches!(d, DiffRecord::Inactivated { .. }))
        .collect();
    let term_changed: Vec<&DiffRecord> = diffs
        .iter()
        .filter(|d| matches!(d, DiffRecord::PreferredTermChanged { .. }))
        .collect();
    let hier_changed: Vec<&DiffRecord> = diffs
        .iter()
        .filter(|d| matches!(d, DiffRecord::HierarchyChanged { .. }))
        .collect();

    let old_active = old_map.values().filter(|r| r.active).count();
    let new_active = new_map.values().filter(|r| r.active).count();

    println!("# SNOMED CT diff summary");
    println!();
    println!(
        "Old artefact: {}",
        plural_count(old_active as u64, "active concept")
    );
    println!(
        "New artefact: {}",
        plural_count(new_active as u64, "active concept")
    );
    println!();
    println!("Changes:");
    println!("  Added:              {:>6}", fmt_count(added.len() as u64));
    println!(
        "  Inactivated:        {:>6}",
        fmt_count(inactivated.len() as u64)
    );
    println!(
        "  Preferred term:     {:>6}",
        fmt_count(term_changed.len() as u64)
    );
    println!(
        "  Hierarchy moved:    {:>6}",
        fmt_count(hier_changed.len() as u64)
    );

    if !added.is_empty() {
        println!();
        println!("## Added ({})", plural_count(added.len() as u64, "concept"));
        for d in &added {
            if let DiffRecord::Added {
                id,
                preferred_term,
                hierarchy,
            } = d
            {
                println!("  + [{id}] {preferred_term}  ({hierarchy})");
            }
        }
    }

    if !inactivated.is_empty() {
        println!();
        println!(
            "## Inactivated ({})",
            plural_count(inactivated.len() as u64, "concept")
        );
        for d in &inactivated {
            if let DiffRecord::Inactivated {
                id,
                preferred_term,
                hierarchy,
            } = d
            {
                println!("  - [{id}] {preferred_term}  ({hierarchy})");
            }
        }
    }

    if !term_changed.is_empty() {
        println!();
        println!(
            "## Preferred term changed ({})",
            plural_count(term_changed.len() as u64, "concept")
        );
        for d in &term_changed {
            if let DiffRecord::PreferredTermChanged {
                id,
                old_preferred_term,
                new_preferred_term,
                ..
            } = d
            {
                println!("  ~ [{id}] \"{old_preferred_term}\" -> \"{new_preferred_term}\"");
            }
        }
    }

    if !hier_changed.is_empty() {
        println!();
        println!(
            "## Hierarchy changed ({})",
            plural_count(hier_changed.len() as u64, "concept")
        );
        for d in &hier_changed {
            if let DiffRecord::HierarchyChanged {
                id,
                preferred_term,
                old_hierarchy,
                new_hierarchy,
            } = d
            {
                println!("  > [{id}] {preferred_term}  {old_hierarchy} -> {new_hierarchy}");
            }
        }
    }

    if diffs.is_empty() {
        println!();
        println!("No differences found.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Output: NDJSON
// ---------------------------------------------------------------------------

fn print_ndjson(diffs: &[DiffRecord], mut writer: Box<dyn Write>) -> Result<()> {
    for d in diffs {
        let line = serde_json::to_string(d).context("serialising diff record")?;
        writeln!(writer, "{}", line)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load an NDJSON file into a HashMap<id, ConceptRecord>.
fn load_ndjson(path: &PathBuf) -> Result<HashMap<String, ConceptRecord>> {
    anyhow::ensure!(
        path.extension().and_then(|e| e.to_str()) == Some("ndjson"),
        "{} is not an .ndjson file; `sct diff` compares the NDJSON artefacts produced by `sct ndjson`, not the SQLite databases produced by `sct sqlite`",
        path.display()
    );
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut map = HashMap::new();
    for line in reader.lines() {
        let line = line.context("reading line")?;
        if line.trim().is_empty() {
            continue;
        }
        if crate::provenance::try_parse_ndjson_line(&line).is_some() {
            continue;
        }
        let record: ConceptRecord = serde_json::from_str(&line).context("parsing NDJSON record")?;
        map.insert(record.id.clone(), record);
    }
    Ok(map)
}

fn diff_id(d: &DiffRecord) -> &str {
    match d {
        DiffRecord::Added { id, .. } => id,
        DiffRecord::Inactivated { id, .. } => id,
        DiffRecord::PreferredTermChanged { id, .. } => id,
        DiffRecord::HierarchyChanged { id, .. } => id,
    }
}

fn change_order(d: &DiffRecord) -> u8 {
    match d {
        DiffRecord::Added { .. } => 0,
        DiffRecord::Inactivated { .. } => 1,
        DiffRecord::PreferredTermChanged { .. } => 2,
        DiffRecord::HierarchyChanged { .. } => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(id: &str, preferred_term: &str, hierarchy: &str, active: bool) -> ConceptRecord {
        use crate::schema::SCHEMA_VERSION;
        use indexmap::IndexMap;
        ConceptRecord {
            id: id.into(),
            fsn: format!("{preferred_term} (finding)"),
            preferred_term: preferred_term.into(),
            synonyms: vec![],
            hierarchy: hierarchy.into(),
            hierarchy_path: vec![hierarchy.into(), preferred_term.into()],
            parents: vec![],
            children_count: 0,
            active,
            definition_status: "900000000000074008".into(),
            module: "900000000000207008".into(),
            effective_time: "20260101".into(),
            attributes: IndexMap::new(),
            ctv3_codes: vec![],
            read2_codes: vec![],
            refsets: vec![],
            relationships: vec![],
            crossmaps: vec![],
            schema_version: SCHEMA_VERSION,
        }
    }

    fn write_ndjson(records: &[ConceptRecord]) -> tempfile::NamedTempFile {
        // The .ndjson suffix matters: load_ndjson validates the file extension.
        let mut f = tempfile::Builder::new()
            .suffix(".ndjson")
            .tempfile()
            .unwrap();
        for r in records {
            writeln!(f, "{}", serde_json::to_string(r).unwrap()).unwrap();
        }
        f
    }

    #[test]
    fn rejects_non_ndjson_file_with_actionable_error() {
        // A file that parses fine as text but isn't an .ndjson artefact - the
        // common mistake is passing the SQLite database instead.
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("uk_sct2mo_42.5.0.db");
        std::fs::write(&db_path, b"SQLite format 3\x00 and other binary noise").unwrap();

        let error = load_ndjson(&db_path).unwrap_err();
        let message = error.to_string();
        assert!(message.contains(".ndjson"), "mentions .ndjson: {message}");
        assert!(
            message.contains("sct ndjson"),
            "mentions sct ndjson: {message}"
        );
        assert!(
            message.contains("sct sqlite"),
            "mentions sct sqlite: {message}"
        );
    }

    #[test]
    fn error_message_names_the_offending_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wrong-type.db");
        std::fs::write(&path, b"not an ndjson file").unwrap();

        let error = load_ndjson(&path).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("wrong-type.db"),
            "error names the file: {message}"
        );
    }

    #[test]
    fn detects_added_concept() {
        let old_records = vec![make_record("111", "Fever", "Clinical finding", true)];
        let new_records = vec![
            make_record("111", "Fever", "Clinical finding", true),
            make_record("222", "New concept", "Clinical finding", true),
        ];
        let old_file = write_ndjson(&old_records);
        let new_file = write_ndjson(&new_records);

        let old_map = load_ndjson(&old_file.path().to_path_buf()).unwrap();
        let new_map = load_ndjson(&new_file.path().to_path_buf()).unwrap();

        let mut diffs = vec![];
        for (id, new_rec) in &new_map {
            if !old_map.contains_key(id) && new_rec.active {
                diffs.push(DiffRecord::Added {
                    id: id.clone(),
                    preferred_term: new_rec.preferred_term.clone(),
                    hierarchy: new_rec.hierarchy.clone(),
                });
            }
        }
        assert_eq!(diffs.len(), 1);
        assert!(matches!(&diffs[0], DiffRecord::Added { id, .. } if id == "222"));
    }

    #[test]
    fn detects_inactivated_concept() {
        let old_records = vec![make_record("111", "Fever", "Clinical finding", true)];
        let new_records = vec![make_record("111", "Fever", "Clinical finding", false)];
        let old_file = write_ndjson(&old_records);
        let new_file = write_ndjson(&new_records);

        let old_map = load_ndjson(&old_file.path().to_path_buf()).unwrap();
        let new_map = load_ndjson(&new_file.path().to_path_buf()).unwrap();

        let mut diffs = vec![];
        for (id, old_rec) in &old_map {
            if !old_rec.active {
                continue;
            }
            if let Some(new_rec) = new_map.get(id) {
                if !new_rec.active {
                    diffs.push(DiffRecord::Inactivated {
                        id: id.clone(),
                        preferred_term: old_rec.preferred_term.clone(),
                        hierarchy: old_rec.hierarchy.clone(),
                    });
                }
            }
        }
        assert_eq!(diffs.len(), 1);
        assert!(matches!(&diffs[0], DiffRecord::Inactivated { id, .. } if id == "111"));
    }

    #[test]
    fn detects_preferred_term_change() {
        let old_records = vec![make_record("111", "Fever", "Clinical finding", true)];
        let new_records = vec![make_record("111", "Pyrexia", "Clinical finding", true)];
        let old_file = write_ndjson(&old_records);
        let new_file = write_ndjson(&new_records);

        let old_map = load_ndjson(&old_file.path().to_path_buf()).unwrap();
        let new_map = load_ndjson(&new_file.path().to_path_buf()).unwrap();

        let mut diffs = vec![];
        for (id, old_rec) in &old_map {
            if let Some(new_rec) = new_map.get(id) {
                if old_rec.preferred_term != new_rec.preferred_term {
                    diffs.push(DiffRecord::PreferredTermChanged {
                        id: id.clone(),
                        old_preferred_term: old_rec.preferred_term.clone(),
                        new_preferred_term: new_rec.preferred_term.clone(),
                        hierarchy: new_rec.hierarchy.clone(),
                    });
                }
            }
        }
        assert_eq!(diffs.len(), 1);
        assert!(
            matches!(&diffs[0], DiffRecord::PreferredTermChanged { new_preferred_term, .. } if new_preferred_term == "Pyrexia")
        );
    }
}
