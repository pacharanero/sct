// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Refactor an explicit set of SCTIDs into a compact ECL expression - the
//! inverse of [`crate::ecl::expand`]. See `spec/commands/ecl-compress.md`.
//!
//! Strategy (a greedy heuristic, not a proof of global minimality):
//!   1. cover the set from above with `<<root` clauses over its maximal elements;
//!   2. carve the resulting over-inclusion back out with `MINUS <<x` clauses over
//!      the maximal *clean* elements (subtrees disjoint from the target set);
//!   3. guarantee exactness by re-expanding and appending literal `OR`/`MINUS`
//!      residuals for anything the intensional form still gets wrong.
//!
//! Correctness never depends on the heuristic's cleverness: the residual net in
//! step 3 makes the emitted expression provably reproduce the input. The
//! heuristic only decides how *compact* the result is.

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::ecl::eval::{ancestors_with_tct, descendants_or_self_with_tct, IdSet};

/// The outcome of compressing a set into ECL.
#[derive(Debug, Clone)]
pub(crate) struct CompressResult {
    /// The expression to emit. Exact (reproduces the input) when `exact` was
    /// requested; otherwise the intensional-only form.
    pub expr: String,
    /// `<<root` include roots (maximal elements of the input).
    pub includes: Vec<String>,
    /// `MINUS <<x` exclusion roots (maximal clean elements of the over-inclusion).
    pub excludes: Vec<String>,
    /// Input members the intensional form failed to include (→ `OR id` residuals).
    pub missing: Vec<String>,
    /// Concepts the intensional form wrongly included (→ `MINUS id` residuals).
    pub extra: Vec<String>,
    /// Fraction of the input expressed intensionally, in `[0, 100]`.
    pub coverage: f64,
    /// Whether `expr` was verified to reproduce the input set exactly.
    pub exact: bool,
    /// Clean exclusion roots dropped because `--max-exclusions` was hit; their
    /// subtrees fall through to `extra` residuals. Purely informational.
    pub dropped_exclusions: usize,
}

/// Compress `target` (a non-empty set of active SCTIDs) into ECL.
///
/// `max_exclusions` bounds the number of `MINUS <<x` clauses before the
/// remainder is handed to the residual net. When `exact` is true the returned
/// `expr` includes literal residuals and (unless `verify` is false) is checked
/// by re-expansion; when false, `expr` is the intensional-only form.
#[cfg(test)]
pub(crate) fn compress(
    conn: &Connection,
    target: &IdSet,
    max_exclusions: usize,
    exact: bool,
    verify: bool,
) -> Result<CompressResult> {
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn)?;
    let tct = crate::ecl::eval::has_tct(conn)?;
    compress_with_tct(conn, target, max_exclusions, exact, verify, tct)
}

