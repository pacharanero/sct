// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FHIR terminology operations as pure functions over a `rusqlite::Connection`,
//! returning `serde_json::Value` FHIR resources (or [`FhirError`]). The HTTP
//! layer in `mod.rs` is a thin wrapper around these. See `spec/commands/serve.md`.

use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Instant;

use super::fhir::{
    designation, expansion_designation, internal_to_system, parameters, property_concept,
    system_to_internal, value_set_expansion, FhirError, SNOMED_SYSTEM,
};
use crate::ecl::ast::{Expr, Op};
use crate::sdk::{ConceptDesignations, SctError, Subsumption};

fn ex(e: rusqlite::Error) -> FhirError {
    FhirError::exception(e.to_string())
}

/// Ceiling on how many concept ids a single **compound** ECL evaluation (or a
/// combined ECL/filter expansion) may materialise in memory - roadmap `R53`.
/// A bare hierarchy/refset operator (`<<73211009`, `^refsetId`) never hits
/// this: `expand`'s fast path answers it with two indexed SQL queries and
/// never builds the full id set in Rust. Only expressions that fall through
/// to the general engine (booleans, refinements, wildcards) are bounded here.
/// Generous on purpose - real clinical ECL rarely approaches this - but firm
/// enough that a remote client cannot force gigabytes of `u64`s into memory.
const MAX_COMPOUND_ECL_RESULTS: usize = 100_000;

/// Resolve the FHIR `displayLanguage` `$expand` parameter against `sct`'s
/// data: the loaded SQLite database bakes in a single locale's preferred
/// terms at `sct ndjson --locale` build time (see
/// [`crate::builder::language_refset_priority`]) and does not record which
/// one, so the server cannot honour an arbitrary requested language - it can
/// only tell English apart from everything else. A request whose primary
/// BCP-47 subtag is `en` (any region, e.g. `en-GB`, `en-US`) is passed
/// through verbatim; anything else falls back to bare `en`, since that is
/// the only language family `sct` ever loads. Returns `None` when no
/// `displayLanguage` was requested, so the caller omits the
/// `expansion.parameter` entry entirely rather than reporting a language
/// nobody asked about.
pub fn resolve_display_language(requested: Option<&str>) -> Option<String> {
    let requested = requested?;
    let primary = requested.split(['-', '_']).next().unwrap_or("");
    if primary.eq_ignore_ascii_case("en") {
        Some(requested.to_string())
    } else {
        Some("en".to_string())
    }
}

/// Approximate number of SQLite virtual-machine instructions between
/// `sqlite3_progress_handler` callbacks. Small enough to interrupt an
/// overrunning statement within a fraction of a second of the deadline,
/// large enough that the callback itself is not measurable query overhead.
const PROGRESS_HANDLER_INTERVAL: std::ffi::c_int = 100_000;

/// RAII guard installing a SQLite progress handler that aborts the
/// currently-executing statement once `deadline` passes (`SQLITE_INTERRUPT`),
/// so a single expensive query - a wide recursive CTE, a full
/// `concept_relationships` scan - is cancelled mid-execution instead of
/// running to completion on a background thread after the client's HTTP
/// response has already timed out (roadmap `R53`, following on from the
/// request-level timeout added for `R73`). `conn` is a pooled, reused
/// connection, so the handler is unconditionally cleared on drop: left in
/// place, it would wrongly interrupt an unrelated later request that happens
/// to borrow the same connection.
struct DeadlineGuard<'c> {
    conn: &'c Connection,
}

impl<'c> DeadlineGuard<'c> {
    fn install(conn: &'c Connection, deadline: Instant) -> Self {
        // A failure here just means the handler wasn't installed (rusqlite
        // only rejects this on a connection it doesn't own); evaluation still
        // runs, just without early interruption if it overruns.
        let _ = conn.progress_handler(
            PROGRESS_HANDLER_INTERVAL,
            Some(move || Instant::now() >= deadline),
        );
        Self { conn }
    }
}

impl Drop for DeadlineGuard<'_> {
    fn drop(&mut self) {
        let _ = self.conn.progress_handler(0, None::<fn() -> bool>);
    }
}

/// Refuse an operation whose time budget is already spent, before touching the
/// database at all. The progress handler in [`DeadlineGuard`] only fires once a
/// statement is *running*, and only every [`PROGRESS_HANDLER_INTERVAL`]
/// instructions, so it cannot catch this case. It arises in practice for a
/// `$batch` Bundle, where every entry shares one request budget: once earlier
/// entries have consumed it, the rest should fail fast rather than each start
/// fresh work the client will never receive.
fn check_budget(deadline: Option<Instant>) -> Result<(), FhirError> {
    match deadline {
        Some(d) if Instant::now() >= d => Err(FhirError::timeout(
            "the request time budget was exhausted before this operation started".to_string(),
        )),
        _ => Ok(()),
    }
}

