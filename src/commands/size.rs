// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct size` - Estimate NDJSON and SQLite file sizes for a concept subtree.
//!
//! Acts as a data-planning tool ("how big will `sct filter` output be?"):
//!
//! - Counts all concepts in the subtree (using the TCT when available).
//! - Samples N rows from the subtree, serialises each as JSON, and averages the
//!   byte length to estimate the NDJSON export size.
//! - Uses SQLite's `PRAGMA page_size` and `PRAGMA page_count` to estimate the
//!   proportional SQLite database size.
//! - Optionally prints a `du`-style tree of descendant counts with `--tree`.

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::Connection;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct Args {
    /// Starting concept ID. Defaults to the SNOMED CT root (138875005), or the
    /// single active root detected in the database for filtered/subset databases.
    #[arg(long, short)]
    pub concept: Option<String>,

    /// Number of rows to sample when estimating average NDJSON row size (default: 200).
    #[arg(long, short = 'n', default_value_t = 200)]
    pub sample: usize,

    /// Also print a `du`-style descendant count tree.
    #[arg(long, short = 't')]
    pub tree: bool,

    /// Maximum depth for the tree view (default: 2). Only used with --tree.
    #[arg(long, short = 'd', default_value_t = 2)]
    pub depth: usize,

    /// Path to the SNOMED CT SQLite database.
    #[arg(long)]
    pub db: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    let db_path = crate::paths::resolve_db(args.db.as_deref())?.path;

    // Open read-write so we can read PRAGMAs (query_only blocks PRAGMA page_count on older SQLite)
    let conn = Connection::open(&db_path)
        .with_context(|| format!("opening database {}", db_path.display()))?;
    conn.execute_batch("PRAGMA query_only = ON;")?;

    let has_tct = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='concept_ancestors'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    let start_concept = resolve_root(&conn, args.concept)?;

    // --- concept count ---
    let subtree_count = crate::commands::get_subtree_size(&conn, &start_concept)?;
    let total_count: u64 = conn
        .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0) as u64;

    let (preferred_term, _active): (String, i32) = conn
        .query_row(
            "SELECT preferred_term, active FROM concepts WHERE id = ?1",
            rusqlite::params![start_concept],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .with_context(|| format!("concept {} not found in database", start_concept))?;

    // --- NDJSON size estimation (sample average row size) ---
    let avg_ndjson_bytes = sample_avg_row_bytes(&conn, &start_concept, has_tct, args.sample)?;
    let ndjson_estimate = avg_ndjson_bytes * subtree_count;

    // --- SQLite size estimation (proportional page count) ---
    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0))
        .unwrap_or(4096) as u64;
    let page_count: u64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0) as u64;
    let total_db_bytes = page_size * page_count;
    let sqlite_estimate = if total_count > 0 {
        (total_db_bytes as f64 * subtree_count as f64 / total_count as f64) as u64
    } else {
        0
    };

    let pct = if total_count > 0 {
        subtree_count as f64 / total_count as f64 * 100.0
    } else {
        0.0
    };

    // --- Output ---
    println!();
    println!("Subtree: {} ({})", preferred_term, start_concept);
    println!(
        "Concepts: {}  ({:.1}% of {} total in database)",
        fmt_count(subtree_count),
        pct,
        fmt_count(total_count)
    );
    if !has_tct {
        eprintln!(
            "\nwarning: no transitive-closure table found — subtree count used a recursive CTE.\n\
             Build it once for fast estimates: `sct tct --db <db>`"
        );
    }
    println!();
    println!("{:<18} {:<16} Method", "Format", "Estimated size");
    println!("{}", "─".repeat(72));
    println!(
        "{:<18} {:<16} sampled avg {} B/row × {} rows",
        "NDJSON",
        fmt_bytes(ndjson_estimate),
        fmt_count(avg_ndjson_bytes),
        fmt_count(subtree_count)
    );
    println!(
        "{:<18} {:<16} proportional to full DB ({}) by concept count",
        "SQLite DB",
        fmt_bytes(sqlite_estimate),
        fmt_bytes(total_db_bytes)
    );
    println!();

    // --- Optional descendant count tree ---
    if args.tree {
        println!("Descendant Count Tree");
        println!("=====================");
        print_tree(
            &conn,
            &start_concept,
            &preferred_term,
            0,
            args.depth,
            "",
            true,
        )?;
        println!();
    }

    Ok(())
}

