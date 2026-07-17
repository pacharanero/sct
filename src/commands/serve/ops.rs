// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FHIR terminology operations as pure functions over a `rusqlite::Connection`,
//! returning `serde_json::Value` FHIR resources (or [`FhirError`]). The HTTP
//! layer in `mod.rs` is a thin wrapper around these. See `spec/commands/serve.md`.

use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashSet;

use super::fhir::{
    designation, internal_to_system, parameters, property_concept, system_to_internal,
    value_set_expansion, FhirError, SNOMED_SYSTEM,
};
use crate::ecl::ast::{Expr, Op};

fn ex(e: rusqlite::Error) -> FhirError {
    FhirError::exception(e.to_string())
}

struct Concept {
    pt: String,
    fsn: String,
    synonyms: Vec<String>,
    active: bool,
    module: String,
    effective_time: String,
}

fn fetch_concept(conn: &Connection, code: &str) -> Result<Option<Concept>, FhirError> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT preferred_term, fsn, synonyms, active, module, effective_time
             FROM concepts WHERE id = ?1",
        )
        .map_err(ex)?;
    let row = stmt.query_row([code], |r| {
        let synonyms_json: String = r.get(2)?;
        Ok(Concept {
            pt: r.get(0)?,
            fsn: r.get(1)?,
            synonyms: serde_json::from_str(&synonyms_json).unwrap_or_default(),
            active: r.get::<_, i64>(3)? != 0,
            module: r.get(4)?,
            effective_time: r.get(5)?,
        })
    });
    match row {
        Ok(c) => Ok(Some(c)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(ex(e)),
    }
}

/// SNOMED release version recorded in the DB provenance, for the `version`
/// parameter and CapabilityStatement.
pub fn release_version(conn: &Connection) -> Option<String> {
    crate::provenance::read_sqlite(conn)
        .ok()
        .flatten()
        .and_then(|p| {
            if !p.release_date.is_empty() {
                Some(p.release_date)
            } else if !p.release_id.is_empty() {
                Some(p.release_id)
            } else {
                None
            }
        })
}

/// Direct parents (`parent = true`) or children of a concept, active only.
fn direct(conn: &Connection, code: &str, parent: bool) -> Result<Vec<(String, String)>, FhirError> {
    let sql = if parent {
        "SELECT c.id, c.preferred_term FROM concept_isa ci JOIN concepts c ON c.id = ci.parent_id
         WHERE ci.child_id = ?1 AND c.active = 1 ORDER BY c.preferred_term"
    } else {
        "SELECT c.id, c.preferred_term FROM concept_isa ci JOIN concepts c ON c.id = ci.child_id
         WHERE ci.parent_id = ?1 AND c.active = 1 ORDER BY c.preferred_term"
    };
    let mut stmt = conn.prepare_cached(sql).map_err(ex)?;
    let rows = stmt
        .query_map([code], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(ex)?;
    rows.collect::<Result<_, _>>().map_err(ex)
}

/// All (transitive) ancestors of a concept, excluding itself.
fn ancestors(conn: &Connection, code: &str) -> Result<Vec<(String, String)>, FhirError> {
    let sql = "WITH RECURSIVE anc(id) AS (
                   SELECT ?1
                   UNION
                   SELECT ci.parent_id FROM concept_isa ci JOIN anc ON ci.child_id = anc.id
               )
               SELECT c.id, c.preferred_term FROM anc JOIN concepts c ON c.id = anc.id
               WHERE c.id != ?1 ORDER BY c.preferred_term";
    let mut stmt = conn.prepare_cached(sql).map_err(ex)?;
    let rows = stmt
        .query_map([code], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(ex)?;
    rows.collect::<Result<_, _>>().map_err(ex)
}

/// Is `descendant` subsumed by `ancestor` (i.e. is `ancestor` an ancestor-or-self)?
fn is_subsumed(conn: &Connection, descendant: &str, ancestor: &str) -> Result<bool, FhirError> {
    let sql = "WITH RECURSIVE anc(id) AS (
                   SELECT ?1
                   UNION
                   SELECT ci.parent_id FROM concept_isa ci JOIN anc ON ci.child_id = anc.id
               )
               SELECT EXISTS(SELECT 1 FROM anc WHERE id = ?2)";
    let exists: i64 = conn
        .query_row(sql, [descendant, ancestor], |r| r.get(0))
        .map_err(ex)?;
    Ok(exists != 0)
}