/// Whether `error` is (or wraps) a SQLite `SQLITE_INTERRUPT`, i.e. a
/// statement aborted by [`DeadlineGuard`].
fn is_interrupted(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        source
            .downcast_ref::<rusqlite::Error>()
            .and_then(rusqlite::Error::sqlite_error_code)
            == Some(rusqlite::ErrorCode::OperationInterrupted)
    })
}

/// Label for why a concept was retired, or `None` when the release does not
/// say (including on a database built before payload refsets were ingested).
fn inactivation_reason(conn: &Connection, code: &str) -> Result<Option<String>, FhirError> {
    crate::sdk::query_inactivation_reason(conn, code)
        .map(|reason| reason.map(|r| r.label))
        .map_err(|error| FhirError::exception(error.to_string()))
}

/// RF2 historical associations pointing at a retired concept's replacement(s).
fn historical_associations(
    conn: &Connection,
    code: &str,
) -> Result<Vec<crate::sdk::HistoryAssociation>, FhirError> {
    crate::sdk::query_history(conn, code).map_err(|error| FhirError::exception(error.to_string()))
}

fn fetch_concept(conn: &Connection, code: &str) -> Result<Option<ConceptDesignations>, FhirError> {
    crate::sdk::query_concept_designations(conn, code)
        .map_err(|error| FhirError::exception(error.to_string()))
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

/// Enforce the `$expand` `check-system-version` parameter. Each pin is a
/// canonical `[system]|[version]`; the R4 operation definition specifies that
/// an error is returned *instead of* the expansion when the version actually
/// in play differs from the pinned one.
///
/// `sct` serves exactly one SNOMED CT release per process, so a pin naming
/// SNOMED is checked against the loaded release. A pin naming any other code
/// system is vacuously satisfied - no other system ever contributes codes to
/// an expansion here, so there is no version to disagree about. A pin with no
/// `|version` part states no requirement and is ignored.
///
/// A pin the server *cannot* verify, because the database records no release
/// version, is an error rather than a silent pass. The entire point of the
/// parameter is that the client has declined to accept terminology of unknown
/// vintage, and quietly serving it anyway is exactly the failure it prevents.
pub fn check_system_versions(conn: &Connection, pins: &[String]) -> Result<(), FhirError> {
    let pinned_versions: Vec<&str> = pins
        .iter()
        .filter_map(|pin| {
            let (system, version) = pin.split_once('|')?;
            (system_to_internal(system.trim()) == Some("snomed")).then_some(version.trim())
        })
        .collect();
    if pinned_versions.is_empty() {
        return Ok(());
    }

    let Some(loaded) = release_version(conn) else {
        return Err(FhirError::invalid(
            "cannot honour `check-system-version`: this database records no SNOMED CT release version",
        ));
    };
    for pinned in pinned_versions {
        if pinned != loaded {
            return Err(FhirError::invalid(format!(
                "`check-system-version` requires SNOMED CT version {pinned}, but this server has {loaded} loaded"
            )));
        }
    }
    Ok(())
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
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn)
        .map_err(|error| FhirError::exception(format!("starting database read: {error:#}")))?;
    let sql = if crate::ecl::eval::has_tct(conn).map_err(|error| {
        FhirError::exception(format!("checking transitive-closure table: {error:#}"))
    })? {
        "SELECT c.id, c.preferred_term
         FROM concept_ancestors ca
         JOIN concepts c ON c.id = CAST(ca.ancestor_id AS TEXT)
         WHERE ca.descendant_id = ?1 AND ca.ancestor_id != ?1
         ORDER BY c.preferred_term, c.id"
    } else {
        "WITH RECURSIVE anc(id) AS (
             SELECT ?1
             UNION
             SELECT ci.parent_id FROM concept_isa ci JOIN anc ON ci.child_id = anc.id
         )
         SELECT c.id, c.preferred_term FROM anc JOIN concepts c ON c.id = anc.id
         WHERE c.id != ?1 ORDER BY c.preferred_term, c.id"
    };
    let mut stmt = conn.prepare_cached(sql).map_err(ex)?;
    let rows = stmt
        .query_map([code], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(ex)?;
    rows.collect::<Result<_, _>>().map_err(ex)
}

