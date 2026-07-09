// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct size` - View size of SNOMED CT concepts and their subtree distributions.

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::Connection;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct Args {
    /// Starting concept ID (default is the root concept "138875005").
    #[arg(long, short, default_value = "138875005")]
    pub concept: String,

    /// Maximum depth to print in the tree representation (default 2).
    #[arg(long, short, default_value_t = 2)]
    pub depth: usize,

    /// Path to the SNOMED CT SQLite database.
    #[arg(long)]
    pub db: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    let db_path = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = crate::commands::open_db_readonly(&db_path, None)?;

    // Lookup starting concept info
    let (term, active): (String, i32) = conn
        .query_row(
            "SELECT preferred_term, active FROM concepts WHERE id = ?1",
            rusqlite::params![args.concept],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .with_context(|| format!("concept {} not found in database", args.concept))?;

    if active == 0 {
        eprintln!("Warning: Starting concept {} is inactive.", args.concept);
    }

    println!("\nConcept Subtree Size Tree");
    println!("=========================");
    print_tree(&conn, &args.concept, &term, 0, args.depth, "", true)?;

    Ok(())
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

    // Format this node line
    let node_str = format!(
        "{} [{}] ({} descendants)",
        preferred_term,
        concept_id,
        fmt_count(size.saturating_sub(1))
    );
    if depth == 0 {
        println!("{}", node_str);
    } else {
        let connector = if is_last { "└── " } else { "├── " };
        println!("{}{}{}", prefix, connector, node_str);
    }

    if depth >= max_depth {
        return Ok(());
    }

    // Get children of this concept
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

    // Gather subtree sizes for sorting
    let mut children_sizes = Vec::new();
    for (cid, term) in children {
        let sz = crate::commands::get_subtree_size(conn, &cid)?;
        children_sizes.push((cid, term, sz));
    }
    children_sizes.sort_by_key(|b| std::cmp::Reverse(b.2)); // Sort descending by size

    let len = children_sizes.len();
    for (i, (cid, term, _)) in children_sizes.into_iter().enumerate() {
        let next_prefix = if depth == 0 {
            ""
        } else if is_last {
            &format!("{}    ", prefix)
        } else {
            &format!("{}│   ", prefix)
        };
        print_tree(
            conn,
            &cid,
            &term,
            depth + 1,
            max_depth,
            next_prefix,
            i == len - 1,
        )?;
    }

    Ok(())
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