/// `CodeSystem/$lookup`.
pub fn lookup(conn: &Connection, code: &str, props: &[String]) -> Result<Value, FhirError> {
    let c = fetch_concept(conn, code)?
        .ok_or_else(|| FhirError::not_found(format!("Code '{code}' not found in SNOMED CT")))?;
    let want = |p: &str| props.iter().any(|x| x.eq_ignore_ascii_case(p));
    let none_requested = props.is_empty();

    let mut parameter = vec![
        json!({ "name": "name", "valueString": "SNOMED CT" }),
        json!({ "name": "display", "valueString": c.pt }),
    ];
    if let Some(v) = release_version(conn) {
        parameter.push(json!({ "name": "version", "valueString": v }));
    }
    if none_requested || want("designation") {
        parameter.push(designation(
            "900000000000003001",
            "Fully specified name",
            &c.fsn,
        ));
        for s in &c.synonyms {
            parameter.push(designation("900000000000013009", "Synonym", s));
        }
    }
    if want("parent") {
        for (id, pt) in direct(conn, code, true)? {
            parameter.push(property_concept("parent", &id, &pt));
        }
    }
    if want("child") {
        for (id, pt) in direct(conn, code, false)? {
            parameter.push(property_concept("child", &id, &pt));
        }
    }
    if want("ancestor") {
        for (id, pt) in ancestors(conn, code)? {
            parameter.push(property_concept("ancestor", &id, &pt));
        }
    }
    if want("inactive") {
        parameter.push(json!({ "name": "property", "part": [
            { "name": "code", "valueCode": "inactive" },
            { "name": "value", "valueBoolean": !c.active },
        ]}));
    }
    if want("moduleId") {
        parameter.push(json!({ "name": "property", "part": [
            { "name": "code", "valueCode": "moduleId" },
            { "name": "value", "valueCode": c.module },
        ]}));
    }
    if want("effectiveTime") {
        parameter.push(json!({ "name": "property", "part": [
            { "name": "code", "valueCode": "effectiveTime" },
            { "name": "value", "valueString": c.effective_time },
        ]}));
    }
    Ok(parameters(parameter))
}

/// `CodeSystem/$validate-code`. An unknown code is a valid `result=false`
/// response, not an error.
pub fn validate_code(
    conn: &Connection,
    code: &str,
    display: Option<&str>,
) -> Result<Value, FhirError> {
    match fetch_concept(conn, code)? {
        None => Ok(parameters(vec![
            json!({ "name": "result", "valueBoolean": false }),
            json!({ "name": "message", "valueString": format!("Code '{code}' not found in SNOMED CT") }),
        ])),
        Some(c) => {
            let mut result = true;
            let mut messages = Vec::new();
            if let Some(d) = display {
                let matches = d == c.pt || d == c.fsn || c.synonyms.iter().any(|s| s == d);
                if !matches {
                    result = false;
                    messages.push(format!(
                        "Display '{d}' does not match any designation for {code}"
                    ));
                }
            }
            if !c.active {
                messages.push("Concept is inactive".to_string());
            }
            let mut params = vec![
                json!({ "name": "result", "valueBoolean": result }),
                json!({ "name": "display", "valueString": c.pt }),
            ];
            for message in messages {
                params.push(json!({ "name": "message", "valueString": message }));
            }
            Ok(parameters(params))
        }
    }
}

/// `CodeSystem/$subsumes`.
pub fn subsumes(conn: &Connection, code_a: &str, code_b: &str) -> Result<Value, FhirError> {
    if fetch_concept(conn, code_a)?.is_none() {
        return Err(FhirError::not_found(format!("Code '{code_a}' not found")));
    }
    if fetch_concept(conn, code_b)?.is_none() {
        return Err(FhirError::not_found(format!("Code '{code_b}' not found")));
    }
    let outcome = if code_a == code_b {
        "equivalent"
    } else {
        let a_sub_b = is_subsumed(conn, code_a, code_b)?; // B is an ancestor of A
        let b_sub_a = is_subsumed(conn, code_b, code_a)?;
        match (a_sub_b, b_sub_a) {
            (true, true) => "equivalent",
            (true, false) => "subsumed-by",
            (false, true) => "subsumes",
            (false, false) => "not-subsumed",
        }
    };
    Ok(parameters(vec![
        json!({ "name": "outcome", "valueCode": outcome }),
    ]))
}

