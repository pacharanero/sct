// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Evaluate an ECL [`Expr`] against a SNOMED CT SQLite database, returning the
//! set of matching concept SCTIDs. See `spec/ecl.md` §6.
//!
//! Set algebra runs in Rust over `BTreeSet<u64>`; hierarchy and refset
//! membership are pulled from SQLite, using indexed transitive closure where
//! available and recursive CTEs otherwise. Whole-AST SQL compilation for very
//! large result sets is a documented future optimisation.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::BTreeSet;

use crate::ecl::ast::{BoolOp, Expr, Op, Refinement};

/// A set of concept SCTIDs.
///
/// SCTIDs are 6-18 digit numbers, so the whole valid range fits in `u64` with
/// an order of magnitude to spare. Keeping the set algebra over integers
/// instead of `String`s roughly halves large-expansion cost (profiled on a
/// 136k-concept `<<404684003`: ~46% of instructions were the numeric-aware
/// string sort, ~11% string `BTreeSet` inserts) and makes the natural
/// iteration order *numeric*, so results need no post-sort. IDs are parsed
/// once at the SQL/AST boundary and formatted once at the output boundary.
pub type IdSet = BTreeSet<u64>;

/// Hold a stable SQLite read snapshot across a capability probe and the query
/// selected from it. Nested callers reuse an existing transaction.
pub(crate) struct ReadSnapshot<'a> {
    conn: &'a Connection,
    started: bool,
}

impl<'a> ReadSnapshot<'a> {
    pub(crate) fn begin(conn: &'a Connection) -> Result<Self> {
        let started = conn.is_autocommit();
        if started {
            conn.execute_batch("BEGIN DEFERRED TRANSACTION")
                .context("starting a stable database read snapshot")?;
        }
        Ok(Self { conn, started })
    }
}

impl Drop for ReadSnapshot<'_> {
    fn drop(&mut self) {
        if self.started {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

/// Parse a concept literal into an SCTID. SCTIDs are 6-18 digit numbers;
/// anything unparseable as `u64` cannot be a valid SCTID.
pub(crate) fn parse_sctid(id: &str) -> Result<u64> {
    id.parse::<u64>()
        .with_context(|| format!("invalid SCTID {id:?} (SCTIDs are 6-18 digit numbers)"))
}

/// Evaluate an ECL expression against `conn`.
pub fn evaluate(conn: &Connection, expr: &Expr) -> Result<IdSet> {
    let _snapshot = ReadSnapshot::begin(conn)?;
    let mut tct = None;
    eval_expr(conn, expr, &mut tct)
}

#[cfg(feature = "cli")]
pub(crate) fn evaluate_with_tct(conn: &Connection, expr: &Expr, tct: bool) -> Result<IdSet> {
    let mut status = Some(tct);
    eval_expr(conn, expr, &mut status)
}

#[cfg(feature = "cli")]
pub(crate) fn uses_transitive_hierarchy(expr: &Expr) -> bool {
    match expr {
        Expr::Wildcard | Expr::Concept(_) => false,
        Expr::Op(op, inner) => {
            matches!(
                op,
                Op::DescendantOf | Op::DescendantOrSelfOf | Op::AncestorOf | Op::AncestorOrSelfOf
            ) || uses_transitive_hierarchy(inner)
        }
        Expr::Bool(_, left, right) => {
            uses_transitive_hierarchy(left) || uses_transitive_hierarchy(right)
        }
        Expr::Refined(focus, refinement) => {
            uses_transitive_hierarchy(focus) || refinement_uses_transitive_hierarchy(refinement)
        }
    }
}

#[cfg(feature = "cli")]
fn refinement_uses_transitive_hierarchy(refinement: &Refinement) -> bool {
    match refinement {
        Refinement::And(left, right) | Refinement::Or(left, right) => {
            refinement_uses_transitive_hierarchy(left)
                || refinement_uses_transitive_hierarchy(right)
        }
        Refinement::Group(inner) => refinement_uses_transitive_hierarchy(inner),
        Refinement::Attr { attr, value, .. } => {
            uses_transitive_hierarchy(attr) || uses_transitive_hierarchy(value)
        }
    }
}

fn eval_expr(conn: &Connection, expr: &Expr, tct: &mut Option<bool>) -> Result<IdSet> {
    match expr {
        Expr::Wildcard => all_concepts(conn),
        Expr::Concept(id) => Ok(std::iter::once(parse_sctid(id)?).collect()),
        Expr::Op(op, inner) => {
            let base = eval_expr(conn, inner, tct)?;
            eval_op(conn, *op, &base, tct)
        }
        Expr::Bool(op, a, b) => {
            let sa = eval_expr(conn, a, tct)?;
            let sb = eval_expr(conn, b, tct)?;
            Ok(match op {
                BoolOp::And => sa.intersection(&sb).copied().collect(),
                BoolOp::Or => sa.union(&sb).copied().collect(),
                BoolOp::Minus => sa.difference(&sb).copied().collect(),
            })
        }
        Expr::Refined(focus, refinement) => {
            let f = eval_expr(conn, focus, tct)?;
            eval_refinement(conn, &f, refinement, tct)
        }
    }
}

/// Build an [`IdSet`] from an unordered, possibly-duplicated collection of
/// ids. Sorting + deduplicating first lets `BTreeSet::from_iter` take its
/// bulk-build path (one pass over sorted input) instead of a b-tree descent
/// per element - measurably cheaper for the 100k+ row collections the
/// collectors below produce.
fn set_from(mut v: Vec<u64>) -> IdSet {
    v.sort_unstable();
    v.dedup();
    v.into_iter().collect()
}

fn all_concepts(conn: &Connection) -> Result<IdSet> {
    // The id column is TEXT; CAST lets SQLite hand back an integer directly,
    // so no per-row String crosses the FFI boundary.
    let mut stmt = conn
        .prepare_cached("SELECT CAST(id AS INTEGER) FROM concepts WHERE active = 1")
        .context("preparing wildcard query")?;
    let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r? as u64);
    }
    Ok(set_from(out))
}

