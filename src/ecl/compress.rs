// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Refactor an explicit set of SCTIDs into a compact ECL expression - the
//! inverse of [`crate::ecl::expand`]. See `spec/commands/ecl-compress.md`.
//!
//! Strategy (a greedy heuristic, not a proof of global minimality):
//!   0. if the target set is *exactly* some refset's active membership, cover it
//!      with a single `^refsetId` clause instead of any IS-A traversal;
//!   1. otherwise cover the set from above with `<<root` clauses over its
//!      maximal elements;
//!   2. carve the resulting over-inclusion back out with `MINUS <<x` clauses over
//!      the maximal *clean* elements (subtrees disjoint from the target set);
//!   3. guarantee exactness by re-expanding and appending literal `OR`/`MINUS`
//!      residuals for anything the intensional form still gets wrong.
//!
//! Correctness never depends on the heuristic's cleverness: the residual net in
//! step 3 makes the emitted expression provably reproduce the input. The
//! heuristic only decides how *compact* the result is.
//!
//! Straddling-exclusion push-down (`spec/commands/ecl-compress.md` §4.2 steps
//! 4-5): step 2 above scans *every* element of the over-inclusion `E`, not just
//! its top-level maximal elements, for whole-subtree disjointness from the
//! target. That already finds the deepest clean cut point beneath any
//! straddling ancestor in one pass, with no explicit recursion needed - a
//! "clean" exclusion is defined as fully subtree-disjoint from the target, so
//! it can never over-remove, and no OR-back is ever required. What step 2 *did*
//! lack is priority under `--max-exclusions`: candidates are now ranked by
//! subtree size (largest-cover-first, matching §4.4) before truncation, so a
//! tight bound keeps the exclusions that remove the most unwanted concepts
//! rather than an arbitrary subset.

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::ecl::eval::{ancestors_with_tct, descendants_or_self_with_tct, IdSet};

/// The outcome of compressing a set into ECL.
#[derive(Debug, Clone)]
pub(crate) struct CompressResult {
    /// The expression to emit. Exact (reproduces the input) when `exact` was
    /// requested; otherwise the intensional-only form.
    pub expr: String,
    /// Cover roots: either `<<root` ids (maximal elements of the input) or, if
    /// the input is exactly some refset's membership, that refset's single id
    /// (rendered as `^id` in `expr`).
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
    // already in ascending numeric SCTID order - the Vecs it feeds need no sort
    // (excludes are a partial exception - see step 3's size-ranked truncation).

    // 0. Refset-cover recognition: if `target` is *exactly* some refset's active
    // membership, a single `^refsetId` clause is a strictly tighter cover than
    // any IS-A traversal could produce, so skip straight to includes = [refset]
    // with no exclusions. Falls through to the normal steps 1-3 otherwise.
    let (includes, excludes, dropped_exclusions, intensional_expr) =
        if let Some(refset_id) = find_exact_refset_cover(conn, target)? {
            (vec![refset_id], Vec::new(), 0, format!("^{refset_id}"))
        } else {
            // 1. Include roots = maximal elements of the target (no proper
            // ancestor in it).
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

            // 3. Clean elements of E: subtrees wholly disjoint from the target,
            //    so `MINUS <<x` removes only unwanted concepts (see the module
            //    doc for why scanning all of `E`, not just its top-level maximal
            //    elements, already gives arbitrary-depth push-down). Keep the
            //    maximal ones, ranked largest-subtree-first so a
            //    `--max-exclusions` bound keeps the most impactful clauses.
            let mut clean_sizes: Vec<(u64, usize)> = Vec::new();
            for &x in &e {
                let subtree = descendants_or_self_with_tct(conn, x, tct)?;
                if subtree.is_disjoint(target) {
                    clean_sizes.push((x, subtree.len()));
                }
            }
            let clean: IdSet = clean_sizes.iter().map(|&(x, _)| x).collect();
            let mut ranked: Vec<(u64, usize)> = Vec::new();
            for &(x, size) in &clean_sizes {
                let anc = ancestors_with_tct(conn, x, tct)?;
                if anc.is_disjoint(&clean) {
                    ranked.push((x, size));
                }
            }
            ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let dropped_exclusions = ranked.len().saturating_sub(max_exclusions);
            ranked.truncate(max_exclusions);
            let mut excludes: Vec<u64> = ranked.into_iter().map(|(x, _)| x).collect();
            excludes.sort_unstable();

            let intensional_expr = build_intensional(&includes, &excludes);
            (includes, excludes, dropped_exclusions, intensional_expr)
        };

