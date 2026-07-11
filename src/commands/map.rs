// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct map` - cross-terminology mapping between SNOMED CT, Read v2, CTV3,
//! ICD-10, and OPCS-4. Unifies the former `crosswalk` (show all equivalents of
//! one code) and `transcode` (map a stream of codes to one target terminology),
//! which remain as command aliases.
//!
//! Input source and direction are orthogonal:
//!   - a positional `<CODE>`, or a stream on stdin / `--input FILE`;
//!   - `--to` present maps to that one terminology; omitted shows every one.
//!
//! Composable: data on stdout (`--format text|tsv|csv|json`), diagnostics on
//! stderr. See `docs/commands/map.md`.

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use rusqlite::Connection;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use crate::commands::crosswalk::equivalents;
use crate::commands::transcode::{
    is_classification, read_codes, table_exists, transcode_one, SYSTEMS,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum MapFormat {
    /// Human-readable (default).
    Text,
    /// Tab-separated, with a header row.
    Tsv,
    /// Comma-separated, with a header row.
    Csv,
    /// One JSON object per input line (NDJSON).
    Json,
}

#[derive(Parser, Debug)]
pub struct Args {
    /// Code to map. Omit to read codes from stdin, or use `--input`. `-` also
    /// means stdin.
    pub code: Option<String>,

    /// Source terminology: snomed (default) | read2 | ctv3 | icd10 | opcs4.
    #[arg(long, default_value = "snomed")]
    pub from: String,

    /// Target terminology. Omit to show equivalents in every other terminology.
    #[arg(long)]
    pub to: Option<String>,

    /// Read codes from this file (leading token per line) instead of stdin.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub input: Option<PathBuf>,

    /// Forward inactive SNOMED pivots to their replacement(s) via concept_history
    /// (needs a database built with `--refsets all`).
    #[arg(long)]
    pub forward_history: bool,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = MapFormat::Text)]
    pub format: MapFormat,

    /// Deprecated alias for `--format json`.
    #[arg(long, hide = true)]
    pub json: bool,

    /// SNOMED CT SQLite database. Discovered via the usual path-resolution chain
    /// when omitted (see `docs/path-resolution.md`).
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    let from = args.from.to_lowercase();
    validate_system(&from)?;
    let to = match &args.to {
        Some(t) => {
            let t = t.to_lowercase();
            validate_system(&t)?;
            Some(t)
        }
        None => None,
    };

    let format = if args.json {
        eprintln!("warning: --json is deprecated; use --format json");
        MapFormat::Json
    } else {
        args.format
    };

    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = crate::commands::open_db_readonly(&db, None)
        .with_context(|| format!("opening database {}", db.display()))?;

    // Precondition checks, mirroring the old transcode behaviour.
    if args.forward_history && !table_exists(&conn, "concept_history") {
        bail!(
            "--forward-history needs concept history, absent from this database. \
             Rebuild with `sct ndjson --refsets all` then `sct sqlite`."
        );
    }
    let has_crossmaps = table_exists(&conn, "crossmaps");
    if let Some(to) = &to {
        // An explicit conversion to/from a classification needs the maps present.
        if (is_classification(&from) || is_classification(to)) && !has_crossmaps {
            bail!(
                "this database has no ICD-10/OPCS-4 maps. Rebuild with \
                 `sct ndjson --rf2 <release> --refsets all` then `sct sqlite`."
            );
        }
    } else if !has_crossmaps {
        // All-equivalents mode degrades gracefully: just note the gap.
        eprintln!(
            "note: this database has no ICD-10/OPCS-4 maps (rebuild with `--refsets all` \
             to include them); those columns will be empty."
        );
    }

    let inputs = gather_inputs(&args)?;
    if inputs.is_empty() {
        bail!("no codes to map (give a <CODE>, use --input <FILE>, or pipe codes on stdin)");
    }

    let mut out = std::io::stdout().lock();
    match &to {
        Some(to) => render_conversion(
            &mut out,
            &conn,
            &from,
            to,
            &inputs,
            args.forward_history,
            format,
        )?,
        None => render_equivalents(&mut out, &conn, &from, &inputs, format)?,
    }
    Ok(())
}

fn validate_system(s: &str) -> Result<()> {
    if !SYSTEMS.contains(&s) {
        bail!(
            "unknown terminology {s:?}; expected one of {}",
            SYSTEMS.join(", ")
        );
    }
    Ok(())
}

/// Resolve the input codes: a positional code, or a stream from `--input` / stdin.
fn gather_inputs(args: &Args) -> Result<Vec<String>> {
    match (&args.code, &args.input) {
        (Some(c), Some(_)) if c != "-" => {
            bail!("give a single <CODE> or --input <FILE>, not both")
        }
        (Some(c), None) if c != "-" => Ok(vec![c.clone()]),
        // `-`, or no positional: read the stream (file or stdin).
        (_, input) => {
            if input.is_none() && std::io::stdin().is_terminal() {
                bail!(
                    "no codes to map (give a <CODE>, use --input <FILE>, or pipe codes on stdin)"
                );
            }
            read_codes(input.as_deref())
        }
    }
}