fn eval_op(
    conn: &Connection,
    op: Op,
    base: &IdSet,
    tct_status: &mut Option<bool>,
) -> Result<IdSet> {
    let mut out = Vec::new();
    match op {
        Op::DescendantOf | Op::DescendantOrSelfOf => {
            let tct = tct_status_or_probe(conn, tct_status)?;
            for &id in base {
                collect_transitive(conn, id, true, tct, &mut out)?;
            }
            if op == Op::DescendantOrSelfOf {
                out.extend(base.iter().copied());
            }
        }
        Op::AncestorOf | Op::AncestorOrSelfOf => {
            let tct = tct_status_or_probe(conn, tct_status)?;
            for &id in base {
                collect_transitive(conn, id, false, tct, &mut out)?;
            }
            if op == Op::AncestorOrSelfOf {
                out.extend(base.iter().copied());
            }
        }
        Op::ChildOf => {
            for &id in base {
                collect_one_hop(conn, id, true, &mut out)?;
            }
        }
        Op::ParentOf => {
            for &id in base {
                collect_one_hop(conn, id, false, &mut out)?;
            }
        }
        Op::MemberOf => {
            for &id in base {
                collect_members(conn, id, &mut out)?;
            }
        }
    }
    Ok(set_from(out))
}

fn tct_status_or_probe(conn: &Connection, status: &mut Option<bool>) -> Result<bool> {
    if let Some(usable) = *status {
        return Ok(usable);
    }
    let usable = has_tct(conn)?;
    *status = Some(usable);
    Ok(usable)
}

pub(crate) const TCT_SCHEMA_VERSION: i64 = 1;
#[cfg(any(feature = "cli", test))]
pub(crate) const TCT_INVALIDATION_TRIGGERS_SQL: &str = "CREATE TRIGGER tct_invalidate_isa_insert
         AFTER INSERT ON concept_isa
         BEGIN UPDATE concept_ancestors_meta SET schema_version = 0; END;
     CREATE TRIGGER tct_invalidate_isa_update
         AFTER UPDATE ON concept_isa
         BEGIN UPDATE concept_ancestors_meta SET schema_version = 0; END;
     CREATE TRIGGER tct_invalidate_isa_delete
         AFTER DELETE ON concept_isa
         BEGIN UPDATE concept_ancestors_meta SET schema_version = 0; END;
     CREATE TRIGGER tct_invalidate_concepts_insert
         AFTER INSERT ON concepts
         BEGIN UPDATE concept_ancestors_meta SET schema_version = 0; END;
     CREATE TRIGGER tct_invalidate_concepts_update
         AFTER UPDATE OF id ON concepts
         BEGIN UPDATE concept_ancestors_meta SET schema_version = 0; END;
     CREATE TRIGGER tct_invalidate_concepts_delete
         AFTER DELETE ON concepts
         BEGIN UPDATE concept_ancestors_meta SET schema_version = 0; END;
     CREATE TRIGGER tct_invalidate_ca_insert
         AFTER INSERT ON concept_ancestors
         BEGIN UPDATE concept_ancestors_meta SET schema_version = 0; END;
     CREATE TRIGGER tct_invalidate_ca_update
         AFTER UPDATE ON concept_ancestors
         BEGIN UPDATE concept_ancestors_meta SET schema_version = 0; END;
     CREATE TRIGGER tct_invalidate_ca_delete
         AFTER DELETE ON concept_ancestors
         BEGIN UPDATE concept_ancestors_meta SET schema_version = 0; END;";