    // 4. Measure what the intensional expression gets wrong.
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

/// If `target` is exactly the active membership of some refset, return its id -
/// `^refsetId` is then a single, strictly tighter clause than any IS-A cover
/// could produce (`spec/commands/ecl-compress.md` §7 slice 3). Cardinality is
/// checked first via `list_refsets` (already grouped/counted per refset) so
/// only genuine candidates pay for a full membership fetch; ties - unlikely
/// outside adversarial/synthetic data - resolve to the lowest refset id.
fn find_exact_refset_cover(conn: &Connection, target: &IdSet) -> Result<Option<u64>> {
    let refsets = crate::refset::list_refsets(conn, None)?;
    let mut candidates: Vec<u64> = refsets
        .iter()
        .filter(|r| r.member_count as usize == target.len())
        .filter_map(|r| r.id.parse::<u64>().ok())
        .collect();
    candidates.sort_unstable();
    for refset_id in candidates {
        if &refset_member_ids(conn, refset_id)? == target {
            return Ok(Some(refset_id));
        }
    }
    Ok(None)
}

/// Active members of `refset_id` as an `IdSet`. Every row in `refset_members`
/// is already an active membership (see `Rf2Dataset::load`), so no `active`
/// filter is needed - mirrors `collect_members` in `crate::ecl::eval`.
fn refset_member_ids(conn: &Connection, refset_id: u64) -> Result<IdSet> {
    let mut stmt = conn.prepare_cached(
        "SELECT CAST(referenced_component_id AS INTEGER) FROM refset_members WHERE refset_id = ?1",
    )?;
    let rows = stmt.query_map([refset_id.to_string()], |r| r.get::<_, i64>(0))?;
    let mut out = IdSet::new();
    for r in rows {
        out.insert(r? as u64);
    }
    Ok(out)
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
            "CREATE TABLE concepts (id TEXT PRIMARY KEY, active INTEGER NOT NULL,
                 preferred_term TEXT, fsn TEXT, module TEXT);
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             CREATE TABLE refset_members (refset_id TEXT NOT NULL,
                 referenced_component_id TEXT NOT NULL,
                 PRIMARY KEY (refset_id, referenced_component_id));",
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