/// Resolve the starting concept: use the user's value, fall back to `138875005`,
/// then fall back to any active concept with no parents (for filtered databases).
fn resolve_root(conn: &Connection, concept: Option<String>) -> Result<String> {
    if let Some(id) = concept {
        return Ok(id);
    }
    let root_exists: bool = conn
        .query_row("SELECT 1 FROM concepts WHERE id = '138875005'", [], |_| {
            Ok(true)
        })
        .unwrap_or(false);
    if root_exists {
        return Ok("138875005".to_string());
    }
    // Filtered DB — find any active concept with an empty parents array
    let detected: Option<String> = conn
        .query_row(
            "SELECT id FROM concepts WHERE active = 1 AND (parents = '[]' OR parents IS NULL) LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(detected.unwrap_or_else(|| "138875005".to_string()))
}

/// Sample up to `limit` concepts from the subtree, serialise each row's text columns
/// as a JSON object (approximating a real NDJSON line), and return the average byte count.
fn sample_avg_row_bytes(
    conn: &Connection,
    root_id: &str,
    has_tct: bool,
    limit: usize,
) -> Result<u64> {
    // Columns that appear in a ConceptRecord NDJSON line
    let sql = if has_tct {
        format!(
            "SELECT id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
                    parents, children_count, attributes, active, module, effective_time,
                    ctv3_codes, read2_codes
             FROM concepts
             WHERE id IN (
                 SELECT descendant_id FROM concept_ancestors WHERE ancestor_id = '{root_id}'
                 UNION SELECT '{root_id}'
             )
             ORDER BY RANDOM()
             LIMIT {limit}"
        )
    } else {
        format!(
            "WITH RECURSIVE descendants(id) AS (
                 SELECT '{root_id}'
                 UNION
                 SELECT child_id FROM concept_isa JOIN descendants ON parent_id = id
             )
             SELECT c.id, c.fsn, c.preferred_term, c.synonyms, c.hierarchy, c.hierarchy_path,
                    c.parents, c.children_count, c.attributes, c.active, c.module, c.effective_time,
                    c.ctv3_codes, c.read2_codes
             FROM concepts c
             JOIN descendants d ON c.id = d.id
             ORDER BY RANDOM()
             LIMIT {limit}"
        )
    };

    let mut stmt = conn.prepare(&sql)?;

    // Measure the byte length of a minimal JSON serialisation of each row.
    // We build a small JSON object with the same keys sct ndjson would emit.
    let col_names = [
        "id",
        "fsn",
        "preferred_term",
        "synonyms",
        "hierarchy",
        "hierarchy_path",
        "parents",
        "children_count",
        "attributes",
        "active",
        "module",
        "effective_time",
        "ctv3_codes",
        "read2_codes",
    ];

    let mut total_bytes: u64 = 0;
    let mut sampled: u64 = 0;

    stmt.query_map([], |row| {
        // Build a JSON-like byte count: sum all text column lengths + key overhead
        let mut row_bytes: usize = 2; // outer braces {}
        for (i, name) in col_names.iter().enumerate() {
            row_bytes += name.len() + 4; // "key":  (quotes + colon + space)
            if let Ok(Some(val)) = row.get_ref(i).map(|v| v.as_str().ok()) {
                row_bytes += val.len() + 2; // value + surrounding quotes
            } else {
                row_bytes += 4; // null
            }
            if i + 1 < col_names.len() {
                row_bytes += 1; // comma
            }
        }
        row_bytes += 1; // newline at end of NDJSON line
        Ok(row_bytes as u64)
    })?
    .filter_map(|r| r.ok())
    .for_each(|bytes| {
        total_bytes += bytes;
        sampled += 1;
    });

    if sampled == 0 {
        return Ok(0);
    }
    Ok(total_bytes / sampled)
}

fn fmt_bytes(n: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = 1_024 * KB;
    const GB: u64 = 1_024 * MB;
    if n >= GB {
        format!("~{:.2} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("~{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("~{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("~{} B", n)
    }
}

fn fmt_count(n: u64) -> String {
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

fn print_tree(
    conn: &Connection,
    concept_id: &str,
    preferred_term: &str,
    depth: usize,
    max_depth: usize,
    prefix: &str,
    is_last: bool,
) -> Result<()> {
    let size = crate::commands::get_subtree_size(conn, concept_id)?;
    let node_str = format!(
        "{} [{}] ({} descendants)",
        preferred_term,
        concept_id,
        fmt_count(size.saturating_sub(1))
    );
    if depth == 0 {
        println!("{node_str}");
    } else {
        let connector = if is_last { "└── " } else { "├── " };
        println!("{prefix}{connector}{node_str}");
    }

    if depth >= max_depth {
        return Ok(());
    }

    let mut stmt = conn.prepare(
        "SELECT c.id, c.preferred_term
         FROM concept_isa i
         JOIN concepts c ON c.id = i.child_id
         WHERE i.parent_id = ?1 AND c.active = 1",
    )?;
    let children = stmt
        .query_map(rusqlite::params![concept_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut children_sizes: Vec<(String, String, u64)> = Vec::new();
    for (cid, term) in children {
        let sz = crate::commands::get_subtree_size(conn, &cid)?;
        children_sizes.push((cid, term, sz));
    }
    children_sizes.sort_by_key(|b| std::cmp::Reverse(b.2));

    let len = children_sizes.len();
    for (i, (cid, term, _)) in children_sizes.into_iter().enumerate() {
        let child_is_last = i == len - 1;
        let next_prefix = if depth == 0 {
            String::new()
        } else if is_last {
            format!("{prefix}    ")
        } else {
            format!("{prefix}│   ")
        };
        print_tree(
            conn,
            &cid,
            &term,
            depth + 1,
            max_depth,
            &next_prefix,
            child_is_last,
        )?;
    }
    Ok(())
}