/// `ValueSet/$expand` over an optional ECL constraint and/or text filter.
pub fn expand(
    conn: &Connection,
    ecl: Option<&str>,
    filter: Option<&str>,
    count: usize,
    offset: usize,
    include_designations: bool,
) -> Result<Value, FhirError> {
    let count = count.min(1000);

    // Fast path: a single hierarchy/refset ECL with no text filter is answered
    // by two cheap SQL queries - an indexed COUNT and an index-ordered page -
    // so we never materialise the whole, potentially huge, id set in Rust.
    // Compound ECL, text filters, and the all-concepts case fall through.
    if filter.is_none() {
        if let Some(e) = ecl {
            if let Ok(parsed) = crate::ecl::parse(e) {
                if let Some((op, id)) = simple_op(&parsed) {
                    return expand_simple(
                        conn,
                        op,
                        &id,
                        has_tct(conn),
                        count,
                        offset,
                        include_designations,
                    );
                }
            }
        }
    }

    let matched: Vec<String> = match (ecl, filter) {
        // Entire implicit SNOMED ValueSet: paginate in SQL.
        (None, None) => {
            let total: i64 = conn
                .query_row("SELECT COUNT(*) FROM concepts WHERE active = 1", [], |r| {
                    r.get(0)
                })
                .map_err(ex)?;
            let mut stmt = conn
                .prepare("SELECT id FROM concepts WHERE active = 1 ORDER BY id LIMIT ?1 OFFSET ?2")
                .map_err(ex)?;
            let ids: Vec<String> = stmt
                .query_map([count as i64, offset as i64], |r| r.get(0))
                .map_err(ex)?
                .collect::<Result<_, _>>()
                .map_err(ex)?;
            let contains = build_contains(conn, &ids, include_designations)?;
            return Ok(value_set_expansion(total as usize, offset, count, contains));
        }
        (Some(e), None) => eval_ecl(conn, e)?,
        (None, Some(f)) => fts_ids(conn, f)?,
        (Some(e), Some(f)) => {
            let set: HashSet<String> = eval_ecl(conn, e)?.into_iter().collect();
            fts_ids(conn, f)?
                .into_iter()
                .filter(|id| set.contains(id))
                .collect()
        }
    };

    let total = matched.len();
    let start = offset.min(total);
    let end = offset.saturating_add(count).min(total);
    let contains = build_contains(conn, &matched[start..end], include_designations)?;
    Ok(value_set_expansion(total, offset, count, contains))
}

fn eval_ecl(conn: &Connection, ecl: &str) -> Result<Vec<String>, FhirError> {
    crate::ecl::expand(conn, ecl).map_err(|e| FhirError::invalid(format!("ECL error: {e:#}")))
}

/// FTS5 ids ordered by relevance, capped. Plain text is wrapped as a phrase to
/// avoid FTS5 parse errors on bare special characters.
fn fts_ids(conn: &Connection, filter: &str) -> Result<Vec<String>, FhirError> {
    let q = sanitise_fts(filter);
    let mut stmt = conn
        .prepare_cached(
            "SELECT c.id FROM concepts_fts JOIN concepts c ON concepts_fts.rowid = c.rowid
             WHERE concepts_fts MATCH ?1 AND c.active = 1 ORDER BY rank LIMIT 5000",
        )
        .map_err(ex)?;
    let ids = stmt
        .query_map([q], |r| r.get::<_, String>(0))
        .map_err(ex)?
        .collect::<Result<_, _>>()
        .map_err(ex)?;
    Ok(ids)
}

fn sanitise_fts(q: &str) -> String {
    let has_ops = q.contains('"')
        || q.contains('*')
        || q.contains('^')
        || q.to_uppercase().contains(" AND ")
        || q.to_uppercase().contains(" OR ")
        || q.to_uppercase().contains(" NOT ");
    if has_ops {
        q.to_string()
    } else {
        format!("\"{}\"", q.replace('"', "\"\""))
    }
}

