// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct lexical` - Full-text keyword search over a SNOMED CT SQLite database.
//!
//! Uses the FTS5 virtual table built by `sct sqlite`. Supports any FTS5 query
//! syntax: phrase search, prefix search, column filters, boolean operators.
//!
//! Examples:
//!   sct lexical --db snomed.db "heart attack"
//!   sct lexical --db snomed.db "myocardial infarct*"
//!   sct lexical --db snomed.db "heart attack" --hierarchy "Clinical finding"
//!   sct lexical --db snomed.db "heart attack" --limit 20

use anyhow::Result;
use clap::Parser;
use rusqlite::params;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::format::{ConceptFields, ConceptFormat};
use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, ProvenanceFlags};

#[derive(Parser, Debug)]
pub struct Args {
    /// Search query (FTS5 syntax: phrases, prefix*, boolean AND/OR/NOT).
    pub query: String,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Restrict results to a specific top-level hierarchy (e.g. "Clinical finding").
    #[arg(long)]
    pub hierarchy: Option<String>,

    /// Maximum number of results to return.
    #[arg(long, short, default_value = "10")]
    pub limit: u32,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Emit only matching SCTIDs (newline-delimited) for piping, e.g.
    /// `sct lexical "asthma" --ids | sct codelist add list.codelist -`.
    #[arg(long)]
    pub ids: bool,

    /// Override the per-concept line template (text output only). See
    /// `docs/commands/refset.md` for the variable list.
    #[arg(long)]
    pub template: Option<String>,

    /// Override the FSN suffix template (rendered only when FSN differs from PT).
    #[arg(long)]
    pub template_fsn_suffix: Option<String>,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

pub fn run(args: Args) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = crate::commands::open_db_readonly(&db, None)?;
    let prov = provenance::read_sqlite(&conn).unwrap_or(None);
    let out = args.format;
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    // Sanitise the FTS5 query: wrap in quotes if it looks like plain text
    // (no FTS5 operators), to avoid parse errors on bare terms with special chars.
    let fts_query = sanitise_fts_query(&args.query);

    let results: Vec<(String, String, String, String)> = if let Some(ref hier) = args.hierarchy {
        let sql = "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy
                   FROM concepts_fts
                   JOIN concepts c ON concepts_fts.rowid = c.rowid
                   WHERE concepts_fts MATCH ?1
                     AND c.hierarchy = ?2
                   ORDER BY rank
                   LIMIT ?3";
        conn.prepare(sql)?
            .query_map(params![fts_query, hier, args.limit], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .flatten()
            .collect()
    } else {
        let sql = "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy
                   FROM concepts_fts
                   JOIN concepts c ON concepts_fts.rowid = c.rowid
                   WHERE concepts_fts MATCH ?1
                   ORDER BY rank
                   LIMIT ?2";
        conn.prepare(sql)?
            .query_map(params![fts_query, args.limit], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .flatten()
            .collect()
    };

    // `--ids`: machine output for pipes - just SCTIDs on stdout, nothing else.
    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for (id, _, _, _) in &results {
            writeln!(out, "{id}")?;
        }
        return Ok(());
    }

    if results.is_empty() && !out.is_structured() {
        println!("No results for {:?}", args.query);
        return Ok(());
    }

    if out.is_structured() {
        let items: Vec<Value> = results
            .iter()
            .map(|(id, pt, fsn, hier)| {
                json!({ "id": id, "preferred_term": pt, "fsn": fsn, "hierarchy": hier })
            })
            .collect();
        let value = if show_prov {
            let mut v = json!({ "results": items });
            provenance::inject_into_json(&mut v, prov.as_ref(), true);
            v
        } else {
            Value::Array(items)
        };
        if let Some(s) = out.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    let format = ConceptFormat::load().with_overrides(args.template, args.template_fsn_suffix);
    for (id, preferred_term, fsn, hierarchy) in &results {
        println!(
            "{}",
            format.render(&ConceptFields {
                id,
                pt: preferred_term,
                fsn,
                hierarchy,
                ..Default::default()
            })
        );
    }

    provenance::print_human_footer(prov.as_ref(), show_prov);

    Ok(())
}

/// Sanitise an FTS5 query. If the string contains no FTS5 operator characters
/// we treat it as an implicit phrase (wrap in double quotes). This prevents
/// parse errors for queries like "heart attack" typed without quotes.
fn sanitise_fts_query(q: &str) -> String {
    let has_operators = q.contains('"')
        || q.contains('*')
        || q.contains('^')
        || q.to_uppercase().contains(" AND ")
        || q.to_uppercase().contains(" OR ")
        || q.to_uppercase().contains(" NOT ");
    if has_operators {
        q.to_string()
    } else {
        format!("\"{}\"", q.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_wrapped_in_quotes() {
        assert_eq!(sanitise_fts_query("heart attack"), "\"heart attack\"");
    }

    #[test]
    fn prefix_query_left_as_is() {
        assert_eq!(sanitise_fts_query("myocardial*"), "myocardial*");
    }

    #[test]
    fn boolean_query_left_as_is() {
        assert_eq!(sanitise_fts_query("heart AND attack"), "heart AND attack");
    }
}
