// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct size` - Estimate NDJSON and SQLite file sizes for a concept subtree.
//!
//! Acts as a data-planning tool ("how big will `sct filter` output be?"):
//!
//! - Counts all concepts in the subtree (using the TCT when available).
//! - Samples N rows and approximates each row's NDJSON byte length from its
//!   stored text columns, then averages, to estimate the export size. This is a
//!   deliberate lower bound: it excludes `refsets`, `relationships`, and
//!   `crossmaps` (which live in other tables), so a real `sct ndjson` line is
//!   somewhat larger.
//! - Uses SQLite's `PRAGMA page_size` and `PRAGMA page_count` to estimate the
//!   proportional SQLite database size.
//! - Optionally prints a `du`-style tree of descendant counts with `--tree`.
//!
//! The core estimation logic is exposed as [`estimate_sizes`] so the GUI and TUI
//! can reuse it without duplicating code.

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::Connection;
use serde_json::json;
use std::path::PathBuf;

use crate::output::OutputFormat;

/// Default number of rows sampled to estimate the average NDJSON row size.
/// Shared by the CLI default and the TUI/GUI callers so all three agree.
pub(crate) const DEFAULT_SAMPLE: usize = 200;

// ─── Public shared types ──────────────────────────────────────────────────────

/// File-size estimates for a concept subtree.
///
/// Produced by [`estimate_sizes`] and consumed by the CLI, TUI, and GUI.
#[derive(Debug, Clone)]
pub struct SizeEstimate {
    /// Number of concepts in the subtree (including the root concept itself).
    pub subtree_count: u64,
    /// Total number of concepts in the database.
    pub total_count: u64,
    /// Average NDJSON row size in bytes (from sampling).
    pub avg_ndjson_bytes: u64,
    /// Estimated total NDJSON export size in bytes.
    pub ndjson_total: u64,
    /// Total SQLite database size in bytes (page_size × page_count).
    pub total_db_bytes: u64,
    /// Estimated proportional SQLite size for this subtree in bytes.
    pub sqlite_total: u64,
}

impl SizeEstimate {
    /// Percentage of the full database represented by this subtree.
    pub fn pct(&self) -> f64 {
        if self.total_count > 0 {
            self.subtree_count as f64 / self.total_count as f64 * 100.0
        } else {
            0.0
        }
    }
}

/// Compute [`SizeEstimate`] for the subtree rooted at `root_id`.
///
/// `sample` controls how many rows are randomly sampled to derive the average
/// NDJSON row byte length. A value of 50–200 gives a good balance between speed
/// and accuracy. Requires an open `Connection`; does **not** open its own.
pub fn estimate_sizes(conn: &Connection, root_id: &str, sample: usize) -> Result<SizeEstimate> {
    let has_tct = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='concept_ancestors'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    let subtree_count = crate::commands::get_subtree_size(conn, root_id)?;
    let total_count: u64 = conn
        .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0) as u64;

    let avg_ndjson_bytes = sample_avg_row_bytes(conn, root_id, has_tct, sample)?;
    let ndjson_total = avg_ndjson_bytes * subtree_count;

    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0))
        .unwrap_or(4096) as u64;
    let page_count: u64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0) as u64;
    let total_db_bytes = page_size * page_count;
    let sqlite_total = if total_count > 0 {
        (total_db_bytes as f64 * subtree_count as f64 / total_count as f64) as u64
    } else {
        0
    };

    Ok(SizeEstimate {
        subtree_count,
        total_count,
        avg_ndjson_bytes,
        ndjson_total,
        total_db_bytes,
        sqlite_total,
    })
}

// ─── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
pub struct Args {
    /// Starting concept ID. Defaults to the SNOMED CT root (138875005), or the
    /// single active root detected in the database for filtered/subset databases.
    #[arg(long, short)]
    pub concept: Option<String>,

    /// Number of rows to sample when estimating average NDJSON row size.
    #[arg(long, short = 'n', default_value_t = DEFAULT_SAMPLE)]
    pub sample: usize,

    /// Also print a `du`-style descendant count tree (text output only).
    #[arg(long, short = 't')]
    pub tree: bool,

    /// Maximum depth for the tree view. Only used with --tree.
    #[arg(long, short = 'd', default_value_t = 2)]
    pub depth: usize,

