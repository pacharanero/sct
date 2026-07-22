// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Typed results and SQLite queries for SNOMED CT simple reference sets.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// Sentinel passed to SQLite `LIMIT ?` meaning "no limit".
const SQLITE_NO_LIMIT: i64 = -1;

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefsetSummary {
    pub id: String,
    pub preferred_term: String,
    pub fsn: String,
    pub module: String,
    pub member_count: i64,
}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefsetMember {
    pub id: String,
    pub preferred_term: String,
    pub fsn: String,
    pub hierarchy: String,
    pub effective_time: String,
}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefsetDiffSet {
    pub count: i64,
    pub members: Vec<RefsetMember>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefsetComparison {
    pub refset_a: RefsetSummary,
    pub refset_b: RefsetSummary,
    pub only_in_a: RefsetDiffSet,
    pub only_in_b: RefsetDiffSet,
    pub in_both: RefsetDiffSet,
}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HierarchyCount {
    pub hierarchy: String,
    pub count: i64,
}

/// List all refsets with at least one loaded member, ordered by preferred term.
/// Pass `limit = None` for no limit.
pub fn list_refsets(conn: &Connection, limit: Option<i64>) -> Result<Vec<RefsetSummary>> {
    let mut stmt = conn.prepare(
        "SELECT rm.refset_id,
                COALESCE(c.preferred_term, '(unknown refset)'),
                COALESCE(c.fsn, ''),
                COALESCE(c.module, ''),
                COUNT(*) AS n
         FROM refset_members rm
         LEFT JOIN concepts c ON c.id = rm.refset_id
         GROUP BY rm.refset_id
         ORDER BY c.preferred_term
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit.unwrap_or(SQLITE_NO_LIMIT)], |row| {
            Ok(RefsetSummary {
                id: row.get(0)?,
                preferred_term: row.get(1)?,
                fsn: row.get(2)?,
                module: row.get(3)?,
                member_count: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// List concepts belonging to a refset, ordered by preferred term.
/// Pass `limit = None` for no limit.
pub fn list_refset_members(
    conn: &Connection,
    refset_id: &str,
    limit: Option<i64>,
) -> Result<Vec<RefsetMember>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy, c.effective_time
         FROM refset_members rm
         JOIN concepts c ON c.id = rm.referenced_component_id
         WHERE rm.refset_id = ?1
         ORDER BY c.preferred_term
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(
            params![refset_id, limit.unwrap_or(SQLITE_NO_LIMIT)],
            |row| {
                Ok(RefsetMember {
                    id: row.get(0)?,
                    preferred_term: row.get(1)?,
                    fsn: row.get(2)?,
                    hierarchy: row.get(3)?,
                    effective_time: row.get(4)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Look up a single refset's metadata + member count. `None` if the id isn't
/// a concept in the database at all (distinct from a concept with 0 members).
pub fn refset_summary(conn: &Connection, id: &str) -> Result<Option<RefsetSummary>> {
    match conn.query_row(
        "SELECT c.id, c.preferred_term, c.fsn, c.module,
                    (SELECT COUNT(*) FROM refset_members WHERE refset_id = c.id)
             FROM concepts c
             WHERE c.id = ?1",
        params![id],
        |row| {
            Ok(RefsetSummary {
                id: row.get(0)?,
                preferred_term: row.get(1)?,
                fsn: row.get(2)?,
                module: row.get(3)?,
                member_count: row.get(4)?,
            })
        },
    ) {
        Ok(summary) => Ok(Some(summary)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Which side of a two-refset membership comparison a query targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffSet {
    /// In `a`, not in `b`.
    OnlyA,
    /// In `b`, not in `a`.
    OnlyB,
    /// In both `a` and `b`.
    Both,
}

impl DiffSet {
    /// SQL keyword for the correlated subquery clause: `NOT EXISTS` for the
    /// "only in one side" sets, `EXISTS` for the intersection.
    fn sql_condition(self) -> &'static str {
        match self {
            DiffSet::OnlyA | DiffSet::OnlyB => "NOT EXISTS",
            DiffSet::Both => "EXISTS",
        }
    }
}

/// Count + (optionally limited) member list for one side of a refset
/// membership comparison. `primary`/`other` are refset ids; for `Both` the
/// order doesn't matter, for `OnlyA`/`OnlyB` `primary` is the refset the
/// members must belong to and `other` is the one they must be absent from.
fn refset_diff_set(
    conn: &Connection,
    primary: &str,
    other: &str,
    which: DiffSet,
    limit: Option<i64>,
) -> Result<RefsetDiffSet> {
    let exists = which.sql_condition();
    let count_sql = format!(
        "SELECT COUNT(*)
         FROM refset_members rm
         WHERE rm.refset_id = ?1
           AND {exists} (
               SELECT 1 FROM refset_members rm2
               WHERE rm2.refset_id = ?2 AND rm2.referenced_component_id = rm.referenced_component_id
           )"
    );
    let count: i64 = conn.query_row(&count_sql, params![primary, other], |row| row.get(0))?;

    let members_sql = format!(
        "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy, c.effective_time
         FROM refset_members rm
         JOIN concepts c ON c.id = rm.referenced_component_id
         WHERE rm.refset_id = ?1
           AND {exists} (
               SELECT 1 FROM refset_members rm2
               WHERE rm2.refset_id = ?2 AND rm2.referenced_component_id = rm.referenced_component_id
           )
         ORDER BY c.preferred_term
         LIMIT ?3"
    );
    let mut stmt = conn.prepare(&members_sql)?;
    let members = stmt
        .query_map(
            params![primary, other, limit.unwrap_or(SQLITE_NO_LIMIT)],
            |row| {
                Ok(RefsetMember {
                    id: row.get(0)?,
                    preferred_term: row.get(1)?,
                    fsn: row.get(2)?,
                    hierarchy: row.get(3)?,
                    effective_time: row.get(4)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(RefsetDiffSet { count, members })
}

/// Compare membership of two refsets. Pass `limit = None` to list every
/// member of each set; the reported `count` is always exact regardless of
/// `limit` (it comes from a separate `COUNT(*)` query, not `members.len()`).
pub fn compare_refsets(
    conn: &Connection,
    id_a: &str,
    id_b: &str,
    limit: Option<i64>,
) -> Result<RefsetComparison> {
    Ok(RefsetComparison {
        refset_a: refset_summary(conn, id_a)?.unwrap_or(RefsetSummary {
            id: id_a.to_string(),
            preferred_term: "(unknown refset)".into(),
            fsn: String::new(),
            module: String::new(),
            member_count: 0,
        }),
        refset_b: refset_summary(conn, id_b)?.unwrap_or(RefsetSummary {
            id: id_b.to_string(),
            preferred_term: "(unknown refset)".into(),
            fsn: String::new(),
            module: String::new(),
            member_count: 0,
        }),
        only_in_a: refset_diff_set(conn, id_a, id_b, DiffSet::OnlyA, limit)?,
        only_in_b: refset_diff_set(conn, id_b, id_a, DiffSet::OnlyB, limit)?,
        in_both: refset_diff_set(conn, id_a, id_b, DiffSet::Both, limit)?,
    })
}

/// Breakdown of a refset's members by top-level hierarchy, ordered by count
/// descending (ties broken by hierarchy name for stable output).
pub fn profile_refset_by_hierarchy(
    conn: &Connection,
    refset_id: &str,
) -> Result<Vec<HierarchyCount>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(c.hierarchy, ''), '(unknown)') AS h, COUNT(*) AS n
         FROM refset_members rm
         JOIN concepts c ON c.id = rm.referenced_component_id
         WHERE rm.refset_id = ?1
         GROUP BY h
         ORDER BY n DESC, h ASC",
    )?;
    let rows = stmt
        .query_map(params![refset_id], |row| {
            Ok(HierarchyCount {
                hierarchy: row.get(0)?,
                count: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