/// `CodeSystem/$lookup`.
pub fn lookup(conn: &Connection, code: &str, props: &[String]) -> Result<Value, FhirError> {
    let c = fetch_concept(conn, code)?
        .ok_or_else(|| FhirError::not_found(format!("Code '{code}' not found in SNOMED CT")))?;
    let want = |p: &str| props.iter().any(|x| x.eq_ignore_ascii_case(p));
    let none_requested = props.is_empty();

    let mut parameter = vec![
        json!({ "name": "name", "valueString": "SNOMED CT" }),
        json!({ "name": "display", "valueString": c.preferred_term }),
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
                let matches =
                    d == c.preferred_term || d == c.fsn || c.synonyms.iter().any(|s| s == d);
                if !matches {
                    result = false;
                    messages.push(format!(
                        "Display '{d}' does not match any designation for {code}"
                    ));
                }
            }
            if !c.active {
                // Name the reason and the replacement here, not just the fact
                // of inactivity: $validate-code is exactly where a client
                // discovers a code from an old record is no longer valid, and
                // the useful next step is which code to use instead. The FHIR
                // R4 SNOMED CT binding has no standard element for either, so
                // they go in the free-text message rather than an invented
                // property.
                let mut message = "Concept is inactive".to_string();
                if let Some(reason) = inactivation_reason(conn, code)? {
                    message.push_str(&format!(" ({reason})"));
                }
                messages.push(message);
                for association in historical_associations(conn, code)? {
                    let display = association.target_display.as_deref().unwrap_or("");
                    messages.push(format!(
                        "{} {} {}",
                        association.association.replace('_', " "),
                        association.target,
                        display
                    ));
                }
            }
            let mut params = vec![
                json!({ "name": "result", "valueBoolean": result }),
                json!({ "name": "display", "valueString": c.preferred_term }),
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
    let relationship = match crate::sdk::query_subsumption(conn, code_a, code_b) {
        Ok(relationship) => relationship,
        Err(SctError::ConceptNotFound { id }) => {
            return Err(FhirError::not_found(format!("Code '{id}' not found")))
        }
        Err(SctError::InvalidSctid { value, .. }) => {
            return Err(FhirError::invalid(format!(
                "Code '{value}' is not a valid SCTID"
            )))
        }
        Err(error) => return Err(FhirError::exception(error.to_string())),
    };
    let outcome = match relationship {
        Subsumption::Equivalent => "equivalent",
        Subsumption::Subsumes => "subsumes",
        Subsumption::SubsumedBy => "subsumed-by",
        Subsumption::NotSubsumed => "not-subsumed",
    };
    Ok(parameters(vec![
        json!({ "name": "outcome", "valueCode": outcome }),
    ]))
}

/// `ValueSet/$expand` over an optional ECL constraint and/or text filter.
/// `active_only` (the FHIR `activeOnly` parameter) filters the expansion to
/// active concepts when true; `sct serve` defaults this to true (see
/// `spec/commands/serve.md`), matching the pre-`activeOnly` behaviour of the
/// wildcard and text-filter paths. `deadline`, when set, bounds the
/// ECL/combined-filter evaluation path (see [`eval_ecl`], roadmap `R53`);
/// server handlers pass `Instant::now() + REQUEST_TIMEOUT`, tests pass
/// `None`. Always applies the production compound-result cap
/// (`MAX_COMPOUND_ECL_RESULTS`) - see [`expand_with_cap_for_tests`] for the
/// test-only variant with a caller-supplied cap.
#[allow(clippy::too_many_arguments)]
pub fn expand(
    conn: &Connection,
    ecl: Option<&str>,
    filter: Option<&str>,
    count: usize,
    offset: usize,
    include_designations: bool,
    active_only: bool,
    deadline: Option<Instant>,
    display_language: Option<&str>,
) -> Result<Value, FhirError> {
    expand_inner(
        conn,
        ecl,
        filter,
        count,
        offset,
        include_designations,
        active_only,
        deadline,
        MAX_COMPOUND_ECL_RESULTS,
        display_language,
    )
}

/// Identical to [`expand`] but with an explicit `max_results` cap, so tests
/// can exercise the bound-violation path (`403 too-costly`, no unbounded
/// materialisation) through the exact production code path
/// (parse -> [`evaluate_bounded`](crate::ecl::eval::evaluate_bounded) ->
/// [`classify_ecl_error`]) without needing >100k rows of real data in the
/// small committed fixture. `expand` itself always uses
/// `MAX_COMPOUND_ECL_RESULTS`; production traffic never calls this.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn expand_with_cap_for_tests(
    conn: &Connection,
    ecl: Option<&str>,
    filter: Option<&str>,
    count: usize,
    offset: usize,
    include_designations: bool,
    active_only: bool,
    deadline: Option<Instant>,
    max_results: usize,
    display_language: Option<&str>,
) -> Result<Value, FhirError> {
    expand_inner(
        conn,
        ecl,
        filter,
        count,
        offset,
        include_designations,
        active_only,
        deadline,
        max_results,
        display_language,
    )
}