pub(crate) fn compress_with_tct(
    conn: &Connection,
    target: &IdSet,
    max_exclusions: usize,
    exact: bool,
    verify: bool,
    tct: bool,
) -> Result<CompressResult> {
    anyhow::ensure!(!target.is_empty(), "cannot compress an empty set");

    // Ordering note: `IdSet` is a `BTreeSet<u64>`, so every iteration below is
    // already in ascending numeric SCTID order - the Vecs it feeds need no sort.

    // 1. Include roots = maximal elements of the target (no proper ancestor in it).
    let mut includes: Vec<u64> = Vec::new();
    for &c in target {
        let anc = ancestors_with_tct(conn, c, tct)?;
        if anc.is_disjoint(target) {
            includes.push(c);
        }
    }

    // 2. Cover from above, then the over-inclusion E = cover \ target.
    let mut cover = IdSet::new();
    for &m in &includes {
        cover.extend(descendants_or_self_with_tct(conn, m, tct)?);
    }
    let e: IdSet = cover.difference(target).copied().collect();

    // 3. Clean elements of E: subtrees wholly disjoint from the target, so
    //    `MINUS <<x` removes only unwanted concepts. Then keep the maximal ones.
    let mut clean = IdSet::new();
    for &x in &e {
        if descendants_or_self_with_tct(conn, x, tct)?.is_disjoint(target) {
            clean.insert(x);
        }
    }
    let mut excludes: Vec<u64> = Vec::new();
    for &x in &clean {
        let anc = ancestors_with_tct(conn, x, tct)?;
        if anc.is_disjoint(&clean) {
            excludes.push(x);
        }
    }
    let dropped_exclusions = excludes.len().saturating_sub(max_exclusions);
    excludes.truncate(max_exclusions);

    // 4. Build the intensional expression and measure what it gets wrong.
    let intensional_expr = build_intensional(&includes, &excludes);
    let produced: IdSet = crate::ecl::expand_set_with_tct(conn, &intensional_expr, tct)
        .context("re-expanding the intensional expression for verification")?;
    let missing: Vec<u64> = target.difference(&produced).copied().collect();
    let extra: Vec<u64> = produced.difference(target).copied().collect();

    let coverage = (target.len() - missing.len()) as f64 / target.len() as f64 * 100.0;

    // 5. Exactness. In exact mode, append literal residuals and (optionally)
    //    verify the round-trip. In intensional-only mode, `expr` is the bare
    //    intensional form and `exact` reflects whether it already matched.
    let (expr, verified_exact) = if exact {
        let e = append_residuals(&intensional_expr, &missing, &extra);
        let ok = if verify {
            let check: IdSet = crate::ecl::expand_set_with_tct(conn, &e, tct)
                .context("verifying the compressed expression")?;
            &check == target
        } else {
            true
        };
        (e, ok)
    } else {
        (
            intensional_expr.clone(),
            missing.is_empty() && extra.is_empty(),
        )
    };

    Ok(CompressResult {
        expr,
        includes: to_strings(&includes),
        excludes: to_strings(&excludes),
        missing: to_strings(&missing),
        extra: to_strings(&extra),
        coverage,
        exact: verified_exact,
        dropped_exclusions,
    })
}

/// Render SCTIDs for the string-facing [`CompressResult`] fields.
fn to_strings(ids: &[u64]) -> Vec<String> {
    ids.iter().map(u64::to_string).collect()
}

/// `<<a` for one root, `(<<a OR <<b …)` for several, then ` MINUS <<x` per
/// exclusion. The parenthesised include group keeps `MINUS` (which binds tighter
/// than `OR` in this parser) from associating with only the last include.
fn build_intensional(includes: &[u64], excludes: &[u64]) -> String {
    let inc = if includes.len() == 1 {
        format!("<<{}", includes[0])
    } else {
        let parts: Vec<String> = includes.iter().map(|i| format!("<<{i}")).collect();
        format!("({})", parts.join(" OR "))
    };
    let mut expr = inc;
    for x in excludes {
        expr = format!("{expr} MINUS <<{x}");
    }
    expr
}

/// Force exactness: `OR id` re-adds a missing member, `MINUS id` removes a
/// wrongly-included one. The whole expression is parenthesised before the
/// `MINUS` residuals so the final subtractions apply to the entire set rather
/// than binding to the nearest `OR` term (`MINUS` binds tighter than `OR`).
fn append_residuals(base: &str, missing: &[u64], extra: &[u64]) -> String {
    let mut expr = base.to_string();
    for m in missing {
        expr = format!("{expr} OR {m}");
    }
    if !extra.is_empty() {
        expr = format!("({expr})");
        for x in extra {
            expr = format!("{expr} MINUS {x}");
        }
    }
    expr
}