/// Whether a usable precomputed transitive-closure table (`concept_ancestors`,
/// built by `sct sqlite --transitive-closure` / `sct tct`) is present. The
/// completion marker is published in the same transaction as the table, rows,
/// indexes, and invalidation triggers, so partial or subsequently modified
/// builds fail closed instead of returning incomplete hierarchy results. When
/// usable, `<<`/`>>` are indexed lookups instead of recursive CTEs. See
/// [`warn_if_no_tct`](crate::ecl::warn_if_no_tct).
pub(crate) fn has_tct(conn: &Connection) -> Result<bool> {
    Ok(has_tct_completion_marker(conn)? && has_tct_indexes(conn)?)
}

pub(crate) fn has_tct_completion_marker(conn: &Connection) -> Result<bool> {
    if !has_tct_table_schema(conn)?
        || !has_tct_meta_schema(conn)?
        || !has_tct_invalidation_triggers(conn)?
    {
        return Ok(false);
    }

    conn.query_row(
        "SELECT COUNT(*) = 1
             AND MIN(schema_version) = ?1
             AND MIN(include_self) IN (0, 1)
         FROM concept_ancestors_meta",
        [TCT_SCHEMA_VERSION],
        |row| row.get(0),
    )
    .context("checking the transitive-closure completion marker")
}

fn has_tct_invalidation_triggers(conn: &Connection) -> Result<bool> {
    const EXPECTED: [(&str, &str, &str); 9] = [
        ("tct_invalidate_isa_insert", "concept_isa", "insert"),
        ("tct_invalidate_isa_update", "concept_isa", "update"),
        ("tct_invalidate_isa_delete", "concept_isa", "delete"),
        ("tct_invalidate_concepts_insert", "concepts", "insert"),
        ("tct_invalidate_concepts_update", "concepts", "update of id"),
        ("tct_invalidate_concepts_delete", "concepts", "delete"),
        ("tct_invalidate_ca_insert", "concept_ancestors", "insert"),
        ("tct_invalidate_ca_update", "concept_ancestors", "update"),
        ("tct_invalidate_ca_delete", "concept_ancestors", "delete"),
    ];

    let mut stmt = conn
        .prepare(
            "SELECT name, tbl_name, sql
             FROM sqlite_master
             WHERE type = 'trigger'
               AND name IN (
                   'tct_invalidate_isa_insert',
                   'tct_invalidate_isa_update',
                   'tct_invalidate_isa_delete',
                   'tct_invalidate_concepts_insert',
                   'tct_invalidate_concepts_update',
                   'tct_invalidate_concepts_delete',
                   'tct_invalidate_ca_insert',
                   'tct_invalidate_ca_update',
                   'tct_invalidate_ca_delete'
               )",
        )
        .context("inspecting transitive-closure invalidation triggers")?;
    let triggers = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if triggers.len() != EXPECTED.len() {
        return Ok(false);
    }

    Ok(EXPECTED.iter().all(|(name, table, event)| {
        let expected = format!(
            "create trigger {name} after {event} on {table} begin update concept_ancestors_meta set schema_version = 0; end"
        );
        triggers.iter().any(|(actual_name, actual_table, sql)| {
            actual_name == name
                && actual_table == table
                && normalise_schema_sql(sql) == expected
        })
    }))
}

fn normalise_schema_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(feature = "cli")]
pub(crate) fn tct_includes_self(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT include_self != 0 FROM concept_ancestors_meta",
        [],
        |row| row.get(0),
    )
    .context("reading the transitive-closure completion marker")
}