    /// A wider hierarchy for straddling-exclusion tests, extending `fixture()`'s
    /// shape with a deeper straddle (`6` → `8`,`9`) and two additional branches
    /// of different sizes (`20` alone, `21` → `22`,`23`,`24`) to exercise
    /// multi-level push-down and `--max-exclusions` prioritisation:
    ///   1 ── 2 ── 4
    ///     │    └─ 5
    ///     ├─ 3 ── 6 ── 8
    ///     │    │    └─ 9
    ///     │    └─ 7
    ///     ├─ 20
    ///     └─ 21 ── 22
    ///           ├─ 23
    ///           └─ 24
    fn fixture_wide() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (id TEXT PRIMARY KEY, active INTEGER NOT NULL,
                 preferred_term TEXT, fsn TEXT, module TEXT);
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             CREATE TABLE refset_members (refset_id TEXT NOT NULL,
                 referenced_component_id TEXT NOT NULL,
                 PRIMARY KEY (refset_id, referenced_component_id));",
        )
        .unwrap();
        for id in [
            "1", "2", "3", "4", "5", "6", "7", "8", "9", "20", "21", "22", "23", "24",
        ] {
            conn.execute("INSERT INTO concepts (id, active) VALUES (?1, 1)", [id])
                .unwrap();
        }
        for (c, p) in [
            ("2", "1"),
            ("3", "1"),
            ("20", "1"),
            ("21", "1"),
            ("4", "2"),
            ("5", "2"),
            ("6", "3"),
            ("7", "3"),
            ("8", "6"),
            ("9", "6"),
            ("22", "21"),
            ("23", "21"),
            ("24", "21"),
        ] {
            conn.execute(
                "INSERT INTO concept_isa (child_id, parent_id) VALUES (?1, ?2)",
                [c, p],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn deep_straddle_pushes_exclusion_to_clean_leaf() {
        let conn = fixture_wide();
        // Keep 7 (direct child of straddling 3) and 8 (grandchild of 3, via
        // straddling 6), but drop 9 and both straddling ancestors 3 and 6
        // themselves. Neither 3 nor 6 is a clean exclusion (their subtrees
        // still contain a wanted concept), but 9 is - proving the single-pass
        // scan over `E` already finds a clean cut point two levels below the
        // straddling root, with no explicit recursion (see the module doc).
        let target = set(&["1", "2", "4", "5", "7", "8"]);
        let r = compress(&conn, &target, 32, true, true).unwrap();
        assert!(r.exact);
        assert_eq!(expand(&conn, &r.expr), target);
        assert!(r.excludes.contains(&"9".to_string()));
        assert!(r.extra.contains(&"3".to_string()));
        assert!(r.extra.contains(&"6".to_string()));
    }

    #[test]
    fn max_exclusions_prioritises_largest_clean_subtree() {
        let conn = fixture_wide();
        // Keep everything under 1 except the 20/21 branch. 21's subtree (4
        // concepts) is a strictly better exclusion than 20's (1 concept) under
        // a tight bound - ranking by subtree size (not ascending id, under
        // which 20 would have been picked) keeps the more impactful one.
        let target = set(&["1", "2", "3", "4", "5", "6", "7", "8", "9"]);
        let r = compress(&conn, &target, 1, true, true).unwrap();
        assert_eq!(r.excludes, vec!["21".to_string()]);
        assert_eq!(r.dropped_exclusions, 1);
        assert!(r.exact);
        assert_eq!(expand(&conn, &r.expr), target);
        assert_eq!(r.extra, vec!["20".to_string()]);
    }

    fn insert_refset_member(conn: &Connection, refset_id: &str, concept_id: &str) {
        conn.execute(
            "INSERT INTO refset_members (refset_id, referenced_component_id) VALUES (?1, ?2)",
            [refset_id, concept_id],
        )
        .unwrap();
    }

    #[test]
    fn refset_membership_becomes_single_cover_clause() {
        let conn = fixture();
        for member in ["4", "5"] {
            insert_refset_member(&conn, "900000", member);
        }
        let target = set(&["4", "5"]);
        let r = compress(&conn, &target, 32, true, true).unwrap();
        assert_eq!(r.expr, "^900000");
        assert_eq!(r.includes, vec!["900000".to_string()]);
        assert!(r.excludes.is_empty());
        assert!(r.exact);
        assert_eq!(expand(&conn, &r.expr), target);
    }

    #[test]
    fn refset_cover_requires_exact_membership_match() {
        let conn = fixture();
        // Refset covers a strict superset of the target - the cardinality
        // mismatch alone must rule it out as a cover clause.
        for member in ["4", "5", "6"] {
            insert_refset_member(&conn, "900000", member);
        }
        let target = set(&["4", "5"]);
        let r = compress(&conn, &target, 32, true, true).unwrap();
        assert!(!r.expr.contains('^'));
        assert!(r.exact);
        assert_eq!(expand(&conn, &r.expr), target);
    }
}
