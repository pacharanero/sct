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

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::{params, Connection};
use std::path::PathBuf;

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
}

pub fn run(args: Args) -> Result<()> {
    let conn = Connection::open(&args.db)
        .with_context(|| format!("opening database {}", args.db.display()))?;
    conn.execute_batch("PRAGMA query_only = ON;")?;

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

    println!(
        "{} result{} for {:?}{}:",
        results.len(),
        if results.len() == 1 { "" } else { "s" },
        args.query,
        args.hierarchy
            .as_deref()
            .map(|h| format!(" in \"{h}\""))
            .unwrap_or_default()
    );
    println!();

    for (id, preferred_term, fsn, hierarchy) in &results {
        println!("  [{id}] {preferred_term}");
        if preferred_term != fsn {
            // Strip semantic tag from FSN for cleaner display
            let fsn_clean = fsn.rfind(" (").map(|p| &fsn[..p]).unwrap_or(fsn.as_str());
            if fsn_clean != preferred_term {
                println!("        FSN: {fsn_clean}");
            }
        }
        println!("        {hierarchy}");
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