#[cfg(feature = "cli")]
pub(crate) fn tct_marker_includes_self(conn: &Connection) -> Result<bool> {
    if !has_tct_meta_schema(conn)? {
        return Ok(false);
    }
    conn.query_row(
        "SELECT COUNT(*) = 1 AND MAX(include_self) != 0
         FROM concept_ancestors_meta",
        [],
        |row| row.get(0),
    )
    .context("reading the transitive-closure build mode")
}

fn has_tct_table_schema(conn: &Connection) -> Result<bool> {
    if !schema_object_is_table(conn, "concept_ancestors")? {
        return Ok(false);
    }
    let mut stmt = conn
        .prepare(
            "SELECT name, type, \"notnull\"
             FROM pragma_table_info('concept_ancestors')
             ORDER BY cid",
        )
        .context("inspecting the transitive-closure table")?;
    let columns = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns
        == [
            ("ancestor_id".to_string(), "INTEGER".to_string(), true),
            ("descendant_id".to_string(), "INTEGER".to_string(), true),
            ("depth".to_string(), "INTEGER".to_string(), true),
        ])
}

fn has_tct_meta_schema(conn: &Connection) -> Result<bool> {
    if !schema_object_is_table(conn, "concept_ancestors_meta")? {
        return Ok(false);
    }
    let mut stmt = conn
        .prepare(
            "SELECT name, type, \"notnull\"
             FROM pragma_table_info('concept_ancestors_meta')
             ORDER BY cid",
        )
        .context("inspecting the transitive-closure marker table")?;
    let columns = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns
        == [
            ("schema_version".to_string(), "INTEGER".to_string(), true),
            ("include_self".to_string(), "INTEGER".to_string(), true),
        ])
}

fn schema_object_is_table(conn: &Connection, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
         )",
        [name],
        |row| row.get(0),
    )
    .with_context(|| format!("checking the SQLite object type for {name}"))
}

pub(crate) fn has_tct_indexes(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT
             EXISTS(
                 SELECT 1 FROM pragma_index_list('concept_ancestors')
                 WHERE name = 'idx_ca_ancestor' AND \"unique\" = 0
                   AND origin = 'c' AND partial = 0
             )
             AND (SELECT COUNT(*) FROM pragma_index_xinfo('idx_ca_ancestor') WHERE key = 1) = 1
             AND EXISTS(
                 SELECT 1 FROM pragma_index_xinfo('idx_ca_ancestor')
                 WHERE key = 1 AND seqno = 0 AND name = 'ancestor_id' AND \"desc\" = 0
             )
             AND EXISTS(
                 SELECT 1 FROM pragma_index_list('concept_ancestors')
                 WHERE name = 'idx_ca_descendant' AND \"unique\" = 0
                   AND origin = 'c' AND partial = 0
             )
             AND (SELECT COUNT(*) FROM pragma_index_xinfo('idx_ca_descendant') WHERE key = 1) = 1
             AND EXISTS(
                 SELECT 1 FROM pragma_index_xinfo('idx_ca_descendant')
                 WHERE key = 1 AND seqno = 0 AND name = 'descendant_id' AND \"desc\" = 0
             )
             AND EXISTS(
                 SELECT 1 FROM pragma_index_list('concept_ancestors')
                 WHERE name = 'idx_ca_pair' AND \"unique\" = 1
                   AND origin = 'c' AND partial = 0
             )
             AND (SELECT COUNT(*) FROM pragma_index_xinfo('idx_ca_pair') WHERE key = 1) = 2
             AND EXISTS(
                 SELECT 1 FROM pragma_index_xinfo('idx_ca_pair')
                 WHERE key = 1 AND seqno = 0 AND name = 'ancestor_id' AND \"desc\" = 0
             )
             AND EXISTS(
                 SELECT 1 FROM pragma_index_xinfo('idx_ca_pair')
                 WHERE key = 1 AND seqno = 1 AND name = 'descendant_id' AND \"desc\" = 0
             )",
        [],
        |row| row.get(0),
    )
    .context("checking transitive-closure indexes")
}

/// Descendants of `id` **including `id` itself** (`<<id`). Shared by
/// `sct diagram` and `sct ecl compress`; reuses the same traversal the ECL
/// evaluator uses so subsumption has one definition across the codebase.
#[cfg(feature = "cli")]
pub(crate) fn descendants_or_self_with_tct(conn: &Connection, id: u64, tct: bool) -> Result<IdSet> {
    let mut out = Vec::new();
    collect_transitive(conn, id, true, tct, &mut out)?;
    out.push(id);
    Ok(set_from(out))
}

