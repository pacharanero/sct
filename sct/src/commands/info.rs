//! `sct info` — Inspect a `sct`-produced artefact and print a summary.
//!
//! Supports:
//!   .ndjson  — concept count, schema_version, hierarchy breakdown, source date
//!   .db      — concept count, schema_version, FTS row count, file size
//!   .arrow   — embedding count, embedding dimension, file size

use anyhow::{Context, Result};
use clap::Parser;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use crate::schema::ConceptRecord;

#[derive(Parser, Debug)]
pub struct Args {
    /// Path to a `.ndjson`, `.db`, or `.arrow` file produced by `sct`.
    pub file: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let path = &args.file;
    anyhow::ensure!(path.exists(), "file not found: {}", path.display());

    match path.extension().and_then(|e| e.to_str()) {
        Some("ndjson") => info_ndjson(path),
        Some("db") => info_db(path),
        Some("arrow") => info_arrow(path),
        other => anyhow::bail!(
            "unrecognised file extension {:?}; expected .ndjson, .db, or .arrow",
            other
        ),
    }
}

// ---------------------------------------------------------------------------
// NDJSON
// ---------------------------------------------------------------------------

fn info_ndjson(path: &Path) -> Result<()> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let file_size = file.metadata()?.len();
    let reader = BufReader::new(file);

    let mut count: u64 = 0;
    let mut inactive_count: u64 = 0;
    let mut schema_version: Option<u32> = None;
    let mut hierarchy_counts: BTreeMap<String, u64> = BTreeMap::new();

    for line in reader.lines() {
        let line = line.context("reading line")?;
        if line.trim().is_empty() {
            continue;
        }
        let record: ConceptRecord = serde_json::from_str(&line).context("parsing NDJSON record")?;

        count += 1;
        if !record.active {
            inactive_count += 1;
        }
        if schema_version.is_none() {
            schema_version = Some(record.schema_version);
        }
        *hierarchy_counts
            .entry(record.hierarchy.clone())
            .or_insert(0) += 1;
    }

    // Try to parse a release date from the filename (e.g. "…20260311…").
    let source_date = extract_date_from_filename(path);

    println!("File:           {}", path.display());
    println!("Size:           {}", human_size(file_size));
    println!("Format:         NDJSON");
    println!(
        "Schema version: {}",
        schema_version
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    if let Some(date) = source_date {
        println!("Release date:   {}", date);
    }
    println!("Concepts:       {}", fmt_count(count));
    if inactive_count > 0 {
        println!("  (inactive):   {}", fmt_count(inactive_count));
    }
    println!();
    println!(
        "Hierarchy breakdown ({} top-level):",
        hierarchy_counts.len()
    );

    // Sort by count descending for display
    let mut sorted: Vec<(&String, &u64)> = hierarchy_counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1));
    for (hierarchy, n) in sorted {
        println!("  {:<45} {:>7}", hierarchy, fmt_count(*n));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

fn info_db(path: &Path) -> Result<()> {
    use rusqlite::Connection;

    let file_size = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .len();

    let conn =
        Connection::open(path).with_context(|| format!("opening database {}", path.display()))?;
    conn.execute_batch("PRAGMA query_only = ON;")?;

    let concept_count: u64 = conn
        .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get::<_, i64>(0))
        .map(|n| n as u64)?;

    let schema_version: Option<u32> = conn
        .query_row("SELECT MAX(schema_version) FROM concepts", [], |r| r.get(0))
        .unwrap_or(None);

    let fts_count: u64 = conn
        .query_row("SELECT COUNT(*) FROM concepts_fts", [], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n as u64)
        .unwrap_or(0);

    let isa_count: u64 = conn
        .query_row("SELECT COUNT(*) FROM concept_isa", [], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n as u64)
        .unwrap_or(0);

    // Hierarchy breakdown
    let mut stmt = conn.prepare(
        "SELECT hierarchy, COUNT(*) as n FROM concepts GROUP BY hierarchy ORDER BY n DESC",
    )?;
    let rows: Vec<(String, u64)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1).map(|n| n as u64)?,
            ))
        })?
        .flatten()
        .collect();

    println!("File:              {}", path.display());
    println!("Size:              {}", human_size(file_size));
    println!("Format:            SQLite (sct sqlite)");
    println!(
        "Schema version:    {}",
        schema_version
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    println!("Concepts:          {}", fmt_count(concept_count));
    println!("FTS5 rows:         {}", fmt_count(fts_count));
    println!("IS-A edges:        {}", fmt_count(isa_count));
    println!();
    println!("Hierarchy breakdown ({} top-level):", rows.len());
    for (hierarchy, n) in &rows {
        println!("  {:<45} {:>7}", hierarchy, fmt_count(*n));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Arrow IPC
// ---------------------------------------------------------------------------

fn info_arrow(path: &Path) -> Result<()> {
    use arrow::ipc::reader::FileReader;

    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let file_size = file.metadata()?.len();
    let reader = FileReader::try_new(file, None).context("reading Arrow IPC file")?;

    let schema = reader.schema();

    // Determine embedding dimension from the FixedSizeList field
    let dim: Option<i32> = schema.fields().iter().find_map(|f| {
        if f.name() == "embedding" {
            if let arrow::datatypes::DataType::FixedSizeList(_, size) = f.data_type() {
                return Some(*size);
            }
        }
        None
    });

    // Count total rows by summing batches
    let row_count: u64 = reader
        .map(|b| b.map(|b| b.num_rows() as u64).unwrap_or(0))
        .sum();

    println!("File:             {}", path.display());
    println!("Size:             {}", human_size(file_size));
    println!("Format:           Arrow IPC (sct embed)");
    println!("Embeddings:       {}", fmt_count(row_count));
    println!(
        "Dimension:        {}",
        dim.map(|d| d.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    println!();
    println!("Schema:");
    for field in schema.fields() {
        println!("  {:<20} {}", field.name(), field.data_type());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn human_size(bytes: u64) -> String {
    const GB: u64 = 1 << 30;
    const MB: u64 = 1 << 20;
    const KB: u64 = 1 << 10;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn fmt_count(n: u64) -> String {
    // Simple thousands-separator formatting
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

/// Try to extract a YYYYMMDD date from a filename like
/// `snomedct-monolithrf2-production-20260311t120000z.ndjson`.
fn extract_date_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    // Find an 8-digit run that looks like YYYYMMDD
    let bytes = stem.as_bytes();
    for i in 0..bytes.len().saturating_sub(7) {
        if bytes[i..i + 8].iter().all(|b| b.is_ascii_digit()) {
            let s = &stem[i..i + 8];
            // Basic sanity: year 1900–2100, month 01–12, day 01–31
            let year: u32 = s[0..4].parse().ok()?;
            let month: u32 = s[4..6].parse().ok()?;
            let day: u32 = s[6..8].parse().ok()?;
            if (1900..=2100).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)
            {
                return Some(format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_count_thousands() {
        assert_eq!(fmt_count(1_234_567), "1,234,567");
        assert_eq!(fmt_count(831_132), "831,132");
        assert_eq!(fmt_count(42), "42");
    }

    #[test]
    fn extract_date_from_monolith_filename() {
        use std::path::PathBuf;
        let p = PathBuf::from("snomedct-monolithrf2-production-20260311t120000z.ndjson");
        assert_eq!(extract_date_from_filename(&p), Some("2026-03-11".into()));
    }

    #[test]
    fn extract_date_none_for_plain_name() {
        use std::path::PathBuf;
        let p = PathBuf::from("snomed.ndjson");
        assert_eq!(extract_date_from_filename(&p), None);
    }
}