#[allow(clippy::too_many_arguments)]
fn expand_inner(
    conn: &Connection,
    ecl: Option<&str>,
    filter: Option<&str>,
    count: usize,
    offset: usize,
    include_designations: bool,
    active_only: bool,
    deadline: Option<Instant>,
    max_results: usize,
    display_language: Option<&str>,
) -> Result<Value, FhirError> {
    let display_language = resolve_display_language(display_language);
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn)
        .map_err(|error| FhirError::exception(format!("starting database read: {error:#}")))?;
    // Installed here rather than in `eval_ecl` so it also covers the fast path
    // below, whose descendant/ancestor COUNT is an unlimited recursive CTE on a
    // database with no transitive-closure table - the single most expensive
    // statement a remote client can trigger with a short, obvious request.
    check_budget(deadline)?;
    let _guard = deadline.map(|d| DeadlineGuard::install(conn, d));
    let count = count.min(1000);

    // Fast path: a single hierarchy/refset ECL with no text filter is answered
    // by two cheap SQL queries - an indexed COUNT and an index-ordered page -
    // so we never materialise the whole, potentially huge, id set in Rust.
    // Compound ECL, text filters, and the all-concepts case fall through.
    if filter.is_none() {
        if let Some(e) = ecl {
            if let Ok(parsed) = crate::ecl::parse(e) {
                if let Some((op, id)) = simple_op(&parsed) {
                    id.parse::<u64>().map_err(|error| {
                        FhirError::invalid(format!("ECL error: invalid SCTID {id:?}: {error}"))
                    })?;
                    let has_tct = if matches!(
                        op,
                        Some(
                            Op::DescendantOf
                                | Op::DescendantOrSelfOf
                                | Op::AncestorOf
                                | Op::AncestorOrSelfOf
                        )
                    ) {
                        crate::ecl::eval::has_tct(conn).map_err(|error| {
                            FhirError::exception(format!(
                                "checking transitive-closure table: {error:#}"
                            ))
                        })?
                    } else {
                        false
                    };
                    return expand_simple(
                        conn,
                        op,
                        &id,
                        has_tct,
                        count,
                        offset,
                        include_designations,
                        active_only,
                        display_language.as_deref(),
                    );
                }
            }
        }
    }

    let matched: Vec<String> = match (ecl, filter) {
        // Entire implicit SNOMED ValueSet: paginate in SQL.
        (None, None) => {
            let where_clause = if active_only { "WHERE active = 1" } else { "" };
            let total: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM concepts {where_clause}"),
                    [],
                    |r| r.get(0),
                )
                .map_err(ex)?;
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT id FROM concepts {where_clause} ORDER BY id LIMIT ?1 OFFSET ?2"
                ))
                .map_err(ex)?;
            let ids: Vec<String> = stmt
                .query_map([count as i64, offset as i64], |r| r.get(0))
                .map_err(ex)?
                .collect::<Result<_, _>>()
                .map_err(ex)?;
            let contains = build_contains(conn, &ids, include_designations)?;
            return Ok(value_set_expansion(
                total as usize,
                offset,
                count,
                contains,
                display_language.as_deref(),
            ));
        }
        (Some(e), None) => {
            let ids = eval_ecl(conn, e, deadline, max_results)?;
            if active_only {
                filter_active_ids(conn, ids)?
            } else {
                ids
            }
        }
        (None, Some(f)) => fts_ids(conn, f, active_only)?,
        (Some(e), Some(f)) => {
            let set: HashSet<String> = eval_ecl(conn, e, deadline, max_results)?
                .into_iter()
                .collect();
            fts_ids(conn, f, active_only)?
                .into_iter()
                .filter(|id| set.contains(id))
                .collect()
        }
    };

    let total = matched.len();
    let start = offset.min(total);
    let end = offset.saturating_add(count).min(total);
    let contains = build_contains(conn, &matched[start..end], include_designations)?;
    Ok(value_set_expansion(
        total,
        offset,
        count,
        contains,
        display_language.as_deref(),
    ))
}

/// Evaluate an ECL expression bounded for server use: `deadline`, when set,
/// both installs a [`DeadlineGuard`] (so a single overrunning SQL statement
/// is interrupted rather than run to completion in the background) and is
/// checked cooperatively inside the evaluator itself (so many small
/// statements that overrun the wall clock in aggregate are also caught -
/// see [`EvalLimits`](crate::ecl::eval::EvalLimits)). `max_results` bounds
/// in-memory materialisation. See roadmap `R53`.
fn eval_ecl(
    conn: &Connection,
    ecl: &str,
    deadline: Option<Instant>,
    max_results: usize,
) -> Result<Vec<String>, FhirError> {
    let expr = crate::ecl::parse(ecl)
        .map_err(|error| FhirError::invalid(format!("ECL error: {error:#}")))?;
    // The [`DeadlineGuard`] is installed by the public entry points, not here:
    // it must also cover `expand`'s fast path, and nesting two guards on one
    // connection would leave the outer one inert once the inner one dropped.
    let limits = crate::ecl::eval::EvalLimits {
        max_results: Some(max_results),
        deadline,
    };
    crate::ecl::eval::evaluate_bounded(conn, &expr, limits)
        .map(|ids| ids.into_iter().map(|id| id.to_string()).collect())
        .map_err(classify_ecl_error)
}