// ---------------------------------------------------------------------------
// Conversion mode (`--to` given): map each input to one target terminology.
// ---------------------------------------------------------------------------

fn render_conversion(
    out: &mut impl Write,
    conn: &Connection,
    from: &str,
    to: &str,
    inputs: &[String],
    forward_history: bool,
    format: MapFormat,
) -> Result<()> {
    if matches!(format, MapFormat::Tsv | MapFormat::Csv) {
        let sep = separator(format);
        writeln!(out, "input{sep}target{sep}snomed{sep}display")?;
    }
    let mut matched = 0usize;
    for code in inputs {
        let rows = transcode_one(conn, from, code, to, forward_history)?;
        if !rows.is_empty() {
            matched += 1;
        }
        match format {
            MapFormat::Text => {
                let targets: Vec<&str> = rows.iter().map(|r| r.target.as_str()).collect();
                if targets.is_empty() {
                    writeln!(out, "{code}  →  (no {to} map)")?;
                } else {
                    writeln!(out, "{code}  →  {}", targets.join(", "))?;
                }
            }
            MapFormat::Tsv | MapFormat::Csv => {
                let sep = separator(format);
                if rows.is_empty() {
                    writeln!(out, "{}{sep}{sep}{sep}", field(code, format))?;
                } else {
                    for r in &rows {
                        writeln!(
                            out,
                            "{}{sep}{}{sep}{}{sep}{}",
                            field(code, format),
                            field(&r.target, format),
                            field(&r.snomed, format),
                            field(r.display.as_deref().unwrap_or(""), format),
                        )?;
                    }
                }
            }
            MapFormat::Json => {
                if rows.is_empty() {
                    writeln!(
                        out,
                        "{}",
                        serde_json::json!({
                            "input": code, "from": from, "to": to,
                            "target": serde_json::Value::Null,
                            "snomed": serde_json::Value::Null,
                            "display": serde_json::Value::Null,
                        })
                    )?;
                } else {
                    for r in &rows {
                        writeln!(
                            out,
                            "{}",
                            serde_json::json!({
                                "input": code, "from": from, "to": to,
                                "target": r.target, "snomed": r.snomed, "display": r.display,
                            })
                        )?;
                    }
                }
            }
        }
    }
    eprintln!(
        "map {from} -> {to}: {} input code(s), {matched} mapped, {} unmapped",
        inputs.len(),
        inputs.len() - matched
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Equivalents mode (`--to` omitted): show every terminology for each input.
// ---------------------------------------------------------------------------

fn render_equivalents(
    out: &mut impl Write,
    conn: &Connection,
    from: &str,
    inputs: &[String],
    format: MapFormat,
) -> Result<()> {
    // Column order for tabular formats: every system except the source.
    let cols: Vec<&&str> = SYSTEMS.iter().filter(|s| **s != from).collect();
    if matches!(format, MapFormat::Tsv | MapFormat::Csv) {
        let sep = separator(format);
        let mut header = format!("input{sep}display");
        for c in &cols {
            header.push_str(&format!("{sep}{c}"));
        }
        writeln!(out, "{header}")?;
    }

    let mut resolved = 0usize;
    for (i, code) in inputs.iter().enumerate() {
        let cw = equivalents(conn, from, code)?;
        if !cw.snomed.is_empty() {
            resolved += 1;
        }
        let lookup = |sys: &str| -> String {
            cw.equivalents
                .iter()
                .find(|(s, _)| *s == sys)
                .map(|(_, v)| v.join(";"))
                .unwrap_or_default()
        };
        match format {
            MapFormat::Text => {
                if i > 0 {
                    writeln!(out)?;
                }
                if from == "snomed" {
                    writeln!(out, "{code}  {}", cw.display)?;
                } else if cw.snomed.is_empty() {
                    writeln!(out, "{code} ({from})  →  (no SNOMED CT match)")?;
                } else {
                    writeln!(
                        out,
                        "{code} ({from})  →  SNOMED {}  {}",
                        cw.snomed, cw.display
                    )?;
                }
                for (sys, codes) in &cw.equivalents {
                    let val = if codes.is_empty() {
                        "(none)".to_string()
                    } else {
                        codes.join(", ")
                    };
                    writeln!(out, "  {:<7} {val}", format!("{sys}:"))?;
                }
            }
            MapFormat::Tsv | MapFormat::Csv => {
                let sep = separator(format);
                let mut row = format!("{}{sep}{}", field(code, format), field(&cw.display, format));
                for c in &cols {
                    row.push_str(&format!("{sep}{}", field(&lookup(c), format)));
                }
                writeln!(out, "{row}")?;
            }
            MapFormat::Json => {
                let eq: serde_json::Map<String, serde_json::Value> = cw
                    .equivalents
                    .iter()
                    .map(|(s, v)| (s.to_string(), serde_json::json!(v)))
                    .collect();
                writeln!(
                    out,
                    "{}",
                    serde_json::json!({
                        "input": code, "from": from,
                        "snomed": cw.snomed, "display": cw.display,
                        "equivalents": eq,
                    })
                )?;
            }
        }
    }
    eprintln!(
        "map {from} equivalents: {} input code(s), {resolved} resolved to SNOMED CT",
        inputs.len()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Field formatting
// ---------------------------------------------------------------------------

fn separator(format: MapFormat) -> char {
    match format {
        MapFormat::Csv => ',',
        _ => '\t',
    }
}

/// Escape a field for the target format: CSV quoting when needed, or tab/newline
/// stripping for TSV so a stray character cannot break the column layout.
fn field(s: &str, format: MapFormat) -> String {
    match format {
        MapFormat::Csv => {
            if s.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_string()
            }
        }
        _ => s.replace(['\t', '\n', '\r'], " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// A tiny crossmaps fixture: SNOMED 22298006 ↔ ICD-10 I21.9 and Read v2 G30.
    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (id TEXT PRIMARY KEY, preferred_term TEXT NOT NULL,
                 active INTEGER NOT NULL);
             CREATE TABLE crossmaps (source_system TEXT, source_code TEXT,
                 target_system TEXT, target_code TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO concepts VALUES ('22298006', 'Myocardial infarction', 1)",
            [],
        )
        .unwrap();
        // Note the two families store opposite directions in `crossmaps`:
        // SNOMED→classification for ICD-10/OPCS-4, but legacy→SNOMED for Read v2/CTV3.
        for (ss, sc, ts, tc) in [
            ("snomed", "22298006", "icd10", "I21.9"),
            ("read2", "G30..", "snomed", "22298006"),
        ] {
            conn.execute(
                "INSERT INTO crossmaps VALUES (?1, ?2, ?3, ?4)",
                [ss, sc, ts, tc],
            )
            .unwrap();
        }
        conn
    }

    fn render_conv(conn: &Connection, from: &str, to: &str, code: &str, f: MapFormat) -> String {
        let mut buf = Vec::new();
        render_conversion(&mut buf, conn, from, to, &[code.to_string()], false, f).unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn render_equiv(conn: &Connection, from: &str, code: &str, f: MapFormat) -> String {
        let mut buf = Vec::new();
        render_equivalents(&mut buf, conn, from, &[code.to_string()], f).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn icd10_input_tolerates_undotted() {
        let conn = fixture();
        // Baseline: the canonical dotted form resolves to the mapped concept.
        assert!(
            render_conv(&conn, "icd10", "snomed", "I21.9", MapFormat::Text).contains("22298006")
        );
        // Issue #31: the undotted form a UK claims extract might present resolves
        // to the same concept.
        assert!(
            render_conv(&conn, "icd10", "snomed", "I219", MapFormat::Text).contains("22298006")
        );
    }

    #[test]
    fn conversion_text_and_tsv_and_json() {
        let conn = fixture();
        assert!(
            render_conv(&conn, "snomed", "icd10", "22298006", MapFormat::Text).contains("→  I21.9")
        );
        let tsv = render_conv(&conn, "snomed", "icd10", "22298006", MapFormat::Tsv);
        assert!(tsv.starts_with("input\ttarget\tsnomed\tdisplay"));
        assert!(tsv.contains("22298006\tI21.9\t22298006\tMyocardial infarction"));
        let json = render_conv(&conn, "snomed", "icd10", "22298006", MapFormat::Json);
        assert!(json.contains("\"target\":\"I21.9\""));
    }

    #[test]
    fn conversion_unmatched_emits_empty_row() {
        let conn = fixture();
        let tsv = render_conv(&conn, "snomed", "opcs4", "22298006", MapFormat::Tsv);
        // No OPCS-4 map → one row with empty target fields.
        assert!(tsv.lines().nth(1).unwrap().starts_with("22298006\t\t\t"));
        assert!(
            render_conv(&conn, "snomed", "opcs4", "22298006", MapFormat::Text)
                .contains("(no opcs4 map)")
        );
    }

    #[test]
    fn equivalents_text_lists_all_systems() {
        let conn = fixture();
        let text = render_equiv(&conn, "snomed", "22298006", MapFormat::Text);
        assert!(text.contains("Myocardial infarction"));
        assert!(text.contains("icd10:"));
        assert!(text.contains("I21.9"));
        assert!(text.contains("read2:"));
        assert!(text.contains("G30.."));
    }

    #[test]
    fn equivalents_csv_header_excludes_source_and_quotes_commas() {
        let conn = fixture();
        conn.execute(
            "UPDATE concepts SET preferred_term = 'Infarct, myocardial' WHERE id = '22298006'",
            [],
        )
        .unwrap();
        let csv = render_equiv(&conn, "snomed", "22298006", MapFormat::Csv);
        let header = csv.lines().next().unwrap();
        assert_eq!(header, "input,display,read2,ctv3,icd10,opcs4");
        // Display contains a comma → must be quoted.
        assert!(csv.contains("\"Infarct, myocardial\""));
    }

    #[test]
    fn reverse_direction_pivots_through_snomed() {
        let conn = fixture();
        // ICD-10 → Read v2, via the SNOMED pivot.
        let text = render_conv(&conn, "icd10", "read2", "I21.9", MapFormat::Text);
        assert!(text.contains("G30.."));
    }
}
