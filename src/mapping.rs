// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQLite cross-terminology mapping engine.
//!
//! Maps codes between SNOMED CT, Read v2, CTV3, ICD-10, and OPCS-4, pivoting
//! through SNOMED CT. The engine has no command-line or file-reading behavior.

use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

#[cfg(feature = "cli")]
pub(crate) const SYSTEMS: [&str; 5] = ["snomed", "read2", "ctv3", "icd10", "opcs4"];

/// One mapped output: the target code, the SNOMED pivot concept it went through,
/// and that concept's preferred term (when known).
pub struct Mapped {
    pub target: String,
    pub snomed: String,
    pub display: Option<String>,
    /// RF2 `correlationId` from the ExtendedMap member that produced this
    /// mapping, when the target is ICD-10/OPCS-4 and the source data carries
    /// one. `None` for CTV3/Read v2 (SimpleMap has no correlation column) and
    /// for a SNOMED target (the identity mapping needs no correlation).
    ///
    /// Only read by `serve`-gated `$translate` equivalence reporting; builds
    /// without that feature (e.g. the `python` crate) never consume it.
    #[cfg_attr(not(feature = "serve"), allow(dead_code))]
    pub correlation: Option<String>,
}

/// Map a single `code` from terminology `from` to terminology `to`, pivoting
/// through SNOMED CT.
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
            let display = pt(conn, &snomed)?;
            for (target, correlation) in from_snomed(conn, &snomed, to)? {
                if seen.insert((snomed.clone(), target.clone())) {
                    out.push(Mapped {
                        target,
                        snomed: snomed.clone(),
                        display: display.clone(),
                        correlation,
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
            if !from_crossmaps.is_empty() || !table_exists(conn, "concept_maps")? {
                Ok(from_crossmaps)
            } else {
                collect(
                    conn,
                    "SELECT concept_id FROM concept_maps WHERE code = ?1 AND terminology = ?2",
                    params![code, from],
                )
            }
        }
        "icd10" if table_exists(conn, "crossmaps")? => {
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
        "opcs4" if table_exists(conn, "crossmaps")? => collect(
            conn,
            "SELECT DISTINCT source_code FROM crossmaps WHERE target_system = ?1 AND target_code = ?2",
            params![from, code],
        ),
        "icd10" | "opcs4" => Ok(vec![]), // no crossmaps table -> no maps
        _ => bail!("unknown source terminology {from:?}"),
    }
}

/// Map a SNOMED concept id to its code(s) in the `to` terminology, with the
/// RF2 `correlationId` for each ICD-10/OPCS-4 result (see [`Mapped::correlation`]).
fn from_snomed(
    conn: &Connection,
    concept: &str,
    to: &str,
) -> Result<Vec<(String, Option<String>)>> {
    match to {
        "snomed" => Ok(vec![(concept.to_string(), None)]),
        "ctv3" | "read2" => {
            let from_crossmaps = legacy_from_snomed_from_crossmaps(conn, concept, to)?;
            let codes = if !from_crossmaps.is_empty() || !table_exists(conn, "concept_maps")? {
                from_crossmaps
            } else {
                collect(
                    conn,
                    "SELECT code FROM concept_maps WHERE concept_id = ?1 AND terminology = ?2",
                    params![concept, to],
                )?
            };
            Ok(codes.into_iter().map(|c| (c, None)).collect())
        }
        "icd10" | "opcs4" if table_exists(conn, "crossmaps")? => collect_with_correlation(
            conn,
            // DISTINCT is over (target_code, correlation): the same target
            // code from two ExtendedMap rows with different correlations is
            // rare but not invalid RF2, and each is a genuinely different
            // claim about equivalence.
            "SELECT DISTINCT target_code, correlation FROM crossmaps
             WHERE source_code = ?1 AND target_system = ?2",
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
    if !table_exists(conn, "crossmaps")? {
        return Ok(vec![]);
    }
    let active_filter = if column_exists(conn, "crossmaps", "active")? {
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
    if !table_exists(conn, "crossmaps")? {
        return Ok(vec![]);
    }
    let active_filter = if column_exists(conn, "crossmaps", "active")? {
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

fn pt(conn: &Connection, id: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT preferred_term FROM concepts WHERE id = ?1",
        [id],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub(crate) fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1)",
        [name],
        |row| row.get(0),
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

#[cfg(feature = "cli")]
pub(crate) fn is_classification(system: &str) -> bool {
    matches!(system, "icd10" | "opcs4")
}

fn collect(conn: &Connection, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(p, |r| r.get::<_, String>(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Like [`collect`], but for a two-column `(code, correlation)` query, where
/// the second column may be `NULL`.
fn collect_with_correlation(
    conn: &Connection,
    sql: &str,
    p: &[&dyn rusqlite::ToSql],
) -> Result<Vec<(String, Option<String>)>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(p, |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}