/// Build `expansion.contains` entries for a page of ids, preserving order and
/// skipping ids that aren't concepts (e.g. refset metadata).
fn build_contains(
    conn: &Connection,
    ids: &[String],
    include_designations: bool,
) -> Result<Vec<Value>, FhirError> {
    let mut stmt = conn
        .prepare_cached("SELECT preferred_term, fsn, synonyms FROM concepts WHERE id = ?1")
        .map_err(ex)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let row = stmt.query_row([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        });
        match row {
            Ok((pt, fsn, syn)) => {
                out.push(contains_entry(id, &pt, &fsn, &syn, include_designations))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(e) => return Err(ex(e)),
        }
    }
    Ok(out)
}

/// Build a single `expansion.contains` entry, with designations when requested.
fn contains_entry(
    code: &str,
    pt: &str,
    fsn: &str,
    synonyms_json: &str,
    include_designations: bool,
) -> Value {
    let mut entry = json!({ "system": SNOMED_SYSTEM, "code": code, "display": pt });
    if include_designations {
        let synonyms: Vec<String> = serde_json::from_str(synonyms_json).unwrap_or_default();
        let mut des = vec![designation(
            "900000000000003001",
            "Fully specified name",
            fsn,
        )];
        for s in &synonyms {
            des.push(designation("900000000000013009", "Synonym", s));
        }
        entry["designation"] = Value::Array(des);
    }
    entry
}

/// Whether the transitive-closure table is present (lets `<<`/`>>` be indexed).
fn has_tct(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='concept_ancestors'",
        [],
        |_| Ok(()),
    )
    .is_ok()
}

/// If `expr` is a single hierarchy/refset operator on one concept, or a bare
/// concept, return `(op, concept_id)` - the shape the SQL fast path can handle.
/// `None` (the op slot) means a bare concept. Returns `None` overall for
/// anything compound (booleans, refinements, wildcards), so the caller falls
/// back to the full ECL engine.
fn simple_op(expr: &Expr) -> Option<(Option<Op>, String)> {
    match expr {
        Expr::Concept(id) => Some((None, id.clone())),
        Expr::Op(op, inner) => match &**inner {
            Expr::Concept(id) => Some((Some(*op), id.clone())),
            _ => None,
        },
        _ => None,
    }
}

/// SQL to count (`?1` = concept id) and page (`?1` = id, `?2` = limit, `?3` =
/// offset) the *proper* (non-self) set of a hierarchy/refset operator. The page
/// query orders by the id column, which for the transitive-closure cases is the
/// second column of the `(ancestor_id, descendant_id)` index - so SQLite serves
/// the page straight from the index with no sort.
fn body_sql(op: Op, tct: bool) -> (String, String) {
    match (op, tct) {
        (Op::DescendantOf | Op::DescendantOrSelfOf, true) => (
            "SELECT COUNT(*) FROM concept_ancestors WHERE ancestor_id = ?1 AND descendant_id != ?1"
                .into(),
            // concept_ancestors.descendant_id is INTEGER; CAST back to TEXT so
            // the row reader (shared with the TEXT concept_isa CTE path) sees a
            // string. ORDER BY is on the INTEGER column, so paging is numeric.
            "SELECT CAST(descendant_id AS TEXT) FROM concept_ancestors
             WHERE ancestor_id = ?1 AND descendant_id != ?1
             ORDER BY descendant_id LIMIT ?2 OFFSET ?3"
                .into(),
        ),
        (Op::DescendantOf | Op::DescendantOrSelfOf, false) => {
            let cte = "WITH RECURSIVE d(id) AS (
                SELECT child_id FROM concept_isa WHERE parent_id = ?1
                UNION
                SELECT ci.child_id FROM concept_isa ci JOIN d ON ci.parent_id = d.id)";
            (
                format!("{cte} SELECT COUNT(*) FROM d"),
                format!("{cte} SELECT id FROM d ORDER BY id LIMIT ?2 OFFSET ?3"),
            )
        }
        (Op::AncestorOf | Op::AncestorOrSelfOf, true) => (
            "SELECT COUNT(*) FROM concept_ancestors WHERE descendant_id = ?1 AND ancestor_id != ?1"
                .into(),
            // See the descendant case: CAST INTEGER id back to TEXT for the
            // shared row reader; ORDER BY stays on the INTEGER column.
            "SELECT CAST(ancestor_id AS TEXT) FROM concept_ancestors
             WHERE descendant_id = ?1 AND ancestor_id != ?1
             ORDER BY ancestor_id LIMIT ?2 OFFSET ?3"
                .into(),
        ),
        (Op::AncestorOf | Op::AncestorOrSelfOf, false) => {
            let cte = "WITH RECURSIVE a(id) AS (
                SELECT parent_id FROM concept_isa WHERE child_id = ?1
                UNION
                SELECT ci.parent_id FROM concept_isa ci JOIN a ON ci.child_id = a.id)";
            (
                format!("{cte} SELECT COUNT(*) FROM a"),
                format!("{cte} SELECT id FROM a ORDER BY id LIMIT ?2 OFFSET ?3"),
            )
        }
        (Op::ChildOf, _) => (
            "SELECT COUNT(*) FROM concept_isa WHERE parent_id = ?1".into(),
            "SELECT child_id FROM concept_isa WHERE parent_id = ?1
             ORDER BY child_id LIMIT ?2 OFFSET ?3"
                .into(),
        ),
        (Op::ParentOf, _) => (
            "SELECT COUNT(*) FROM concept_isa WHERE child_id = ?1".into(),
            "SELECT parent_id FROM concept_isa WHERE child_id = ?1
             ORDER BY parent_id LIMIT ?2 OFFSET ?3"
                .into(),
        ),
        (Op::MemberOf, _) => (
            "SELECT COUNT(*) FROM refset_members WHERE refset_id = ?1".into(),
            "SELECT referenced_component_id FROM refset_members WHERE refset_id = ?1
             ORDER BY referenced_component_id LIMIT ?2 OFFSET ?3"
                .into(),
        ),
    }
}