/// Turn an ECL evaluation error into the right FHIR status: a malformed SCTID
/// is `invalid` (400); exceeding the result cap is `too-costly` (403) so the
/// client knows to narrow its query; running out of time - whether caught by
/// [`EvalLimits`](crate::ecl::eval::EvalLimits)'s own deadline check or by
/// [`DeadlineGuard`] interrupting a single SQL statement - is `timeout` (408),
/// matching the outer request-timeout middleware rather than a generic `500`;
/// anything else remains an `exception` (500).
fn classify_ecl_error(error: anyhow::Error) -> FhirError {
    if error
        .chain()
        .any(|source| source.is::<std::num::ParseIntError>())
    {
        return FhirError::invalid(format!("ECL error: {error:#}"));
    }
    if let Some(bound) = error
        .chain()
        .find_map(|source| source.downcast_ref::<crate::ecl::eval::EclBoundError>())
    {
        return match bound {
            crate::ecl::eval::EclBoundError::TooManyResults(_) => {
                FhirError::too_costly(bound.to_string())
            }
            crate::ecl::eval::EclBoundError::DeadlineExceeded => FhirError::timeout(format!(
                "{bound} and evaluation was interrupted before returning a result"
            )),
        };
    }
    if is_interrupted(&error) {
        return FhirError::timeout(
            "ECL evaluation exceeded the request time budget and was interrupted".to_string(),
        );
    }
    FhirError::exception(format!("evaluating ECL: {error:#}"))
}

/// FTS5 ids ordered by relevance, capped. Plain text is wrapped as a phrase to
/// avoid FTS5 parse errors on bare special characters.
fn fts_ids(conn: &Connection, filter: &str, active_only: bool) -> Result<Vec<String>, FhirError> {
    let q = sanitise_fts(filter);
    let active_and = if active_only { "AND c.active = 1" } else { "" };
    let mut stmt = conn
        .prepare_cached(&format!(
            "SELECT c.id FROM concepts_fts JOIN concepts c ON concepts_fts.rowid = c.rowid
             WHERE concepts_fts MATCH ?1 {active_and} ORDER BY rank LIMIT 5000"
        ))
        .map_err(ex)?;
    let ids = stmt
        .query_map([q], |r| r.get::<_, String>(0))
        .map_err(ex)?
        .collect::<Result<_, _>>()
        .map_err(ex)?;
    Ok(ids)
}

/// Filter `ids` (already-materialised matches from the general ECL engine, which
/// itself is `activeOnly`-agnostic - see [`eval_ecl`]) down to active concepts,
/// preserving `ids`' order. Batches the membership check to stay well clear of
/// SQLite's bound-parameter limit for large compound-ECL result sets.
fn filter_active_ids(conn: &Connection, ids: Vec<String>) -> Result<Vec<String>, FhirError> {
    if ids.is_empty() {
        return Ok(ids);
    }
    const CHUNK: usize = 500;
    let mut active: HashSet<String> = HashSet::with_capacity(ids.len());
    for chunk in ids.chunks(CHUNK) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id FROM concepts WHERE active = 1 AND id IN ({placeholders})");
        let mut stmt = conn.prepare(&sql).map_err(ex)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(params.as_slice(), |r| r.get::<_, String>(0))
            .map_err(ex)?;
        for r in rows {
            active.insert(r.map_err(ex)?);
        }
    }
    Ok(ids.into_iter().filter(|id| active.contains(id)).collect())
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
        let mut des = vec![expansion_designation(
            "900000000000003001",
            "Fully specified name",
            fsn,
        )];
        for s in &synonyms {
            des.push(expansion_designation("900000000000013009", "Synonym", s));
        }
        entry["designation"] = Value::Array(des);
    }
    entry
}

/// Whether a designation's SNOMED description-type code (`900000000000003001`
/// Fully specified name, or `900000000000013009` Synonym) matches one of the
/// requested `$expand` `designation` parameter tokens. Each token is either
/// `system|code`, a bare description-type code, `*` (match everything), or a
/// BCP-47 language tag. `sct` serves only SNOMED designations from one English
/// locale, so another coding system or language never matches.
fn designation_matches(tokens: &[String], type_id: &str) -> bool {
    tokens.iter().any(|tok| {
        if tok == "*" {
            return true;
        }

        if let Some((system, code)) = tok.split_once('|') {
            return system == SNOMED_SYSTEM && code == type_id;
        }

        if tok == "900000000000003001" || tok == "900000000000013009" {
            return tok == type_id;
        }

        tok.eq_ignore_ascii_case("en")
            || tok
                .get(..3)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("en-"))
    })
}

