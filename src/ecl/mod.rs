// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SNOMED CT Expression Constraint Language (ECL).
//!
//! A parser and evaluator for the supported ECL subset (`spec/ecl.md`). ECL is
//! the intermediate representation the query stack converges on: it backs
//! `sct codelist add --ecl`, `sct serve` `$expand`, and is the
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
mod terms;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

pub use ast::Expr;
pub use eval::IdSet;
pub use parse::parse;

/// Canonical repair instruction shown by CLI and MCP callers when hierarchy
/// traversal must fall back to recursive CTEs.
pub const TCT_REPAIR_GUIDANCE: &str = "Build or repair it for a big speed-up: `sct tct --db <db>` (or use `sct sqlite --transitive-closure` when creating the database).";

/// Render the canonical unusable-TCT diagnostic for one affected operation.
pub fn tct_fallback_guidance(operation: &str) -> String {
    format!(
        "this database has no usable transitive-closure table, so {operation} uses slower recursive CTEs. {TCT_REPAIR_GUIDANCE}"
    )
}

/// Parse and evaluate an ECL expression against the database, returning the
/// matching SCTIDs as an [`IdSet`]. Prefer this over [`expand`] when the
/// caller does set algebra on the result - it skips the string formatting.
pub fn expand_set(conn: &Connection, ecl: &str) -> Result<IdSet> {
    let expr = parse(ecl).with_context(|| format!("parsing ECL {ecl:?}"))?;
    let _snapshot = eval::ReadSnapshot::begin(conn)?;
    eval::evaluate(conn, &expr).context("evaluating ECL")
}

#[cfg(feature = "cli")]
pub(crate) fn expand_set_with_tct(conn: &Connection, ecl: &str, tct: bool) -> Result<IdSet> {
    let expr = parse(ecl).with_context(|| format!("parsing ECL {ecl:?}"))?;
    eval::evaluate_with_tct(conn, &expr, tct).context("evaluating ECL")
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

/// Print a stderr hint when the database lacks a usable transitive-closure
/// table. This compatibility wrapper retains the original public API; command
/// adapters that need the status should use [`warn_if_tct_unusable`].
pub fn warn_if_no_tct(conn: &Connection) {
    if matches!(eval::has_tct(conn), Ok(false)) {
        warn_tct_fallback("transitive hierarchy evaluation");
    }
}

/// Check TCT usability and print the canonical stderr hint when `operation`
/// must use recursive CTEs. Pass the returned capability into the operation so
/// detection, diagnostics, and execution use the same decision.
pub fn warn_if_tct_unusable(conn: &Connection, operation: &str) -> Result<bool> {
    let usable = eval::has_tct(conn)?;
    if !usable {
        warn_tct_fallback(operation);
    }
    Ok(usable)
}

/// Print the canonical unusable-TCT guidance to stderr.
pub(crate) fn warn_tct_fallback(operation: &str) {
    eprintln!("note: {}", tct_fallback_guidance(operation));
}