/// Expand a simple operator with an indexed `COUNT` for the total and an
/// index-ordered `LIMIT`/`OFFSET` for the page, so only one page of ids ever
/// reaches Rust. For the `-or-self` operators (`<<`, `>>`) and a bare concept,
/// the focus concept is prepended to the result (FHIR does not mandate an
/// ordering), shifting the body page by one slot.
fn expand_simple(
    conn: &Connection,
    op: Option<Op>,
    concept_id: &str,
    tct: bool,
    count: usize,
    offset: usize,
    include_designations: bool,
) -> Result<Value, FhirError> {
    let include_self = matches!(
        op,
        None | Some(Op::DescendantOrSelfOf) | Some(Op::AncestorOrSelfOf)
    );
    // Only count/return self when it is an actual active concept.
    let self_active = include_self
        && fetch_concept(conn, concept_id)?
            .map(|c| c.active)
            .unwrap_or(false);

    let body_count: i64 = match op {
        None => 0,
        Some(o) => {
            let (count_sql, _) = body_sql(o, tct);
            conn.query_row(&count_sql, [concept_id], |r| r.get(0))
                .map_err(ex)?
        }
    };
    let total = body_count as usize + usize::from(self_active);

    // Assemble the page: self (slot 0) then the body, with the body offset
    // shifted to account for the self slot.
    let mut page_ids: Vec<String> = Vec::new();
    let mut remaining = count;
    let mut body_offset = offset;
    if self_active {
        if offset == 0 {
            if count > 0 {
                page_ids.push(concept_id.to_string());
                remaining = count - 1;
            }
        } else {
            body_offset = offset - 1;
        }
    }
    if remaining > 0 {
        if let Some(o) = op {
            let (_, page_sql) = body_sql(o, tct);
            let mut stmt = conn.prepare(&page_sql).map_err(ex)?;
            let rows = stmt
                .query_map(
                    rusqlite::params![concept_id, remaining as i64, body_offset as i64],
                    |r| r.get::<_, String>(0),
                )
                .map_err(ex)?;
            for r in rows {
                page_ids.push(r.map_err(ex)?);
            }
        }
    }

    let contains = build_contains(conn, &page_ids, include_designations)?;
    Ok(value_set_expansion(total, offset, count, contains))
}

// ---------------------------------------------------------------------------
// Stored ValueSets (backed by `.codelist` files)
// ---------------------------------------------------------------------------

