//! `sct lexical` — Full-text keyword search over a SNOMED CT SQLite database.
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

use crate::format::{ConceptFields, ConceptFormat};

#[derive(Parser, Debug)]
pub struct Args {
    /// Search query (FTS5 syntax: phrases, prefix*, boolean AND/OR/NOT).
    pub query: String,

    /// SQLite database produced by `sct sqlite`.
    #[arg(long, default_value = "snomed.db")]
    pub db: PathBuf,

    /// Restrict results to a specific top-level hierarchy (e.g. "Clinical finding").
    #[arg(long)]
    pub hierarchy: Option<String>,

    /// Maximum number of results to return.
    #[arg(long, short, default_value = "10")]
    pub limit: u32,

    /// Override the per-concept line template. See `docs/commands/refset.md`
    /// for the variable list.
    #[arg(long)]
    pub format: Option<String>,

    /// Override the FSN suffix template (rendered only when FSN differs from PT).
    #[arg(long)]
    pub format_fsn_suffix: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    let conn = crate::commands::open_db_readonly(&args.db, None)?;

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

    if results.is_empty() {
        println!("No results for {:?}", args.query);
        return Ok(());
    }

    let format = ConceptFormat::load().with_overrides(args.format, args.format_fsn_suffix);
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