    /// Output format. `--tree` is honoured for `text` only.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    let db_path = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = crate::commands::open_db_readonly(&db_path, None)?;

    let has_tct = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='concept_ancestors'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    let start_concept = resolve_root(&conn, args.concept)?;

    let (preferred_term, _active): (String, i32) = conn
        .query_row(
            "SELECT preferred_term, active FROM concepts WHERE id = ?1",
            rusqlite::params![start_concept],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .with_context(|| format!("concept {} not found in database", start_concept))?;

    let est = estimate_sizes(&conn, &start_concept, args.sample)?;

    // Structured output (`--format json`/`yaml`): emit the estimate and stop.
    // `--tree` is a text-only visualisation, so it is not rendered here.
    if args.format.is_structured() {
        let value = json!({
            "id": start_concept,
            "preferred_term": preferred_term,
            "has_tct": has_tct,
            "subtree_count": est.subtree_count,
            "total_count": est.total_count,
            "pct": est.pct(),
            "avg_ndjson_bytes": est.avg_ndjson_bytes,
            "ndjson_total_bytes": est.ndjson_total,
            "ndjson_human": fmt_bytes(est.ndjson_total),
            "total_db_bytes": est.total_db_bytes,
            "sqlite_total_bytes": est.sqlite_total,
            "sqlite_human": fmt_bytes(est.sqlite_total),
        });
        if let Some(s) = args.format.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    // --- Output ---
    println!();
    println!("Subtree: {} ({})", preferred_term, start_concept);
    println!(
        "Concepts: {}  ({:.1}% of {} total in database)",
        fmt_count(est.subtree_count),
        est.pct(),
        fmt_count(est.total_count)
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
        fmt_bytes(est.ndjson_total),
        fmt_count(est.avg_ndjson_bytes),
        fmt_count(est.subtree_count)
    );
    println!(
        "{:<18} {:<16} proportional to full DB ({}) by concept count",
        "SQLite DB",
        fmt_bytes(est.sqlite_total),
        fmt_bytes(est.total_db_bytes)
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

// ─── Private helpers ──────────────────────────────────────────────────────────

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
    let detected: Option<String> = conn
        .query_row(
            "SELECT id FROM concepts WHERE active = 1 AND (parents = '[]' OR parents IS NULL) LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(detected.unwrap_or_else(|| "138875005".to_string()))
}

/// Sample up to `limit` concepts from the subtree, approximate each row's NDJSON
/// byte length from its stored text columns, and return the average. This is a
/// lower bound: it omits `refsets` / `relationships` / `crossmaps` (separate
/// tables), so a real `sct ndjson` line is somewhat larger.
fn sample_avg_row_bytes(
    conn: &Connection,
    root_id: &str,
    has_tct: bool,
    limit: usize,
) -> Result<u64> {
    // `root_id` is user-controlled (`--concept`, and the GUI's `/api/size/:id`
    // path), so it is bound as a parameter, never interpolated. `?1` is reused
    // for both references in the TCT branch.
    let sql = if has_tct {
        "SELECT id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
                parents, children_count, attributes, active, module, effective_time,
                ctv3_codes, read2_codes
         FROM concepts
         WHERE id IN (
             SELECT descendant_id FROM concept_ancestors WHERE ancestor_id = ?1
             UNION SELECT ?1
         )
         ORDER BY RANDOM()
         LIMIT ?2"
    } else {
        "WITH RECURSIVE descendants(id) AS (
             SELECT ?1
             UNION
             SELECT child_id FROM concept_isa JOIN descendants ON parent_id = id
         )
         SELECT c.id, c.fsn, c.preferred_term, c.synonyms, c.hierarchy, c.hierarchy_path,
                c.parents, c.children_count, c.attributes, c.active, c.module, c.effective_time,
                c.ctv3_codes, c.read2_codes
         FROM concepts c
         JOIN descendants d ON c.id = d.id
         ORDER BY RANDOM()
         LIMIT ?2"
    };

    let mut stmt = conn.prepare(sql)?;

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

    stmt.query_map(rusqlite::params![root_id, limit as i64], |row| {
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

pub(crate) fn fmt_bytes(n: u64) -> String {
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

pub(crate) fn fmt_count(n: u64) -> String {
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