pub(crate) fn ancestors_with_tct(conn: &Connection, id: u64, tct: bool) -> Result<IdSet> {
    let mut out = Vec::new();
    collect_transitive(conn, id, false, tct, &mut out)?;
    Ok(set_from(out))
}

/// Direct IS-A parents of `id` (one hop up), deduplicated and sorted numerically.
#[cfg(feature = "cli")]
pub(crate) fn parents(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut v = Vec::new();
    collect_one_hop(conn, parse_sctid(id)?, false, &mut v)?;
    Ok(sorted_numeric(v))
}

/// Direct IS-A children of `id` (one hop down), deduplicated and sorted numerically.
#[cfg(feature = "cli")]
pub(crate) fn children(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut v = Vec::new();
    collect_one_hop(conn, parse_sctid(id)?, true, &mut v)?;
    Ok(sorted_numeric(v))
}

/// Defining attribute relationships of `id`: `(type_id, destination_id, group)`,
/// deduplicated (RF2 carries repeated rows) and stably ordered by group then type.
/// Returns an empty vec when the `concept_relationships` table is absent.
#[cfg(feature = "cli")]
pub(crate) fn relationships(conn: &Connection, id: &str) -> Result<Vec<(String, String, i64)>> {
    if !has_relationships_table(conn) {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT type_id, destination_id, group_num
         FROM concept_relationships WHERE source_id = ?1
         ORDER BY group_num, type_id, destination_id",
    )?;
    let rows = stmt.query_map([id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Deduplicate, sort numerically, and render as strings.
#[cfg(feature = "cli")]
fn sorted_numeric(mut v: Vec<u64>) -> Vec<String> {
    v.sort_unstable();
    v.dedup();
    v.into_iter().map(|id| id.to_string()).collect()
}

/// Proper descendants (`down = true`) or ancestors (`down = false`) of `id`.
/// Uses the indexed transitive-closure table when `tct` is set, otherwise a
/// recursive CTE over `concept_isa`. Both exclude `id` itself (the `<<`/`>>`
/// caller adds it back), so the `concept_ancestors` query filters self out in
/// case the table was built with `--include-self`.
pub(crate) fn collect_transitive(
    conn: &Connection,
    id: u64,
    down: bool,
    tct: bool,
    out: &mut Vec<u64>,
) -> Result<()> {
    // The id columns are TEXT: bind the parameter as text (one small format per
    // call, preserving index use) and CAST the result column so each row comes
    // back as an integer rather than an allocated String.
    let sql = match (tct, down) {
        (true, true) => {
            "SELECT CAST(descendant_id AS INTEGER) FROM concept_ancestors
             WHERE ancestor_id = ?1 AND descendant_id != ?1"
        }
        (true, false) => {
            "SELECT CAST(ancestor_id AS INTEGER) FROM concept_ancestors
             WHERE descendant_id = ?1 AND ancestor_id != ?1"
        }
        (false, true) => {
            "WITH RECURSIVE d(id) AS (
                SELECT child_id FROM concept_isa WHERE parent_id = ?1
                UNION
                SELECT ci.child_id FROM concept_isa ci JOIN d ON ci.parent_id = d.id
             ) SELECT CAST(id AS INTEGER) FROM d"
        }
        (false, false) => {
            "WITH RECURSIVE a(id) AS (
                SELECT parent_id FROM concept_isa WHERE child_id = ?1
                UNION
                SELECT ci.parent_id FROM concept_isa ci JOIN a ON ci.child_id = a.id
             ) SELECT CAST(id AS INTEGER) FROM a"
        }
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map([id.to_string()], |r| r.get::<_, i64>(0))?;
    for r in rows {
        out.push(r? as u64);
    }
    Ok(())
}

pub(crate) fn collect_one_hop(
    conn: &Connection,
    id: u64,
    down: bool,
    out: &mut Vec<u64>,
) -> Result<()> {
    let sql = if down {
        "SELECT CAST(child_id AS INTEGER) FROM concept_isa WHERE parent_id = ?1"
    } else {
        "SELECT CAST(parent_id AS INTEGER) FROM concept_isa WHERE child_id = ?1"
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map([id.to_string()], |r| r.get::<_, i64>(0))?;
    for r in rows {
        out.push(r? as u64);
    }
    Ok(())
}

fn collect_members(conn: &Connection, refset_id: u64, out: &mut Vec<u64>) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "SELECT CAST(referenced_component_id AS INTEGER) FROM refset_members WHERE refset_id = ?1",
    )?;
    let rows = stmt.query_map([refset_id.to_string()], |r| r.get::<_, i64>(0))?;
    for r in rows {
        out.push(r? as u64);
    }
    Ok(())
}

fn eval_refinement(
    conn: &Connection,
    focus: &IdSet,
    r: &Refinement,
    tct: &mut Option<bool>,
) -> Result<IdSet> {
    match r {
        Refinement::And(a, b) => {
            let sa = eval_refinement(conn, focus, a, tct)?;
            let sb = eval_refinement(conn, focus, b, tct)?;
            Ok(sa.intersection(&sb).copied().collect())
        }
        Refinement::Or(a, b) => {
            let sa = eval_refinement(conn, focus, a, tct)?;
            let sb = eval_refinement(conn, focus, b, tct)?;
            Ok(sa.union(&sb).copied().collect())
        }
        // v1: a group is a flat conjunction (group cardinality deferred).
        Refinement::Group(inner) => eval_refinement(conn, focus, inner, tct),
        Refinement::Attr {
            attr,
            negate,
            value,
        } => eval_attr(conn, focus, attr, *negate, value, tct),
    }
}

/// Whether the `concept_relationships` table exists. Databases built before
/// schema v4 lack it, so attribute refinement needs a rebuild.
pub(crate) fn has_relationships_table(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='concept_relationships'",
        [],
        |_| Ok(()),
    )
    .is_ok()
}

