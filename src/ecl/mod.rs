// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SNOMED CT Expression Constraint Language (ECL).
//!
//! A parser and evaluator for the supported ECL subset (`spec/ecl.md`). ECL is
//! the intermediate representation the query stack converges on: it backs
//! `sct codelist add --ecl`, the future `sct serve` `$expand`, and is the
//! compile target for SCT-QL.
//!
//! - [`parse()`] - ECL text → [`ast::Expr`]
//! - [`eval::evaluate()`] - [`ast::Expr`] × SQLite → set of matching SCTIDs
//! - [`expand()`] - convenience: ECL text × SQLite → sorted `Vec` of SCTIDs

pub mod ast;
#[cfg(feature = "cli")]
pub mod compress;
pub mod eval;
pub mod lex;
pub mod parse;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

pub use ast::Expr;
pub use eval::IdSet;
pub use parse::parse;

/// Parse and evaluate an ECL expression against the database, returning the
/// matching SCTIDs as an [`IdSet`]. Prefer this over [`expand`] when the
/// caller does set algebra on the result - it skips the string formatting.
pub fn expand_set(conn: &Connection, ecl: &str) -> Result<IdSet> {
    let expr = parse(ecl).with_context(|| format!("parsing ECL {ecl:?}"))?;
    eval::evaluate(conn, &expr).context("evaluating ECL")
}

/// Parse and evaluate an ECL expression against the database, returning the
/// matching concept SCTIDs (ascending, deduplicated).
pub fn expand(conn: &Connection, ecl: &str) -> Result<Vec<String>> {
    // IdSet is a BTreeSet<u64>, so iteration is already in ascending numeric
    // SCTID order - formatting is the only work left.
    Ok(expand_set(conn, ecl)?
        .into_iter()
        .map(|id| id.to_string())
        .collect())
}

/// Open a SNOMED CT SQLite database read-only and [`expand`] an ECL expression
/// against it. Convenience for callers that have a path rather than a live
/// connection (e.g. integration tests).
pub fn expand_path(db: &Path, ecl: &str) -> Result<Vec<String>> {
    let conn = Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {} read-only", db.display()))?;
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA mmap_size = 2147483648;")
        .context("configuring read-only database")?;
    expand(&conn, ecl)
}

/// Print a one-line stderr hint when the database lacks the transitive-closure
/// table (`concept_ancestors`). Without it, large `<<` / `>>` ECL evaluation
/// falls back to recursive CTEs and is much slower. Call from command entry
/// points that run ECL (`sct ecl`, `sct serve`, `sct codelist add --ecl`).
pub fn warn_if_no_tct(conn: &Connection) {
    let has_tct = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='concept_ancestors'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !has_tct {
        eprintln!(
            "note: this database has no transitive-closure table, so large `<<` / `>>` ECL \
             queries fall back to slower recursive CTEs.\n  Build it once for a big speed-up: \
             `sct sqlite --transitive-closure` (or `sct tct --db <db>`)."
        );
    }
}