/// Expand a fixed member list (a stored `.codelist` ValueSet) with in-memory
/// pagination. Each page entry's display is reconciled against the live DB,
/// falling back to the stored term for concepts absent from this edition.
pub fn expand_members(
    conn: &Connection,
    members: &[(String, String)],
    count: usize,
    offset: usize,
    include_designations: bool,
) -> Result<Value, FhirError> {
    let count = count.min(1000);
    let total = members.len();
    let start = offset.min(total);
    let end = offset.saturating_add(count).min(total);

    let mut stmt = conn
        .prepare_cached("SELECT preferred_term, fsn, synonyms FROM concepts WHERE id = ?1")
        .map_err(ex)?;
    let mut contains = Vec::with_capacity(end - start);
    for (id, stored) in &members[start..end] {
        let row = stmt.query_row([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        });
        match row {
            Ok((pt, fsn, syn)) => {
                contains.push(contains_entry(id, &pt, &fsn, &syn, include_designations))
            }
            // Member not in this edition (e.g. retired): keep it, stored display.
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                contains.push(json!({ "system": SNOMED_SYSTEM, "code": id, "display": stored }))
            }
            Err(e) => return Err(ex(e)),
        }
    }
    Ok(value_set_expansion(total, offset, count, contains))
}

/// `ValueSet/$validate-code` against a stored member set: set membership plus
/// the live display term when present.
pub fn validate_code_in_set(
    conn: &Connection,
    members: &HashSet<String>,
    code: &str,
    vs_url: &str,
) -> Result<Value, FhirError> {
    let present = members.contains(code);
    let mut params = vec![json!({ "name": "result", "valueBoolean": present })];
    if present {
        if let Some(c) = fetch_concept(conn, code)? {
            params.push(json!({ "name": "display", "valueString": c.pt }));
        }
    } else {
        params.push(json!({ "name": "message",
            "valueString": format!("Code '{code}' is not in ValueSet {vs_url}") }));
    }
    Ok(parameters(params))
}

/// `ValueSet/$validate-code` against an implicit ECL value set: does `code`
/// satisfy the expression?
pub fn validate_code_in_ecl(conn: &Connection, ecl: &str, code: &str) -> Result<Value, FhirError> {
    let present = eval_ecl(conn, ecl)?.iter().any(|m| m == code);
    let mut params = vec![json!({ "name": "result", "valueBoolean": present })];
    if present {
        if let Some(c) = fetch_concept(conn, code)? {
            params.push(json!({ "name": "display", "valueString": c.pt }));
        }
    } else {
        params.push(json!({ "name": "message",
            "valueString": format!("Code '{code}' does not satisfy ECL: {ecl}") }));
    }
    Ok(parameters(params))
}

// ---------------------------------------------------------------------------
// ConceptMap/$translate (cross-terminology maps)
// ---------------------------------------------------------------------------

/// `ConceptMap/$translate` - map `code` in `source_system` to `target_system`
/// using the crossmap engine (the same maps as `sct transcode`). Supports
/// SNOMED CT, ICD-10, OPCS-4, CTV3, and Read v2 (by FHIR system URI or bare
/// name). Returns a `Parameters` resource with `result` + `match` entries.
pub fn translate(
    conn: &Connection,
    source_system: &str,
    code: &str,
    target_system: &str,
) -> Result<Value, FhirError> {
    let from = system_to_internal(source_system).ok_or_else(|| {
        FhirError::invalid(format!("unsupported source system {source_system:?}"))
    })?;
    let to = system_to_internal(target_system).ok_or_else(|| {
        FhirError::invalid(format!("unsupported target system {target_system:?}"))
    })?;
    let mapped = crate::commands::transcode::transcode_one(conn, from, code, to, false)
        .map_err(|e| FhirError::exception(e.to_string()))?;
    let target_url = internal_to_system(to);

    let mut params = vec![json!({ "name": "result", "valueBoolean": !mapped.is_empty() })];
    for m in &mapped {
        // We only have display strings for SNOMED targets (from `concepts`).
        let coding = if to == "snomed" {
            json!({ "system": target_url, "code": m.target, "display": m.display })
        } else {
            json!({ "system": target_url, "code": m.target })
        };
        params.push(json!({ "name": "match", "part": [
            { "name": "equivalence", "valueCode": "relatedto" },
            { "name": "concept", "valueCoding": coding },
        ]}));
    }
    Ok(parameters(params))
}