/// Ceiling on `|types| x |values|` index probes in attribute refinement before
/// falling back to the scan-by-type path. Probes cost a b-tree descent each
/// (~µs warm); the scan costs the type's whole row count. 4096 keeps every
/// realistic clinical value set on the probe path without letting a
/// pathological `= <<404684003`-sized value set turn into 136k probes.
const PROBE_LIMIT: usize = 4096;

fn eval_attr(
    conn: &Connection,
    focus: &IdSet,
    attr: &Expr,
    negate: bool,
    value: &Expr,
    tct: &mut Option<bool>,
) -> Result<IdSet> {
    if !has_relationships_table(conn) {
        anyhow::bail!(
            "ECL attribute refinement needs the 'concept_relationships' table, \
             which this database predates. Rebuild it with a current sct: \
             `sct ndjson` then `sct sqlite`."
        );
    }
    // `None` means wildcard (any type / any value).
    let type_filter: Option<IdSet> = match attr {
        Expr::Wildcard => None,
        _ => Some(eval_expr(conn, attr, tct)?),
    };
    let value_filter: Option<IdSet> = match value {
        Expr::Wildcard => None,
        _ => Some(eval_expr(conn, value, tct)?),
    };

    let mut matched = Vec::new();
    match &type_filter {
        // `type = <value set>` with a small type × value cross product: probe
        // the (type_id, destination_id) compound index once per pair instead
        // of scanning every row of the type. A common clinical refinement like
        // `<<404684003 : 363698007 = <<39057004` is 1 type × 27 values = 27
        // indexed probes, where the scan walked all 216k finding-site rows
        // (measured ~26x faster end to end). Negation cannot probe ("some
        // relationship whose destination is NOT in the set" needs to see every
        // row), and huge value sets degrade to probe-per-value, so both fall
        // through to the scan.
        Some(types)
            if !negate
                && value_filter
                    .as_ref()
                    .is_some_and(|vals| types.len().saturating_mul(vals.len()) <= PROBE_LIMIT) =>
        {
            let vals = value_filter.as_ref().expect("guard checked is_some");
            let mut stmt = conn.prepare_cached(
                "SELECT CAST(source_id AS INTEGER) FROM concept_relationships
                 WHERE type_id = ?1 AND destination_id = ?2",
            )?;
            for t in types {
                let t_s = t.to_string();
                for v in vals {
                    let rows = stmt.query_map([t_s.as_str(), v.to_string().as_str()], |r| {
                        r.get::<_, i64>(0)
                    })?;
                    for r in rows {
                        matched.push(r? as u64);
                    }
                }
            }
        }
        Some(types) => {
            let mut stmt = conn.prepare_cached(
                "SELECT CAST(source_id AS INTEGER), CAST(destination_id AS INTEGER)
                 FROM concept_relationships WHERE type_id = ?1",
            )?;
            for t in types {
                let rows = stmt.query_map([t.to_string()], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                })?;
                for row in rows {
                    let (source, dest) = row?;
                    consider(
                        source as u64,
                        dest as u64,
                        value_filter.as_ref(),
                        negate,
                        &mut matched,
                    );
                }
            }
        }
        None => {
            // Wildcard attribute type - full scan. Acceptable at codelist scale;
            // see spec/ecl.md §6 on the eventual SQL-compilation path.
            let mut stmt = conn.prepare_cached(
                "SELECT CAST(source_id AS INTEGER), CAST(destination_id AS INTEGER)
                 FROM concept_relationships",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (source, dest) = row?;
                consider(
                    source as u64,
                    dest as u64,
                    value_filter.as_ref(),
                    negate,
                    &mut matched,
                );
            }
        }
    }

    let matched = set_from(matched);
    Ok(focus.intersection(&matched).copied().collect())
}