/// Narrow an already-built expansion's `expansion.contains[].designation`
/// entries down to those matching the `$expand` `designation` parameter's
/// tokens, dropping the `designation` key entirely from any entry left with
/// none. A no-op when `tokens` is empty (no `designation` parameter was
/// requested). Per the operation definition, `designation` selects *which*
/// designations come back rather than which concepts do, so this runs as a
/// post-filter over the expansion built with designations already turned on
/// (the caller must force `include_designations` true whenever `tokens` is
/// non-empty - "if this parameter is present, the request is honored as if
/// includeDesignations is true") rather than threading a filter through the
/// `expand`/`expand_members` SQL, which selects concepts, not designations.
pub fn apply_designation_filter(expansion: &mut Value, tokens: &[String]) {
    if tokens.is_empty() {
        return;
    }
    let Some(contains) = expansion["expansion"]["contains"].as_array_mut() else {
        return;
    };
    for entry in contains {
        let emptied = match entry.get_mut("designation").and_then(|d| d.as_array_mut()) {
            Some(designations) => {
                designations.retain(|d| {
                    d["use"]["code"]
                        .as_str()
                        .is_some_and(|code| designation_matches(tokens, code))
                });
                designations.is_empty()
            }
            None => false,
        };
        if emptied {
            if let Some(obj) = entry.as_object_mut() {
                obj.remove("designation");
            }
        }
    }
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
///
/// When `active_only` is true, an inner join against `concepts` restricts both
/// queries to active concepts (same join shape as [`ancestors`]'s TCT branch),
/// trading the pure index-only scan for one indexed `concepts` probe per row -
/// still index-backed, just no longer the single covering-index scan the
/// `active_only: false` form gets.
fn body_sql(op: Op, tct: bool, active_only: bool) -> (String, String) {
    match (op, tct) {
        (Op::DescendantOf | Op::DescendantOrSelfOf, true) => {
            // concept_ancestors.descendant_id is INTEGER; CAST back to TEXT so
            // the row reader (shared with the TEXT concept_isa CTE path) sees a
            // string. ORDER BY is on the INTEGER column, so paging is numeric.
            let join = if active_only {
                "JOIN concepts ac ON ac.id = CAST(ca.descendant_id AS TEXT) AND ac.active = 1"
            } else {
                ""
            };
            (
                format!(
                    "SELECT COUNT(*) FROM concept_ancestors ca {join}
                     WHERE ca.ancestor_id = ?1 AND ca.descendant_id != ?1"
                ),
                format!(
                    "SELECT CAST(ca.descendant_id AS TEXT) FROM concept_ancestors ca {join}
                     WHERE ca.ancestor_id = ?1 AND ca.descendant_id != ?1
                     ORDER BY ca.descendant_id LIMIT ?2 OFFSET ?3"
                ),
            )
        }
        (Op::DescendantOf | Op::DescendantOrSelfOf, false) => {
            let cte = "WITH RECURSIVE d(id) AS (
                SELECT child_id FROM concept_isa WHERE parent_id = ?1
                UNION
                SELECT ci.child_id FROM concept_isa ci JOIN d ON ci.parent_id = d.id)";
            let join = if active_only {
                "JOIN concepts ac ON ac.id = d.id AND ac.active = 1"
            } else {
                ""
            };
            (
                format!("{cte} SELECT COUNT(*) FROM d {join}"),
                format!("{cte} SELECT d.id FROM d {join} ORDER BY d.id LIMIT ?2 OFFSET ?3"),
            )
        }
        (Op::AncestorOf | Op::AncestorOrSelfOf, true) => {
            // See the descendant case: CAST INTEGER id back to TEXT for the
            // shared row reader; ORDER BY stays on the INTEGER column.
            let join = if active_only {
                "JOIN concepts ac ON ac.id = CAST(ca.ancestor_id AS TEXT) AND ac.active = 1"
            } else {
                ""
            };
            (
                format!(
                    "SELECT COUNT(*) FROM concept_ancestors ca {join}
                     WHERE ca.descendant_id = ?1 AND ca.ancestor_id != ?1"
                ),
                format!(
                    "SELECT CAST(ca.ancestor_id AS TEXT) FROM concept_ancestors ca {join}
                     WHERE ca.descendant_id = ?1 AND ca.ancestor_id != ?1
                     ORDER BY ca.ancestor_id LIMIT ?2 OFFSET ?3"
                ),
            )
        }
        (Op::AncestorOf | Op::AncestorOrSelfOf, false) => {
            let cte = "WITH RECURSIVE a(id) AS (
                SELECT parent_id FROM concept_isa WHERE child_id = ?1
                UNION
                SELECT ci.parent_id FROM concept_isa ci JOIN a ON ci.child_id = a.id)";
            let join = if active_only {
                "JOIN concepts ac ON ac.id = a.id AND ac.active = 1"
            } else {
                ""
            };
            (
                format!("{cte} SELECT COUNT(*) FROM a {join}"),
                format!("{cte} SELECT a.id FROM a {join} ORDER BY a.id LIMIT ?2 OFFSET ?3"),
            )
        }
        (Op::ChildOf, _) => {
            let join = if active_only {
                "JOIN concepts ac ON ac.id = ci.child_id AND ac.active = 1"
            } else {
                ""
            };
            (
                format!("SELECT COUNT(*) FROM concept_isa ci {join} WHERE ci.parent_id = ?1"),
                format!(
                    "SELECT ci.child_id FROM concept_isa ci {join} WHERE ci.parent_id = ?1
                     ORDER BY ci.child_id LIMIT ?2 OFFSET ?3"
                ),
            )
        }
        (Op::ParentOf, _) => {
            let join = if active_only {
                "JOIN concepts ac ON ac.id = ci.parent_id AND ac.active = 1"
            } else {
                ""
            };
            (
                format!("SELECT COUNT(*) FROM concept_isa ci {join} WHERE ci.child_id = ?1"),
                format!(
                    "SELECT ci.parent_id FROM concept_isa ci {join} WHERE ci.child_id = ?1
                     ORDER BY ci.parent_id LIMIT ?2 OFFSET ?3"
                ),
            )
        }
        (Op::MemberOf, _) => {
            let join = if active_only {
                "JOIN concepts ac ON ac.id = rm.referenced_component_id AND ac.active = 1"
            } else {
                ""
            };
            (
                format!("SELECT COUNT(*) FROM refset_members rm {join} WHERE rm.refset_id = ?1"),
                format!(
                    "SELECT rm.referenced_component_id FROM refset_members rm {join}
                     WHERE rm.refset_id = ?1
                     ORDER BY rm.referenced_component_id LIMIT ?2 OFFSET ?3"
                ),
            )
        }
    }
}