/// Render `expr` across multiple indented lines by breaking at each top-level
/// `OR` / `MINUS`. Purely cosmetic - the token stream (and therefore the parse)
/// is unchanged.
pub(crate) fn prettify(expr: &str) -> String {
    expr.replace(" MINUS ", "\n  MINUS ")
        .replace(" OR ", "\n  OR ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// A small hierarchy:
    ///   1 ── 2 ── 4
    ///     │    └─ 5
    ///     └─ 3 ── 6
    ///          └─ 7
    /// plus an unrelated leaf 100.
    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (id TEXT PRIMARY KEY, active INTEGER NOT NULL);
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);",
        )
        .unwrap();
        for id in ["1", "2", "3", "4", "5", "6", "7", "100"] {
            conn.execute("INSERT INTO concepts (id, active) VALUES (?1, 1)", [id])
                .unwrap();
        }
        for (c, p) in [
            ("2", "1"),
            ("3", "1"),
            ("4", "2"),
            ("5", "2"),
            ("6", "3"),
            ("7", "3"),
        ] {
            conn.execute(
                "INSERT INTO concept_isa (child_id, parent_id) VALUES (?1, ?2)",
                [c, p],
            )
            .unwrap();
        }
        conn
    }

    fn set(ids: &[&str]) -> IdSet {
        ids.iter().map(|s| s.parse().unwrap()).collect()
    }

    fn expand(conn: &Connection, expr: &str) -> IdSet {
        crate::ecl::expand_set(conn, expr).unwrap()
    }

    #[test]
    fn pure_subtree_is_single_root() {
        let conn = fixture();
        let target = set(&["1", "2", "3", "4", "5", "6", "7"]);
        let r = compress(&conn, &target, 32, true, true).unwrap();
        assert_eq!(r.expr, "<<1");
        assert!(r.excludes.is_empty());
        assert!(r.missing.is_empty() && r.extra.is_empty());
        assert!(r.exact);
    }

    #[test]
    fn subtree_minus_subtree() {
        let conn = fixture();
        // Everything under 1 except the whole "3" subtree.
        let target = set(&["1", "2", "4", "5"]);
        let r = compress(&conn, &target, 32, true, true).unwrap();
        assert_eq!(r.includes, vec!["1"]);
        assert_eq!(r.excludes, vec!["3"]);
        assert_eq!(r.expr, "<<1 MINUS <<3");
        assert!(r.exact);
        assert_eq!(expand(&conn, &r.expr), target);
    }

    #[test]
    fn unrelated_leaf_becomes_its_own_include_root() {
        let conn = fixture();
        // 100 is unrelated, but it is a maximal element of the target, so it is
        // captured cleanly as `<<100` rather than needing a literal residual.
        let target = set(&["2", "4", "5", "100"]);
        let r = compress(&conn, &target, 32, true, true).unwrap();
        assert!(r.includes.contains(&"100".to_string()));
        assert!(r.missing.is_empty() && r.extra.is_empty());
        assert!(r.exact);
        assert_eq!(expand(&conn, &r.expr), target);
    }

    #[test]
    fn straddling_exclusion_stays_exact_via_residual() {
        let conn = fixture();
        // Under 1, drop 3 itself but keep its child 7: `<<3` cannot be excluded
        // (it would remove 7), so 3 survives as a literal `MINUS 3` residual.
        let target = set(&["1", "2", "4", "5", "7"]);
        let r = compress(&conn, &target, 32, true, true).unwrap();
        assert!(r.exact);
        assert_eq!(expand(&conn, &r.expr), target);
        assert!(r.extra.contains(&"3".to_string()));
        assert!(r.expr.contains("MINUS 3"));
    }

    #[test]
    fn intensional_only_reports_inexact() {
        let conn = fixture();
        // Same straddle: the intensional form cannot exclude the unwanted 3.
        let target = set(&["1", "2", "4", "5", "7"]);
        let r = compress(&conn, &target, 32, false, true).unwrap();
        assert!(!r.exact);
        assert!(r.extra.contains(&"3".to_string()));
        // No literal residual in the emitted (intensional-only) expression.
        assert!(!r.expr.contains("MINUS 3"));
    }

    #[test]
    fn structureless_set_round_trips() {
        let conn = fixture();
        // Two unrelated leaves with no shared clean subtree structure.
        let target = set(&["4", "100"]);
        let r = compress(&conn, &target, 32, true, true).unwrap();
        assert!(r.exact);
        assert_eq!(expand(&conn, &r.expr), target);
    }

    #[test]
    fn max_exclusions_bound_respected_and_exact() {
        let conn = fixture();
        // Force two clean exclusions (drop subtrees 4-only-sibling and 6/7),
        // but allow only one; the other must survive as residual, still exact.
        let target = set(&["1", "2", "4"]); // excludes 5, and the whole 3 subtree
        let r = compress(&conn, &target, 1, true, true).unwrap();
        assert!(r.excludes.len() <= 1);
        assert!(r.exact);
        assert_eq!(expand(&conn, &r.expr), target);
    }
}