/// Record `source` as a match if its relationship `dest` satisfies the value
/// constraint. `None` value filter means "any destination" (`= *`).
fn consider(
    source: u64,
    dest: u64,
    value_filter: Option<&IdSet>,
    negate: bool,
    matched: &mut Vec<u64>,
) {
    let in_value = match value_filter {
        None => true,
        Some(vs) => vs.contains(&dest),
    };
    // `=` matches when in_value; `!=` matches when not.
    if in_value != negate {
        matched.push(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_tct_completion_marker(conn: &Connection, include_self: bool) {
        conn.execute_batch(
            "CREATE TABLE concept_ancestors_meta (
                 schema_version INTEGER NOT NULL,
                 include_self INTEGER NOT NULL CHECK (include_self IN (0, 1))
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO concept_ancestors_meta VALUES (?1, ?2)",
            rusqlite::params![TCT_SCHEMA_VERSION, i64::from(include_self)],
        )
        .unwrap();
        conn.execute_batch(TCT_INVALIDATION_TRIGGERS_SQL).unwrap();
    }

    #[test]
    fn transitive_closure_probe_distinguishes_absent_and_present_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (id TEXT NOT NULL);
             INSERT INTO concepts VALUES ('1'), ('2');
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             INSERT INTO concept_isa VALUES ('2', '1');",
        )
        .unwrap();
        assert!(!has_tct(&conn).unwrap());

        conn.execute_batch(
            "CREATE TABLE concept_ancestors (
                 ancestor_id INTEGER NOT NULL,
                 descendant_id INTEGER NOT NULL,
                 depth INTEGER NOT NULL
             );",
        )
        .unwrap();
        assert!(!has_tct(&conn).unwrap());
        let mut fallback = Vec::new();
        collect_transitive(&conn, 1, true, has_tct(&conn).unwrap(), &mut fallback).unwrap();
        assert_eq!(fallback, [2]);

        conn.execute("INSERT INTO concept_ancestors VALUES (1, 2, 1)", [])
            .unwrap();
        assert!(!has_tct(&conn).unwrap());

        conn.execute_batch(
            "CREATE INDEX idx_ca_ancestor ON concept_ancestors(ancestor_id);
             CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id);
             CREATE UNIQUE INDEX idx_ca_pair
                 ON concept_ancestors(ancestor_id, descendant_id);",
        )
        .unwrap();
        assert!(!has_tct(&conn).unwrap());
        add_tct_completion_marker(&conn, false);
        assert!(has_tct(&conn).unwrap());
    }

    #[test]
    fn empty_transitive_closure_is_valid_for_an_empty_hierarchy() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (id TEXT NOT NULL);
             INSERT INTO concepts VALUES ('1');
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             CREATE TABLE concept_ancestors (
                 ancestor_id INTEGER NOT NULL,
                 descendant_id INTEGER NOT NULL,
                 depth INTEGER NOT NULL
             );
             CREATE INDEX idx_ca_ancestor ON concept_ancestors(ancestor_id);
             CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id);
             CREATE UNIQUE INDEX idx_ca_pair
                  ON concept_ancestors(ancestor_id, descendant_id);",
        )
        .unwrap();
        assert!(!has_tct(&conn).unwrap());
        add_tct_completion_marker(&conn, false);
        assert!(has_tct(&conn).unwrap());
    }
}