/// Expand a simple operator with an indexed `COUNT` for the total and an
/// index-ordered `LIMIT`/`OFFSET` for the page, so only one page of ids ever
/// reaches Rust. For the `-or-self` operators (`<<`, `>>`) and a bare concept,
/// the focus concept is prepended to the result (FHIR does not mandate an
/// ordering), shifting the body page by one slot.
#[allow(clippy::too_many_arguments)]
fn expand_simple(
    conn: &Connection,
    op: Option<Op>,
    concept_id: &str,
    tct: bool,
    count: usize,
    offset: usize,
    include_designations: bool,
    active_only: bool,
    display_language: Option<&str>,
) -> Result<Value, FhirError> {
    let include_self = matches!(
        op,
        None | Some(Op::DescendantOrSelfOf) | Some(Op::AncestorOrSelfOf)
    );
    // Self only counts/is returned when it exists, and (when `active_only`) is
    // active - so `activeOnly=false` can surface an inactive focus concept
    // (e.g. a bare `ecl/<inactive-id>` or `<<<inactive-id>`) that
    // `activeOnly=true` (the default) correctly excludes.
    let self_active = include_self
        && fetch_concept(conn, concept_id)?
            .map(|c| !active_only || c.active)
            .unwrap_or(false);

    let body_count: i64 = match op {
        None => 0,
        Some(o) => {
            let (count_sql, _) = body_sql(o, tct, active_only);
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
            let (_, page_sql) = body_sql(o, tct, active_only);
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
    Ok(value_set_expansion(
        total,
        offset,
        count,
        contains,
        display_language,
    ))
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
    display_language: Option<&str>,
) -> Result<Value, FhirError> {
    let display_language = resolve_display_language(display_language);
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
    Ok(value_set_expansion(
        total,
        offset,
        count,
        contains,
        display_language.as_deref(),
    ))
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
            params.push(json!({ "name": "display", "valueString": c.preferred_term }));
        }
    } else {
        params.push(json!({ "name": "message",
            "valueString": format!("Code '{code}' is not in ValueSet {vs_url}") }));
    }
    Ok(parameters(params))
}

/// `ValueSet/$validate-code` against an implicit ECL value set: does `code`
/// satisfy the expression? `deadline` bounds evaluation as in [`expand`].
pub fn validate_code_in_ecl(
    conn: &Connection,
    ecl: &str,
    code: &str,
    deadline: Option<Instant>,
) -> Result<Value, FhirError> {
    check_budget(deadline)?;
    let _guard = deadline.map(|d| DeadlineGuard::install(conn, d));
    let present = eval_ecl(conn, ecl, deadline, MAX_COMPOUND_ECL_RESULTS)?
        .iter()
        .any(|m| m == code);
    let mut params = vec![json!({ "name": "result", "valueBoolean": present })];
    if present {
        if let Some(c) = fetch_concept(conn, code)? {
            params.push(json!({ "name": "display", "valueString": c.preferred_term }));
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
            { "name": "equivalence", "valueCode": equivalence_for_correlation(m.correlation.as_deref()) },
            { "name": "concept", "valueCoding": coding },
        ]}));
    }
    Ok(parameters(params))
}

/// Translate an RF2 ExtendedMap `correlationId` into the FHIR
/// `ConceptMapEquivalence` code it actually claims, rather than the generic
/// `relatedto` every match used to report regardless of what the map data
/// says. Verified against a real UK Monolith release rather than written from
/// memory (see `spec/roadmap.md` R11 and `AGENTS.md`'s cross-check rule).
///
/// `447562003` ("SNOMED CT to ICD-10 extended map") is the refset's own
/// identity, never a member's `correlationId`, so it is deliberately absent
/// here. `None` covers CTV3/Read v2 (SimpleMap carries no correlation) and a
/// SNOMED target (the identity mapping needs none).
fn equivalence_for_correlation(correlation: Option<&str>) -> &'static str {
    match correlation {
        Some("447557004") => "equivalent", // Exact match
        Some("447558009") => "wider",      // Narrow to broad: target is broader
        Some("447559001") => "narrower",   // Broad to narrow: target is narrower
        Some("447560006") => "inexact",    // Partial overlap
        // "447561005" (correlation not specified), any other/unrecognised
        // value, and CTV3/Read v2/SNOMED targets (which carry no correlation
        // at all) all fall back to the same conservative default as before.
        _ => "relatedto",
    }
}

