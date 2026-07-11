// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cross-terminology mapping engine (behind `sct map`, alias `transcode`):
//! map a stream of codes from one terminology to another,
//! pivoting through SNOMED CT. The CLI equivalent of the NHS Data Migration
//! Workbench `TRANSCODE` console function. Composable: reads codes from stdin
//! (or `--input`), writes TSV (or `--json`) to stdout, diagnostics to stderr.
//!
//! Supported systems: `snomed`, `read2`, `ctv3`, `icd10`, `opcs4`. The maps come
//! from the general `crossmaps` table. Older databases can still resolve CTV3 /
//! Read v2 through the legacy `concept_maps` table.

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::io::BufRead;

pub(crate) const SYSTEMS: [&str; 5] = ["snomed", "read2", "ctv3", "icd10", "opcs4"];

/// One mapped output: the target code, the SNOMED pivot concept it went through,
/// and that concept's preferred term (when known).
pub struct Mapped {
    pub target: String,
    pub snomed: String,
    pub display: Option<String>,
}

/// Map a single `code` from terminology `from` to terminology `to`, pivoting
/// through SNOMED CT. The pure core of `sct transcode` (no I/O), exposed for
/// tests and library reuse.
pub fn transcode_one(
    conn: &Connection,
    from: &str,
    code: &str,
    to: &str,
    forward_history: bool,
) -> Result<Vec<Mapped>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for pivot in to_snomed(conn, from, code)? {
        let forwarded = if forward_history {
            forward(conn, &pivot)?
        } else {
            vec![pivot]
        };
        for snomed in forwarded {
            let display = pt(conn, &snomed);
            for target in from_snomed(conn, &snomed, to)? {
                if seen.insert((snomed.clone(), target.clone())) {
                    out.push(Mapped {
                        target,
                        snomed: snomed.clone(),
                        display: display.clone(),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Resolve a code in `from` to its SNOMED concept id(s).
fn to_snomed(conn: &Connection, from: &str, code: &str) -> Result<Vec<String>> {
    match from {
        "snomed" => Ok(vec![code.to_string()]),
        "ctv3" | "read2" => {
            let from_crossmaps = legacy_to_snomed_from_crossmaps(conn, from, code)?;
            if !from_crossmaps.is_empty() || !table_exists(conn, "concept_maps") {
                Ok(from_crossmaps)
            } else {
                collect(
                    conn,
                    "SELECT concept_id FROM concept_maps WHERE code = ?1 AND terminology = ?2",
                    params![code, from],
                )
            }
        }
        "icd10" if table_exists(conn, "crossmaps") => {
            // Tolerate the undotted ICD-10 form (e.g. `I219`, common in UK
            // SUS/HES and legacy extracts) as well as the canonical dotted form
            // (`I21.9`) by comparing with dots stripped on both sides. Scoped to
            // ICD-10; OPCS-4 matching (below) is left untouched. Issue #31.
            collect(
                conn,
                "SELECT DISTINCT source_code FROM crossmaps
                 WHERE target_system = 'icd10' AND REPLACE(target_code, '.', '') = ?1",
                params![code.replace('.', "")],
            )
        }
        "opcs4" if table_exists(conn, "crossmaps") => collect(
            conn,
            "SELECT DISTINCT source_code FROM crossmaps WHERE target_system = ?1 AND target_code = ?2",
            params![from, code],
        ),
        "icd10" | "opcs4" => Ok(vec![]), // no crossmaps table -> no maps
        _ => bail!("unknown source terminology {from:?}"),
    }
}

/// Map a SNOMED concept id to its code(s) in the `to` terminology.
fn from_snomed(conn: &Connection, concept: &str, to: &str) -> Result<Vec<String>> {
    match to {
        "snomed" => Ok(vec![concept.to_string()]),
        "ctv3" | "read2" => {
            let from_crossmaps = legacy_from_snomed_from_crossmaps(conn, concept, to)?;
            if !from_crossmaps.is_empty() || !table_exists(conn, "concept_maps") {
                Ok(from_crossmaps)
            } else {
                collect(
                    conn,
                    "SELECT code FROM concept_maps WHERE concept_id = ?1 AND terminology = ?2",
                    params![concept, to],
                )
            }
        }
        "icd10" | "opcs4" if table_exists(conn, "crossmaps") => collect(
            conn,
            "SELECT DISTINCT target_code FROM crossmaps WHERE source_code = ?1 AND target_system = ?2",
            params![concept, to],
        ),
        "icd10" | "opcs4" => Ok(vec![]), // no crossmaps table -> no maps
        _ => bail!("unknown target terminology {to:?}"),
    }
}

fn legacy_to_snomed_from_crossmaps(
    conn: &Connection,
    from: &str,
    code: &str,
) -> Result<Vec<String>> {
    if !table_exists(conn, "crossmaps") {
        return Ok(vec![]);
    }
    let active_filter = if column_exists(conn, "crossmaps", "active") {
        "AND active != 0"
    } else {
        ""
    };
    collect(
        conn,
        &format!(
            "SELECT DISTINCT target_code FROM crossmaps
         WHERE source_system = ?1 AND source_code = ?2 AND target_system = 'snomed'
           {active_filter}"
        ),
        params![from, code],
    )
}

fn legacy_from_snomed_from_crossmaps(
    conn: &Connection,
    concept: &str,
    to: &str,
) -> Result<Vec<String>> {
    if !table_exists(conn, "crossmaps") {
        return Ok(vec![]);
    }
    let active_filter = if column_exists(conn, "crossmaps", "active") {
        "AND active != 0"
    } else {
        ""
    };
    collect(
        conn,
        &format!(
            "SELECT DISTINCT source_code FROM crossmaps
         WHERE target_system = 'snomed' AND target_code = ?1 AND source_system = ?2
           {active_filter}"
        ),
        params![concept, to],
    )
}

/// Forward an inactive concept to its replacement(s). Active concepts (and those
/// with no recorded forwarding) pass through unchanged.
fn forward(conn: &Connection, concept: &str) -> Result<Vec<String>> {
    let active: Option<bool> = conn
        .query_row(
            "SELECT active FROM concepts WHERE id = ?1",
            [concept],
            |r| r.get::<_, i64>(0).map(|a| a != 0),
        )
        .optional()?;
    if active == Some(true) {
        return Ok(vec![concept.to_string()]);
    }
    let targets = collect(
        conn,
        "SELECT target_id FROM concept_history
         WHERE source_id = ?1 AND association IN ('replaced_by','same_as','possibly_equivalent_to')",
        params![concept],
    )?;
    if targets.is_empty() {
        Ok(vec![concept.to_string()])
    } else {
        Ok(targets)
    }
}

fn pt(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row(
        "SELECT preferred_term FROM concepts WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub(crate) fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    for row in rows {
        if row.ok().as_deref() == Some(column) {
            return true;
        }
    }
    false
}

pub(crate) fn is_classification(system: &str) -> bool {
    matches!(system, "icd10" | "opcs4")
}

fn collect(conn: &Connection, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(p, |r| r.get::<_, String>(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Read codes from a file or stdin. The leading whitespace-delimited token of
/// each non-blank, non-`#` line is taken as the code (so `sct ecl expand`,
/// `cut`, `grep` output pipes straight in).
pub(crate) fn read_codes(input: Option<&std::path::Path>) -> Result<Vec<String>> {
    let reader: Box<dyn BufRead> = match input {
        Some(p) => Box::new(std::io::BufReader::new(
            std::fs::File::open(p).with_context(|| format!("opening {}", p.display()))?,
        )),
        None => Box::new(std::io::BufReader::new(std::io::stdin())),
    };
    let mut codes = Vec::new();
    for line in reader.lines() {
        let line = line.context("reading input")?;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(tok) = t.split_whitespace().next() {
            codes.push(tok.to_string());
        }
    }
    Ok(codes)
}