#[cfg(test)]
mod equivalence_tests {
    use super::equivalence_for_correlation;

    #[test]
    fn maps_the_four_real_correlation_values() {
        assert_eq!(equivalence_for_correlation(Some("447557004")), "equivalent");
        assert_eq!(equivalence_for_correlation(Some("447558009")), "wider");
        assert_eq!(equivalence_for_correlation(Some("447559001")), "narrower");
        assert_eq!(equivalence_for_correlation(Some("447560006")), "inexact");
    }

    #[test]
    fn falls_back_to_relatedto_for_unspecified_unknown_and_absent_correlation() {
        // "Not specified" - what every row in the committed synthetic fixture
        // uses, so this is the common case in CI, not an edge case.
        assert_eq!(equivalence_for_correlation(Some("447561005")), "relatedto");
        // A correlation value this build does not recognise - fail safe to
        // the conservative default rather than panicking or guessing.
        assert_eq!(equivalence_for_correlation(Some("999999999")), "relatedto");
        // No correlation at all - CTV3/Read v2 (SimpleMap has no correlation
        // column) and the SNOMED identity mapping.
        assert_eq!(equivalence_for_correlation(None), "relatedto");
    }

    /// 447562003 is the ExtendedMap refset's own identity concept ("SNOMED CT
    /// to ICD-10 extended map"), never a member row's correlationId. If it
    /// ever appeared here it must not be silently treated as a real
    /// equivalence claim.
    #[test]
    fn the_refset_identity_concept_is_not_treated_as_a_correlation() {
        assert_eq!(equivalence_for_correlation(Some("447562003")), "relatedto");
    }
}

#[cfg(test)]
mod deadline_guard_tests {
    //! White-box tests of [`DeadlineGuard`] itself: is the interruption real
    //! (aborts a genuinely long-running statement promptly), and is it inert
    //! before the deadline (a query that would otherwise be fine still
    //! succeeds)? These use a plain in-memory connection and a synthetic
    //! recursive-CTE workload rather than the SNOMED fixture, because what's
    //! under test is generic SQLite interruption mechanics, not SNOMED query
    //! logic - the ECL-specific bound/cap behaviour is covered against the
    //! real fixture-built database in `tests/serve.rs`.

    use super::*;
    use std::time::Duration;

    /// A statement that does a meaningful amount of work (tens of millions of
    /// VM instructions) if allowed to run to completion, so an interruption
    /// that fires only "eventually" would still show up as a slow test.
    const EXPENSIVE_COUNT: &str = "WITH RECURSIVE cnt(x) AS (
            SELECT 1
            UNION ALL
            SELECT x + 1 FROM cnt WHERE x < 50000000
        ) SELECT COUNT(*) FROM cnt";

    #[test]
    fn interrupts_an_already_overdue_statement_promptly() {
        let conn = Connection::open_in_memory().unwrap();
        // Already in the past: the very first progress-handler callback (a
        // small, fixed number of VM instructions into the statement) must
        // interrupt it - proving the statement is aborted mid-execution, not
        // merely rejected after running to completion in the background.
        let deadline = Instant::now() - Duration::from_secs(1);
        let _guard = DeadlineGuard::install(&conn, deadline);

        let start = Instant::now();
        let result: rusqlite::Result<i64> = conn.query_row(EXPENSIVE_COUNT, [], |r| r.get(0));
        let elapsed = start.elapsed();

        let err = result.expect_err("an overdue deadline should interrupt the statement");
        assert_eq!(
            err.sqlite_error_code(),
            Some(rusqlite::ErrorCode::OperationInterrupted),
            "expected SQLITE_INTERRUPT, got {err:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "interrupted statement took {elapsed:?} - the progress handler should abort it \
             promptly rather than letting it run to completion"
        );
    }

    #[test]
    fn does_not_interrupt_before_the_deadline() {
        let conn = Connection::open_in_memory().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let _guard = DeadlineGuard::install(&conn, deadline);
        let n: i64 = conn.query_row("SELECT 1 + 1", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn guard_drop_clears_the_handler_for_the_next_use_of_a_pooled_connection() {
        let conn = Connection::open_in_memory().unwrap();
        {
            let deadline = Instant::now() - Duration::from_secs(1);
            let _guard = DeadlineGuard::install(&conn, deadline);
            let result: rusqlite::Result<i64> = conn.query_row(EXPENSIVE_COUNT, [], |r| r.get(0));
            assert!(result.is_err(), "sanity: the overdue guard did interrupt");
        }
        // The guard is dropped now. A pooled connection reused for an
        // unrelated later request must not still be carrying a stale,
        // already-expired deadline from this one.
        let n: i64 = conn.query_row("SELECT 1 + 1", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }
}
