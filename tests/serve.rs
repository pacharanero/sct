// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! `sct serve` FHIR R4 tests over the synthetic RF2 fixture. Exercises the
//! operation logic directly (FHIR semantics) plus one live HTTP round-trip.
//! Gated on `--features serve`.
#![cfg(feature = "serve")]

use rusqlite::Connection;
use sct_rs::commands::ndjson::{self, RefsetMode};
use sct_rs::commands::serve::{fhir, ops, serve_listener, valuesets};
use sct_rs::commands::sqlite;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn build_db() -> (tempfile::TempDir, PathBuf) {
    build_db_with(false)
}

/// Build the fixture DB with `--refsets all`, so the ICD-10/OPCS-4 crossmaps and
/// concept history load (for `ConceptMap/$translate`).
fn build_db_all() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ndjson = dir.path().join("syn.ndjson");
    let db = dir.path().join("syn.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![fixture_dir()],
        locale: "en-GB".to_string(),
        output: Some(ndjson.clone()),
        include_inactive: false,
        refsets: RefsetMode::All,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input: ndjson,
        output: Some(db.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    (dir, db)
}

/// Build the fixture DB, optionally with the transitive-closure table so the
/// `$expand` fast path exercises its TCT SQL form.
fn build_db_with(tct: bool) -> (tempfile::TempDir, PathBuf) {
    build_db_with_inactive(tct, false)
}

/// Like [`build_db_with`], but optionally loads inactive concepts too - the
/// fixture's `9468002` ("Inactive example disorder", `active=0`, an IS-A child
/// of `404684003`) only appears in the DB when `include_inactive` is true, so
/// `activeOnly` tests need this builder rather than the default (which always
/// excludes it, independent of `activeOnly`).
fn build_db_with_inactive(tct: bool, include_inactive: bool) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ndjson = dir.path().join("syn.ndjson");
    let db = dir.path().join("syn.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![fixture_dir()],
        locale: "en-GB".to_string(),
        output: Some(ndjson.clone()),
        include_inactive,
        refsets: RefsetMode::Simple,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input: ndjson,
        output: Some(db.clone()),
        transitive_closure: tct,
        include_self: false,
    })
    .unwrap();
    (dir, db)
}

fn conn(db: &PathBuf) -> Connection {
    Connection::open(db).unwrap()
}

fn param_str<'a>(v: &'a Value, name: &str) -> Option<&'a str> {
    v["parameter"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == name)
        .and_then(|p| p["valueString"].as_str())
}

fn param_bool(v: &Value, name: &str) -> Option<bool> {
    v["parameter"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == name)
        .and_then(|p| p["valueBoolean"].as_bool())
}

fn param_code<'a>(v: &'a Value, name: &str) -> Option<&'a str> {
    v["parameter"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == name)
        .and_then(|p| p["valueCode"].as_str())
}

fn expansion_count(v: &Value) -> Option<u64> {
    v["expansion"]["parameter"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == "count")
        .and_then(|p| p["valueInteger"].as_u64())
}

/// The `displayLanguage` `expansion.parameter` entry, if present.
fn expansion_display_language(v: &Value) -> Option<String> {
    v["expansion"]["parameter"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == "displayLanguage")
        .and_then(|p| p["valueCode"].as_str())
        .map(String::from)
}

/// Collect the `valueCode` values of `$lookup` `property` entries with the given code.
fn property_codes(v: &Value, prop: &str) -> Vec<String> {
    v["parameter"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["name"] == "property")
        .filter(|p| {
            p["part"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .any(|x| x["name"] == "code" && x["valueCode"] == prop)
                })
                .unwrap_or(false)
        })
        .filter_map(|p| {
            p["part"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["name"] == "value")
                .and_then(|x| x["valueCode"].as_str())
                .map(String::from)
        })
        .collect()
}

/// The `value` strings of all `$lookup` `designation` entries.
fn designations(v: &Value) -> Vec<String> {
    v["parameter"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["name"] == "designation")
        .filter_map(|p| {
            p["part"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["name"] == "value")
                .and_then(|x| x["valueString"].as_str())
                .map(String::from)
        })
        .collect()
}

fn contains_codes(vs: &Value) -> Vec<String> {
    vs["expansion"]["contains"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["code"].as_str().map(String::from))
        .collect()
}

fn expansion_designations(vs: &Value, code: &str) -> Vec<String> {
    vs["expansion"]["contains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["code"] == code)
        .and_then(|entry| entry["designation"].as_array())
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|designation| designation["value"].as_str().map(String::from))
        .collect()
}

#[test]
fn lookup_display_designations_parents() {
    let (_d, db) = build_db();
    let c = conn(&db);
    let v = ops::lookup(
        &c,
        "22298006",
        &["display".into(), "designation".into(), "parent".into()],
    )
    .unwrap();

    assert_eq!(v["resourceType"], "Parameters");
    assert_eq!(param_str(&v, "display"), Some("Myocardial infarction"));

    let des = designations(&v);
    assert!(des.contains(&"Myocardial infarction (disorder)".to_string())); // FSN
    assert!(des.contains(&"Heart attack".to_string())); // synonym

    assert!(property_codes(&v, "parent").contains(&"404684003".to_string())); // Clinical finding
}

#[test]
fn lookup_ancestors_match_with_and_without_transitive_closure() {
    let (_without_dir, without_db) = build_db_with(false);
    let (_with_dir, with_db) = build_db_with(true);

    let without = ops::lookup(&conn(&without_db), "46635009", &["ancestor".into()]).unwrap();
    let with = ops::lookup(&conn(&with_db), "46635009", &["ancestor".into()]).unwrap();

    assert_eq!(
        property_codes(&without, "ancestor"),
        property_codes(&with, "ancestor")
    );
}

#[test]
fn lookup_unknown_code_errors() {
    let (_d, db) = build_db();
    assert!(ops::lookup(&conn(&db), "99999999", &[]).is_err());
}

#[test]
fn validate_code_known_and_unknown() {
    let (_d, db) = build_db();
    let c = conn(&db);
    assert_eq!(
        param_bool(&ops::validate_code(&c, "22298006", None).unwrap(), "result"),
        Some(true)
    );
    assert_eq!(
        param_bool(&ops::validate_code(&c, "99999999", None).unwrap(), "result"),
        Some(false)
    );
    assert_eq!(
        param_bool(
            &ops::validate_code(&c, "22298006", Some("Myocardial infarction")).unwrap(),
            "result"
        ),
        Some(true)
    );
    assert_eq!(
        param_bool(
            &ops::validate_code(&c, "22298006", Some("Definitely not myocardial infarction"))
                .unwrap(),
            "result"
        ),
        Some(false)
    );
}

#[test]
fn subsumes_all_outcomes() {
    let (_d, db) = build_db();
    let c = conn(&db);
    let outcome = |a: &str, b: &str| {
        param_code(&ops::subsumes(&c, a, b).unwrap(), "outcome")
            .unwrap()
            .to_string()
    };
    assert_eq!(outcome("46635009", "73211009"), "subsumed-by"); // Type 1 DM is-a DM
    assert_eq!(outcome("73211009", "46635009"), "subsumes");
    assert_eq!(outcome("73211009", "73211009"), "equivalent");
    assert_eq!(outcome("195967001", "22298006"), "not-subsumed"); // Asthma vs MI

    let error = ops::subsumes(&c, "not-an-sctid", "73211009").unwrap_err();
    assert_eq!(error.status, 400);
    assert_eq!(error.code, "invalid");
}

#[test]
fn expand_ecl_filter_and_combined() {
    let (_d, db) = build_db();
    let c = conn(&db);

    let v = ops::expand(
        &c,
        Some("<<73211009"),
        None,
        100,
        0,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    assert_eq!(v["resourceType"], "ValueSet");
    assert_eq!(v["expansion"]["total"], 3);
    let mut codes = contains_codes(&v);
    codes.sort();
    assert_eq!(codes, ["44054006", "46635009", "73211009"]);

    let v = ops::expand(&c, None, Some("diabetes"), 100, 0, false, true, None, None).unwrap();
    assert!(contains_codes(&v).contains(&"73211009".to_string()));

    // ECL ∩ text filter: clinical findings under root, filtered to "diabetes".
    let v = ops::expand(
        &c,
        Some("<<404684003"),
        Some("diabetes"),
        100,
        0,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    let codes = contains_codes(&v);
    assert!(codes.contains(&"73211009".to_string()));
    assert!(!codes.contains(&"22298006".to_string())); // MI is not a "diabetes" match
}

#[test]
fn expand_rejects_an_overflowing_sctid_as_invalid_ecl() {
    let (_d, db) = build_db();
    let c = conn(&db);
    for ecl in [
        "999999999999999999999999",
        "<<999999999999999999999999",
        "999999999999999999999999 OR 73211009",
    ] {
        let error = ops::expand(&c, Some(ecl), None, 100, 0, false, true, None, None).unwrap_err();
        assert_eq!(error.status, 400, "{ecl}");
        assert_eq!(error.code, "invalid", "{ecl}");
    }
}

/// R53: a compound (non fast-path) ECL expression is bounded so a remote
/// client cannot force unbounded in-memory materialisation. `expand`'s SQL
/// fast path only ever handles a single bare hierarchy/refset operator (see
/// `expand_fast_path_with_tct_matches` etc.); a boolean `OR` of two branches
/// always falls through to the general engine, where `EvalLimits` applies.
/// This uses `expand_with_cap_for_tests` (a `#[doc(hidden)]` test seam) with
/// a small cap rather than the real 100k production ceiling, because
/// reaching that scale would need >100k rows of real data that the small
/// committed synthetic fixture deliberately doesn't have - the mechanism
/// under test (parse -> bounded evaluation -> `classify_ecl_error` -> FHIR
/// `too-costly`) is identical either way; only the threshold differs.
#[test]
fn expand_caps_compound_ecl_materialisation_and_reports_too_costly() {
    let (_d, db) = build_db();
    let c = conn(&db);
    // Two disjoint hierarchy branches unioned: 4 distinct active concepts in
    // the fixture (73211009's subtree {73211009, 46635009, 44054006} plus
    // the unrelated 22298006).
    const ECL: &str = "<<73211009 OR 22298006";

    let error =
        ops::expand_with_cap_for_tests(&c, Some(ECL), None, 100, 0, false, true, None, 2, None)
            .unwrap_err();
    assert_eq!(error.status, 403);
    assert_eq!(error.code, "too-costly");
    assert!(
        error.diagnostics.contains("more than 2 concepts"),
        "{}",
        error.diagnostics
    );

    // Sanity: the identical expression succeeds once it fits under the cap -
    // proving the rejection above is really about the cap, not a broken
    // expression or an off-by-one in the check.
    let ok =
        ops::expand_with_cap_for_tests(&c, Some(ECL), None, 100, 0, false, true, None, 10, None)
            .unwrap();
    let mut codes = contains_codes(&ok);
    codes.sort();
    assert_eq!(codes, ["22298006", "44054006", "46635009", "73211009"]);
}

/// R53: an already-overdue deadline interrupts compound ECL evaluation
/// before it returns a result, rather than the request only being abandoned
/// at the HTTP layer while database work continues in the background (the
/// gap left open by `R73`, per the `R44`/`R53` audit history). The result
/// cap is set to `usize::MAX` so only the deadline mechanism is exercised
/// here; `DeadlineGuard`'s own interruption of a genuinely slow SQL
/// statement is covered independently in
/// `commands::serve::ops::deadline_guard_tests`.
#[test]
fn expand_deadline_interrupts_evaluation_before_returning_a_result() {
    let (_d, db) = build_db();
    let c = conn(&db);
    let overdue = Instant::now() - Duration::from_secs(1);

    let error = ops::expand_with_cap_for_tests(
        &c,
        Some("<<73211009 OR 22298006"),
        None,
        100,
        0,
        false,
        true,
        Some(overdue),
        usize::MAX,
        None,
    )
    .unwrap_err();
    assert_eq!(error.status, 408);
    assert_eq!(error.code, "timeout");
    // It must not have quietly computed and returned the correct 4-concept
    // answer anyway - that would mean the deadline check is a no-op.
    assert!(
        error.diagnostics.contains("time budget"),
        "{}",
        error.diagnostics
    );
}

/// The fast path (`expand_simple`) answers a bare hierarchy operator without
/// going through `eval_ecl`, so it needs the deadline enforced at the entry
/// point rather than around the evaluator. On a database with no
/// transitive-closure table its COUNT is an unlimited recursive CTE, which is
/// the most expensive statement a short remote request can trigger - so an
/// exhausted budget must stop it rather than let it run to completion.
#[test]
fn expand_deadline_also_bounds_the_simple_fast_path() {
    let (_d, db) = build_db();
    let c = conn(&db);

    // Same expression, no deadline: takes the fast path and succeeds, so the
    // 408 below is attributable to the deadline and not to a broken query.
    let ok = ops::expand(&c, Some("<<73211009"), None, 10, 0, false, true, None, None).unwrap();
    assert!(
        ok["expansion"]["total"].as_u64().unwrap() > 0,
        "fast path should return the diabetes subtree: {ok}"
    );

    let error = ops::expand(
        &c,
        Some("<<73211009"),
        None,
        10,
        0,
        false,
        true,
        Some(Instant::now() - Duration::from_secs(1)),
        None,
    )
    .unwrap_err();
    assert_eq!(error.status, 408);
    assert_eq!(error.code, "timeout");
}

#[test]
fn expand_pagination() {
    let (_d, db) = build_db();
    let v = ops::expand(
        &conn(&db),
        Some("<<73211009"),
        None,
        2,
        0,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    assert_eq!(v["expansion"]["total"], 3); // total reflects the full set
    assert_eq!(contains_codes(&v).len(), 2); // page is capped at count
}

#[test]
fn expand_fast_path_with_tct_matches() {
    // Same hierarchy expansion against a DB that has the transitive-closure
    // table - exercises the TCT branch of the SQL fast path.
    let (_d, db) = build_db_with(true);
    let v = ops::expand(
        &conn(&db),
        Some("<<73211009"),
        None,
        100,
        0,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    assert_eq!(v["expansion"]["total"], 3);
    let mut codes = contains_codes(&v);
    codes.sort();
    assert_eq!(codes, ["44054006", "46635009", "73211009"]);
}

#[test]
fn expand_refset_member_fast_path() {
    let (_d, db) = build_db();
    let v = ops::expand(
        &conn(&db),
        Some("^991381000000107"),
        None,
        100,
        0,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    let mut codes = contains_codes(&v);
    codes.sort();
    assert_eq!(codes, ["44054006", "46635009"]);
}

#[test]
fn expand_refinement_falls_back_to_engine() {
    let (_d, db) = build_db();
    // Attribute refinement is not a simple candidate, so it routes through the
    // full ECL engine - still correct, just not the SQL fast path.
    let v = ops::expand(
        &conn(&db),
        Some("<<404684003 : 363698007 = <<74281007"),
        None,
        100,
        0,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    assert_eq!(contains_codes(&v), ["22298006"]);
}

/// R16: `activeOnly` (default true) on the SQL fast path (`expand_simple` /
/// `body_sql`), both without and with the transitive-closure table, so both
/// the recursive-CTE and TCT-indexed branches are covered. `9468002` is the
/// fixture's one inactive concept, an IS-A child of `404684003`.
#[test]
fn expand_active_only_filters_the_fast_path_descendant_body() {
    for tct in [false, true] {
        let (_d, db) = build_db_with_inactive(tct, true);
        let c = conn(&db);

        let default = ops::expand(
            &c,
            Some("<404684003"),
            None,
            100,
            0,
            false,
            true,
            None,
            None,
        )
        .unwrap();
        assert!(
            !contains_codes(&default).contains(&"9468002".to_string()),
            "tct={tct}: activeOnly defaults to true, should exclude the inactive concept: {default}"
        );

        let opt_in = ops::expand(
            &c,
            Some("<404684003"),
            None,
            100,
            0,
            false,
            false,
            None,
            None,
        )
        .unwrap();
        assert!(
            contains_codes(&opt_in).contains(&"9468002".to_string()),
            "tct={tct}: activeOnly=false should include the inactive concept: {opt_in}"
        );
        assert_eq!(
            opt_in["expansion"]["total"].as_u64().unwrap(),
            default["expansion"]["total"].as_u64().unwrap() + 1,
            "tct={tct}: activeOnly=false should add exactly the one inactive concept"
        );
    }
}

/// R16: `activeOnly` on a bare-concept fast-path expand (`Op::None`, the
/// `self`-only slot in `expand_simple`) - covers the case where the *focus*
/// concept itself, not just a body member, is inactive.
#[test]
fn expand_active_only_filters_a_bare_inactive_concept() {
    let (_d, db) = build_db_with_inactive(false, true);
    let c = conn(&db);

    let default = ops::expand(&c, Some("9468002"), None, 100, 0, false, true, None, None).unwrap();
    assert_eq!(default["expansion"]["total"], 0);
    assert!(contains_codes(&default).is_empty());

    let opt_in = ops::expand(&c, Some("9468002"), None, 100, 0, false, false, None, None).unwrap();
    assert_eq!(opt_in["expansion"]["total"], 1);
    assert_eq!(contains_codes(&opt_in), ["9468002"]);
}

/// R16: `activeOnly` on the general (non fast-path) ECL engine - a boolean
/// `OR` always falls through to `eval_ecl`, which is itself `activeOnly`-
/// agnostic (shared with the CLI/SDK/MCP), so filtering happens afterwards
/// via `filter_active_ids`.
#[test]
fn expand_active_only_filters_the_compound_ecl_path() {
    let (_d, db) = build_db_with_inactive(false, true);
    let c = conn(&db);
    const ECL: &str = "<404684003 OR 22298006";

    let default = ops::expand(&c, Some(ECL), None, 100, 0, false, true, None, None).unwrap();
    assert!(!contains_codes(&default).contains(&"9468002".to_string()));

    let opt_in = ops::expand(&c, Some(ECL), None, 100, 0, false, false, None, None).unwrap();
    assert!(contains_codes(&opt_in).contains(&"9468002".to_string()));
}

/// R16: `activeOnly` on the implicit whole-CodeSystem wildcard expansion
/// (`ecl=None, filter=None`).
#[test]
fn expand_active_only_filters_the_wildcard_path() {
    let (_d, db) = build_db_with_inactive(false, true);
    let c = conn(&db);

    let default = ops::expand(&c, None, None, 1000, 0, false, true, None, None).unwrap();
    assert!(!contains_codes(&default).contains(&"9468002".to_string()));

    let opt_in = ops::expand(&c, None, None, 1000, 0, false, false, None, None).unwrap();
    assert!(contains_codes(&opt_in).contains(&"9468002".to_string()));
    assert_eq!(
        opt_in["expansion"]["total"].as_u64().unwrap(),
        default["expansion"]["total"].as_u64().unwrap() + 1
    );
}

/// R16: `activeOnly` on the FTS5 text-filter path (`fts_ids`). `9468002`'s
/// FSN is "Inactive example disorder (disorder)".
#[test]
fn expand_active_only_filters_the_text_filter_path() {
    let (_d, db) = build_db_with_inactive(false, true);
    let c = conn(&db);

    let default = ops::expand(
        &c,
        None,
        Some("inactive example"),
        100,
        0,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    assert!(contains_codes(&default).is_empty());

    let opt_in = ops::expand(
        &c,
        None,
        Some("inactive example"),
        100,
        0,
        false,
        false,
        None,
        None,
    )
    .unwrap();
    assert_eq!(contains_codes(&opt_in), ["9468002"]);
}

/// R16, live over real HTTP: the `activeOnly` query parameter round-trips
/// through `pagination()`/`expand()` on the production request path.
#[test]
fn http_expand_active_only_query_param_round_trip() {
    let (_d, db) = build_db_with_inactive(false, true);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    let ecl_param = "url=http://snomed.info/sct?fhir_vs=ecl/%3C404684003";

    let default: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?{ecl_param}"
    )))
    .unwrap();
    assert!(!contains_codes(&default).contains(&"9468002".to_string()));

    let opt_in: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?{ecl_param}&activeOnly=false"
    )))
    .unwrap();
    assert!(contains_codes(&opt_in).contains(&"9468002".to_string()));
}

/// R16: [`ops::resolve_display_language`] in isolation - `sct` only ever
/// loads English-language SNOMED CT content (see
/// `crate::builder::language_refset_priority`), so any English variant is
/// honoured verbatim while anything else falls back to bare `en`, and no
/// request at all resolves to no parameter (the caller omits
/// `expansion.parameter` entirely rather than reporting a language nobody
/// asked about).
#[test]
fn resolve_display_language_honours_english_falls_back_otherwise() {
    assert_eq!(ops::resolve_display_language(None), None);
    assert_eq!(
        ops::resolve_display_language(Some("en")),
        Some("en".to_string())
    );
    assert_eq!(
        ops::resolve_display_language(Some("en-GB")),
        Some("en-GB".to_string())
    );
    assert_eq!(
        ops::resolve_display_language(Some("en-US")),
        Some("en-US".to_string())
    );
    // Case- and separator-insensitive on the primary subtag, like
    // `language_refset_priority`.
    assert_eq!(
        ops::resolve_display_language(Some("EN_gb")),
        Some("EN_gb".to_string())
    );
    // Not English at all: falls back to bare `en`, the only language `sct`
    // ever loads, rather than silently echoing an unhonoured request.
    assert_eq!(
        ops::resolve_display_language(Some("fr")),
        Some("en".to_string())
    );
    assert_eq!(
        ops::resolve_display_language(Some("es-ES")),
        Some("en".to_string())
    );
}

/// R16: an `$expand` with no `displayLanguage` requested omits the
/// `expansion.parameter` entry entirely - `sct` should not volunteer a
/// language claim nobody asked about.
#[test]
fn expand_without_display_language_omits_the_parameter() {
    let (_d, db) = build_db();
    let c = conn(&db);
    let v = ops::expand(
        &c,
        Some("<<73211009"),
        None,
        100,
        0,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    let names: Vec<&str> = v["expansion"]["parameter"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"displayLanguage"), "{v}");
}

/// R16: `includeDesignations=true` adds direct FHIR designation objects, not
/// the `Parameters.parameter` wrapper used by `CodeSystem/$lookup`. Exercise
/// both a hierarchy fast path and the general ECL evaluator.
#[test]
fn expand_include_designations_returns_fsn_and_synonyms() {
    let (_d, db) = build_db();
    let c = conn(&db);

    let fast = ops::expand(&c, Some("22298006"), None, 100, 0, true, true, None, None).unwrap();
    assert_eq!(
        expansion_designations(&fast, "22298006"),
        vec![
            "Myocardial infarction (disorder)".to_string(),
            "Heart attack".to_string(),
        ]
    );
    let designation = &fast["expansion"]["contains"][0]["designation"][0];
    assert_eq!(designation["use"]["code"], "900000000000003001");
    assert_eq!(designation["value"], "Myocardial infarction (disorder)");
    assert!(
        designation.get("name").is_none(),
        "must not be a Parameters entry"
    );

    let general = ops::expand(
        &c,
        Some("22298006 OR 46635009"),
        None,
        100,
        0,
        true,
        true,
        None,
        None,
    )
    .unwrap();
    assert!(expansion_designations(&general, "22298006").contains(&"Heart attack".to_string()));
}

/// R16: the `designation` `$expand` parameter selects *which* designations
/// come back (FSN vs Synonym) rather than which concepts do, per the R4
/// operation definition - exercised here as the post-filter the HTTP
/// handlers in `serve::mod` apply to an `includeDesignations=true` expansion.
#[test]
fn expand_designation_filters_which_designations_are_returned() {
    let (_d, db) = build_db();
    let c = conn(&db);
    let expand_with_designations =
        || ops::expand(&c, Some("22298006"), None, 100, 0, true, true, None, None).unwrap();

    let mut fsn_only = expand_with_designations();
    ops::apply_designation_filter(
        &mut fsn_only,
        &["http://snomed.info/sct|900000000000003001".to_string()],
    );
    assert_eq!(
        expansion_designations(&fsn_only, "22298006"),
        vec!["Myocardial infarction (disorder)".to_string()]
    );

    let mut synonyms_only = expand_with_designations();
    ops::apply_designation_filter(&mut synonyms_only, &["900000000000013009".to_string()]);
    assert_eq!(
        expansion_designations(&synonyms_only, "22298006"),
        vec!["Heart attack".to_string()]
    );

    let both = expansion_designations(&expand_with_designations(), "22298006");

    // "*" and a bare "en" language tag both mean "everything" - `sct` serves
    // a single English locale, so there is no narrower subset a language tag
    // could select.
    let mut wildcard = expand_with_designations();
    ops::apply_designation_filter(&mut wildcard, &["*".to_string()]);
    assert_eq!(expansion_designations(&wildcard, "22298006"), both);

    let mut english = expand_with_designations();
    ops::apply_designation_filter(&mut english, &["en".to_string()]);
    assert_eq!(expansion_designations(&english, "22298006"), both);

    let mut english_dialect = expand_with_designations();
    ops::apply_designation_filter(&mut english_dialect, &["en-GB".to_string()]);
    assert_eq!(expansion_designations(&english_dialect, "22298006"), both);

    // A token's system is part of the match. Reusing a SNOMED description-type
    // code under another system must not select a SNOMED designation.
    let mut wrong_system = expand_with_designations();
    ops::apply_designation_filter(
        &mut wrong_system,
        &["http://example.org|900000000000003001".to_string()],
    );
    assert!(expansion_designations(&wrong_system, "22298006").is_empty());

    // Unknown bare tokens are codes, not automatically language tags.
    let mut unknown_code = expand_with_designations();
    ops::apply_designation_filter(&mut unknown_code, &["english".to_string()]);
    assert!(expansion_designations(&unknown_code, "22298006").is_empty());

    // Any other language tag matches no designation, since there is no other
    // locale to return - and the `designation` key must be dropped entirely
    // rather than left as an empty array.
    let mut french = expand_with_designations();
    ops::apply_designation_filter(&mut french, &["fr".to_string()]);
    assert!(expansion_designations(&french, "22298006").is_empty());
    assert!(
        french["expansion"]["contains"][0]
            .get("designation")
            .is_none(),
        "an empty designation list must be dropped entirely, not left as []"
    );

    // No `designation` tokens at all: a no-op, leaving `includeDesignations`
    // alone to decide.
    let mut untouched = expand_with_designations();
    let before = untouched.clone();
    ops::apply_designation_filter(&mut untouched, &[]);
    assert_eq!(untouched, before);
}

#[test]
fn expand_omits_designations_unless_requested() {
    let (_d, db) = build_db();
    let c = conn(&db);
    let v = ops::expand(&c, Some("22298006"), None, 100, 0, false, true, None, None).unwrap();
    assert!(v["expansion"]["contains"][0].get("designation").is_none());
}

/// R16: `displayLanguage` on the fast path, the general ECL path, and the
/// wildcard path all report the resolved language back on
/// `expansion.parameter` - an English request is echoed verbatim, a
/// non-English request falls back to `en`.
#[test]
fn expand_display_language_is_reported_on_expansion_parameter() {
    let (_d, db) = build_db();
    let c = conn(&db);

    // Fast path (bare hierarchy operator).
    let gb = ops::expand(
        &c,
        Some("<<73211009"),
        None,
        100,
        0,
        false,
        true,
        None,
        Some("en-GB"),
    )
    .unwrap();
    assert_eq!(expansion_display_language(&gb), Some("en-GB".to_string()));

    // General (compound) ECL path.
    let fr = ops::expand(
        &c,
        Some("<<73211009 OR 22298006"),
        None,
        100,
        0,
        false,
        true,
        None,
        Some("fr"),
    )
    .unwrap();
    assert_eq!(expansion_display_language(&fr), Some("en".to_string()));

    // Whole-CodeSystem wildcard path.
    let de = ops::expand(&c, None, None, 100, 0, false, true, None, Some("de-DE")).unwrap();
    assert_eq!(expansion_display_language(&de), Some("en".to_string()));
}

/// R16, live over real HTTP: the `displayLanguage` query parameter round-trips
/// through `pagination`-adjacent parsing and `expand()` on the production
/// request path, matching the `activeOnly` round-trip above.
#[test]
fn http_expand_display_language_query_param_round_trip() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    let ecl_param = "url=http://snomed.info/sct?fhir_vs=ecl/%3C%3C73211009";

    let none: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?{ecl_param}"
    )))
    .unwrap();
    assert_eq!(expansion_display_language(&none), None);

    let honoured: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?{ecl_param}&displayLanguage=en-US"
    )))
    .unwrap();
    assert_eq!(
        expansion_display_language(&honoured),
        Some("en-US".to_string())
    );

    let fallback: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?{ecl_param}&displayLanguage=cy"
    )))
    .unwrap();
    assert_eq!(
        expansion_display_language(&fallback),
        Some("en".to_string())
    );
}

/// R16: the production HTTP handler parses `includeDesignations` and uses the
/// expansion-contained designation shape, rather than the `$lookup` wrapper.
#[test]
fn http_expand_include_designations_query_param_round_trip() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    let value: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl/22298006&includeDesignations=true"
    )))
    .unwrap();
    assert_eq!(
        expansion_designations(&value, "22298006"),
        vec![
            "Myocardial infarction (disorder)".to_string(),
            "Heart attack".to_string(),
        ]
    );
}

/// The `designation` query parameter must reach the HTTP handler (not just
/// `ops::apply_designation_filter` directly), and its presence must imply
/// designations are wanted even without a separate `includeDesignations=true`,
/// per the R4 operation definition: "if this parameter is present, the
/// request is honored as if includeDesignations is true".
#[test]
fn http_expand_designation_query_param_round_trip() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    let value: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl/22298006&designation=900000000000013009"
    )))
    .unwrap();
    assert_eq!(
        expansion_designations(&value, "22298006"),
        vec!["Heart attack".to_string()]
    );
}

/// FHIR's standard `$expand` invocation is a POST carrying a `Parameters`
/// resource, and that is the only way to send an inline `valueSet`. `sct`
/// reads the query string only, so the body was discarded - leaving `$expand`
/// with no value set, which it treated as "expand everything". A request for
/// two concepts came back as the entire code system, HTTP 200.
#[test]
fn http_expand_refuses_a_body_it_cannot_read_rather_than_expanding_everything() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");

    // Wait for readiness via a request that is expected to succeed.
    let _ = get_with_retry(&format!(
        "{base}/ValueSet/$expand?url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs"
    ));

    let inline = r#"{"resourceType":"Parameters","parameter":[{"name":"valueSet","resource":{
        "resourceType":"ValueSet","status":"active","compose":{"include":[{
        "system":"http://snomed.info/sct","concept":[{"code":"22298006"},{"code":"73211009"}]}]}}}]}"#;
    let err = ureq::post(&format!("{base}/ValueSet/$expand"))
        .header("Content-Type", "application/fhir+json")
        .send(inline)
        .unwrap_err();
    assert!(
        matches!(err, ureq::Error::StatusCode(400)),
        "an inline valueSet must be refused, never answered with the whole code system"
    );

    // A POST that puts its parameters in the query string, as this server
    // documents, still works.
    let ok = ureq::post(&format!(
        "{base}/ValueSet/$expand?url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs%3Disa%2F73211009"
    ))
    .send_empty()
    .unwrap()
    .into_body()
    .read_to_string()
    .unwrap();
    let ok: Value = serde_json::from_str(&ok).unwrap();
    assert_eq!(ok["expansion"]["total"], 3);
}

/// The SNOMED CT R4 page defines five implicit value set URL forms. Only
/// `ecl/` was implemented; `isa/` and `refset/` fell through to "no ECL",
/// which `$expand` reads as *the whole code system*. A client asking for the
/// descendants of one concept got every concept in the edition, with a 200 and
/// nothing to indicate a different value set had been substituted.
#[test]
fn implicit_isa_and_refset_forms_expand_to_the_right_value_set() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    let total = |url: &str| -> u64 {
        let v: Value = serde_json::from_str(&get_with_retry(url)).unwrap();
        v["expansion"]["total"].as_u64().unwrap()
    };

    // `isa/73211009` is defined as "including the concept itself", i.e. `<<`.
    let isa = total(&format!(
        "{base}/ValueSet/$expand?url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs%3Disa%2F73211009"
    ));
    let ecl = total(&format!(
        "{base}/ValueSet/$expand?url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs%3Decl%2F%3C%3C73211009"
    ));
    assert_eq!(isa, ecl, "isa/[sctid] must agree with ecl/<<[sctid]");

    // And must be a strict subset of the whole code system, not equal to it -
    // the exact confusion the old code made.
    let everything = total(&format!(
        "{base}/ValueSet/$expand?url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs"
    ));
    assert!(
        isa < everything,
        "isa/ returned the whole code system ({isa} of {everything})"
    );

    // A defined-but-unimplemented form is refused, not silently substituted.
    let err = ureq::get(&format!(
        "{base}/ValueSet/$expand?url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs%3Drefset"
    ))
    .call()
    .unwrap_err();
    assert!(matches!(err, ureq::Error::StatusCode(400)));

    // An entirely unknown value set is a 404, not everything.
    let err = ureq::get(&format!(
        "{base}/ValueSet/$expand?url=http%3A%2F%2Fexample.org%2FValueSet%2Fnope"
    ))
    .call()
    .unwrap_err();
    assert!(matches!(err, ureq::Error::StatusCode(404)));
}

/// R16: `includeDefinition=true` returns the value set's *definition*
/// alongside its expansion. For an implicit SNOMED value set that definition
/// is the `constraint`/`=` filter from the R4 SNOMED CT templates - not the
/// expanded members restated.
#[test]
fn expand_include_definition_emits_the_implicit_constraint_filter() {
    let (_d, db) = build_db();
    let c = conn(&db);

    let mut vs = ops::expand(
        &c,
        Some("<<73211009"),
        None,
        100,
        0,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    assert!(
        vs.get("compose").is_none(),
        "definition must be opt-in, not volunteered"
    );

    fhir::attach_definition(
        &mut vs,
        fhir::implicit_valueset_definition(Some("<<73211009")),
    );
    let include = &vs["compose"]["include"][0];
    assert_eq!(include["system"], "http://snomed.info/sct");
    assert_eq!(include["filter"][0]["property"], "constraint");
    assert_eq!(include["filter"][0]["op"], "=");
    assert_eq!(include["filter"][0]["value"], "<<73211009");
    assert_eq!(
        vs["url"], "http://snomed.info/sct?fhir_vs=ecl/<<73211009",
        "the definition should identify the implicit value set it came from"
    );
    // Attaching a definition must not disturb the expansion itself.
    assert!(vs["expansion"]["contains"].as_array().unwrap().len() > 1);
    assert_eq!(vs["resourceType"], "ValueSet");

    // The bare `?fhir_vs` form is the whole code system: no filter.
    let whole = fhir::implicit_valueset_definition(None);
    assert_eq!(
        whole["compose"]["include"][0]["system"],
        "http://snomed.info/sct"
    );
    assert!(whole["compose"]["include"][0].get("filter").is_none());
    assert_eq!(whole["url"], "http://snomed.info/sct?fhir_vs");
}

/// SNOMED's URI specification says a bare release date is not a safe version,
/// and `sct` cannot build the full edition URI, so the definition must not
/// claim a version at all rather than publish a non-conformant one.
#[test]
fn implicit_definition_omits_the_version_it_cannot_state_conformantly() {
    let def = fhir::implicit_valueset_definition(Some("<<73211009"));
    assert!(def.get("version").is_none(), "{def}");
    assert!(
        def["copyright"].as_str().unwrap().contains("SNOMED CT"),
        "the SNOMED licensing statement belongs on a published definition"
    );
}

/// A definition must never silently overwrite the expansion's own fields.
#[test]
fn attach_definition_never_overwrites_expansion_fields() {
    let mut vs = serde_json::json!({
        "resourceType": "ValueSet",
        "status": "active",
        "url": "http://x/ValueSet/original",
        "expansion": { "total": 7 },
    });
    fhir::attach_definition(
        &mut vs,
        serde_json::json!({
            "resourceType": "Nonsense",
            "status": "draft",
            "url": "http://x/ValueSet/other",
            "expansion": { "total": 0 },
            "compose": { "include": [] },
        }),
    );
    assert_eq!(vs["resourceType"], "ValueSet");
    assert_eq!(vs["status"], "active");
    assert_eq!(vs["url"], "http://x/ValueSet/original");
    assert_eq!(vs["expansion"]["total"], 7);
    assert!(vs.get("compose").is_some(), "new fields are still merged");
}

/// R16, live over HTTP: a stored `.codelist` ValueSet expanded with
/// `includeDefinition=true` carries its own `compose`, and omits it otherwise.
#[test]
fn http_expand_include_definition_round_trip() {
    let (_d, db) = build_db();
    let dir = codelist_dir();
    let cpath = dir.path().to_path_buf();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", Some(cpath), None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");

    let without: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/diabetes/$expand"
    )))
    .unwrap();
    assert!(without.get("compose").is_none());

    let with: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/diabetes/$expand?includeDefinition=true"
    )))
    .unwrap();
    assert_eq!(with["resourceType"], "ValueSet");
    assert!(
        with["compose"]["include"][0]["concept"]
            .as_array()
            .is_some_and(|c| !c.is_empty()),
        "a stored codelist's definition is its enumerated concepts: {with}"
    );
    assert!(with["expansion"]["contains"]
        .as_array()
        .is_some_and(|c| !c.is_empty()));

    // The implicit path over HTTP too.
    let implicit: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl/22298006&includeDefinition=true"
    )))
    .unwrap();
    assert_eq!(
        implicit["compose"]["include"][0]["filter"][0]["value"],
        "22298006"
    );
}

/// R16: `check-system-version` is the client saying "I will not accept
/// terminology from any other release". A pin matching the loaded release
/// passes; a mismatch must fail the whole expansion rather than quietly
/// serving a different version.
#[test]
fn check_system_version_passes_on_match_and_fails_on_mismatch() {
    let (_d, db) = build_db();
    let c = conn(&db);

    // The synthetic fixture's recorded release date.
    let loaded = "2026-01-01";

    assert!(
        ops::check_system_versions(&c, &[format!("http://snomed.info/sct|{loaded}")]).is_ok(),
        "a pin naming the loaded release must be honoured"
    );

    let err = ops::check_system_versions(&c, &["http://snomed.info/sct|20990101".to_string()])
        .expect_err("a mismatched pin must not expand");
    assert_eq!(err.status, 400);
    assert!(
        err.diagnostics.contains("20990101") && err.diagnostics.contains(loaded),
        "diagnostics should name both the demanded and the loaded version: {}",
        err.diagnostics
    );

    // Any of several pins disagreeing is enough to refuse.
    assert!(ops::check_system_versions(
        &c,
        &[
            format!("http://snomed.info/sct|{loaded}"),
            "http://snomed.info/sct|20990101".to_string(),
        ],
    )
    .is_err());
}

/// A pin `sct` has no opinion about must not be turned into a spurious error:
/// no other code system ever contributes codes to an expansion here, and a
/// bare system with no `|version` states no requirement at all.
#[test]
fn check_system_version_ignores_other_systems_and_versionless_pins() {
    let (_d, db) = build_db();
    let c = conn(&db);

    assert!(ops::check_system_versions(&c, &["http://loinc.org|2.62".to_string()]).is_ok());
    assert!(ops::check_system_versions(&c, &["http://snomed.info/sct".to_string()]).is_ok());
    assert!(ops::check_system_versions(&c, &[]).is_ok());
}

/// A database with no recorded release cannot verify the client's pin. Serving
/// anyway would be precisely the silent wrong-version failure the parameter
/// exists to prevent, so this fails closed.
#[test]
fn check_system_version_fails_closed_when_the_release_is_unknown() {
    let (_d, db) = build_db();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DELETE FROM metadata;")
        .unwrap();
    let c = conn(&db);

    // Unverifiable pin: refuse.
    let err = ops::check_system_versions(&c, &["http://snomed.info/sct|2026-01-01".to_string()])
        .expect_err("an unverifiable pin must not silently pass");
    assert_eq!(err.status, 400);

    // But an expansion that never asked for a version guarantee still works.
    assert!(ops::check_system_versions(&c, &[]).is_ok());
}

/// R16, live over HTTP: a mismatched pin fails the request with a 400
/// OperationOutcome instead of returning an expansion.
#[test]
fn http_expand_check_system_version_round_trip() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    let ecl_param = "url=http://snomed.info/sct?fhir_vs=ecl/%3C%3C73211009";

    let ok: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?{ecl_param}&check-system-version=http://snomed.info/sct|2026-01-01"
    )))
    .unwrap();
    assert_eq!(ok["resourceType"], "ValueSet");

    let err = ureq::get(&format!(
        "{base}/ValueSet/$expand?{ecl_param}&check-system-version=http://snomed.info/sct|20990101"
    ))
    .call()
    .unwrap_err();
    assert!(matches!(err, ureq::Error::StatusCode(400)));
}

/// Write a `codelists/` dir with a `diabetes` list (extensional) and a
/// `dm-plus` list that composes it via `includes:`. All ids exist in the fixture.
fn codelist_dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let header = |id: &str, inc: &str| {
        format!(
            "---\nid: {id}\ntitle: {id}\ndescription: t\nterminology: SNOMED CT\n\
             created: 2026-01-01\nupdated: 2026-01-01\nversion: 2\nstatus: active\n\
             licence: CC-BY-4.0\ncopyright: x\nappropriate_use: x\nmisuse: x\n{inc}---\n\n# concepts\n"
        )
    };
    std::fs::write(
        d.path().join("diabetes.codelist"),
        format!(
            "{}46635009 Type 1 diabetes\n44054006 Type 2 diabetes\n",
            header("diabetes", "")
        ),
    )
    .unwrap();
    std::fs::write(
        d.path().join("dm-plus.codelist"),
        format!(
            "{}73211009 STALE STORED TERM\n",
            header("dm-plus", "includes:\n  - diabetes\n")
        ),
    )
    .unwrap();
    d
}

#[test]
fn valueset_registry_loads_extensional_and_composed() {
    let dir = codelist_dir();
    let reg = valuesets::load_registry(dir.path(), "http://x");
    assert_eq!(reg.len(), 2);

    // Extensional list.
    let diabetes = reg.get("diabetes").unwrap();
    let mut ids: Vec<&str> = diabetes.members.iter().map(|(i, _)| i.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["44054006", "46635009"]);

    // Composed list: own member + the included list's members.
    let dm_plus = reg.get("dm-plus").unwrap();
    let mut ids: Vec<&str> = dm_plus.members.iter().map(|(i, _)| i.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["44054006", "46635009", "73211009"]);

    // URL resolution by canonical url and by trailing-id.
    assert!(reg.resolve_url("http://x/ValueSet/diabetes").is_some());
    assert!(reg.resolve_url("anything/dm-plus").is_some());
    assert!(reg.resolve_url("nope").is_none());
}

/// Write a `codelists/` dir with one list carrying an explicit front-matter
/// `canonical_url` override and `status: active`, and one plain list with no
/// override and `status: draft` - for exercising the canonical-URL override
/// and `?status=` search filter together.
fn codelist_dir_override_and_status() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("published.codelist"),
        "---\nid: published\ntitle: published\ndescription: t\nterminology: SNOMED CT\n\
         created: 2026-01-01\nupdated: 2026-01-01\nversion: 1\nstatus: active\n\
         licence: CC-BY-4.0\ncopyright: x\nappropriate_use: x\nmisuse: x\n\
         canonical_url: https://tx.nhs.uk/ValueSet/published-list\n---\n\n# concepts\n\
         46635009 Type 1 diabetes\n",
    )
    .unwrap();
    std::fs::write(
        d.path().join("draft.codelist"),
        "---\nid: draft\ntitle: draft\ndescription: t\nterminology: SNOMED CT\n\
         created: 2026-01-01\nupdated: 2026-01-01\nversion: 1\nstatus: draft\n\
         licence: CC-BY-4.0\ncopyright: x\nappropriate_use: x\nmisuse: x\n---\n\n# concepts\n\
         44054006 Type 2 diabetes\n",
    )
    .unwrap();
    d
}

#[test]
fn valueset_registry_honours_canonical_url_override() {
    let dir = codelist_dir_override_and_status();
    let reg = valuesets::load_registry(dir.path(), "http://x");

    let published = reg.get("published").unwrap();
    assert_eq!(
        published.canonical_url,
        "https://tx.nhs.uk/ValueSet/published-list"
    );
    assert!(reg
        .resolve_url("https://tx.nhs.uk/ValueSet/published-list")
        .is_some());

    // No override: falls back to the derived `<base>/ValueSet/<id>`.
    let draft = reg.get("draft").unwrap();
    assert_eq!(draft.canonical_url, "http://x/ValueSet/draft");
}

#[test]
fn valueset_registry_rejects_duplicate_canonical_urls() {
    let dir = codelist_dir_override_and_status();
    std::fs::write(
        dir.path().join("duplicate.codelist"),
        "---\nid: duplicate\ntitle: duplicate\ndescription: t\nterminology: SNOMED CT\n\
         created: 2026-01-01\nupdated: 2026-01-01\nversion: 1\nstatus: active\n\
         licence: CC-BY-4.0\ncopyright: x\nappropriate_use: x\nmisuse: x\n\
         canonical_url: https://tx.nhs.uk/ValueSet/published-list\n---\n\n# concepts\n\
         44054006 Type 2 diabetes\n",
    )
    .unwrap();

    let reg = valuesets::load_registry(dir.path(), "http://x");
    assert_eq!(reg.len(), 2);
    let resolved = reg
        .resolve_url("https://tx.nhs.uk/ValueSet/published-list")
        .unwrap();
    assert!(matches!(
        resolved.front_matter.id.as_str(),
        "published" | "duplicate"
    ));
    assert_ne!(
        reg.get("published").is_some(),
        reg.get("duplicate").is_some()
    );
}

#[test]
fn valueset_expand_members_reconciles_display() {
    let (_d, db) = build_db();
    let dir = codelist_dir();
    let reg = valuesets::load_registry(dir.path(), "http://x");
    let members = reg.get("dm-plus").unwrap().members.clone();

    let v = ops::expand_members(&conn(&db), &members, 100, 0, false, None).unwrap();
    assert_eq!(v["expansion"]["total"], 3);
    // Display is reconciled against the live DB, not the stale stored term.
    let entry = v["expansion"]["contains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["code"] == "73211009")
        .unwrap();
    assert_ne!(entry["display"], "STALE STORED TERM");
    assert!(entry["display"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("diabet"));

    // Pagination caps the page.
    let p = ops::expand_members(&conn(&db), &members, 2, 0, false, None).unwrap();
    assert_eq!(p["expansion"]["total"], 3);
    assert_eq!(contains_codes(&p).len(), 2);

    let with_designations = ops::expand_members(&conn(&db), &members, 100, 0, true, None).unwrap();
    assert!(expansion_designations(&with_designations, "46635009")
        .iter()
        .any(|designation| designation.contains("Type 1 diabetes")));
}

#[test]
fn valueset_validate_code_membership() {
    let (_d, db) = build_db();
    let dir = codelist_dir();
    let reg = valuesets::load_registry(dir.path(), "http://x");
    let set: HashSet<String> = reg
        .get("diabetes")
        .unwrap()
        .members
        .iter()
        .map(|(i, _)| i.clone())
        .collect();
    let c = conn(&db);

    let yes = ops::validate_code_in_set(&c, &set, "46635009", "http://x/ValueSet/diabetes", None)
        .unwrap();
    assert_eq!(param_bool(&yes, "result"), Some(true));
    let no = ops::validate_code_in_set(&c, &set, "22298006", "http://x/ValueSet/diabetes", None)
        .unwrap();
    assert_eq!(param_bool(&no, "result"), Some(false));
}

/// A `display` supplied to `ValueSet/$validate-code` must be checked against
/// the member concept's own designations, not just accepted because the code
/// is a member (roadmap `R17b-validate-code`). Covers both the stored-set and
/// implicit-ECL paths [`vs_validate_code`](super) routes between.
#[test]
fn valueset_validate_code_checks_display_on_both_membership_paths() {
    let (_d, db) = build_db();
    let dir = codelist_dir();
    let reg = valuesets::load_registry(dir.path(), "http://x");
    let set: HashSet<String> = reg
        .get("diabetes")
        .unwrap()
        .members
        .iter()
        .map(|(i, _)| i.clone())
        .collect();
    let c = conn(&db);

    let matching = ops::validate_code_in_set(
        &c,
        &set,
        "46635009",
        "http://x/ValueSet/diabetes",
        Some("Type 1 diabetes mellitus"),
    )
    .unwrap();
    assert_eq!(param_bool(&matching, "result"), Some(true));

    let mismatched = ops::validate_code_in_set(
        &c,
        &set,
        "46635009",
        "http://x/ValueSet/diabetes",
        Some("Not the right display at all"),
    )
    .unwrap();
    assert_eq!(param_bool(&mismatched, "result"), Some(false));

    let ecl_matching = ops::validate_code_in_ecl(
        &c,
        "<<73211009",
        "46635009",
        None,
        Some("Type 1 diabetes mellitus"),
    )
    .unwrap();
    assert_eq!(param_bool(&ecl_matching, "result"), Some(true));

    let ecl_mismatched = ops::validate_code_in_ecl(
        &c,
        "<<73211009",
        "46635009",
        None,
        Some("Not the right display at all"),
    )
    .unwrap();
    assert_eq!(param_bool(&ecl_mismatched, "result"), Some(false));
}

/// R60: when a member code is in the stored codelist but absent from the
/// loaded `concepts` table, `$validate-code` must keep `result=true` for
/// membership but add a `message` saying the supplied `display` could not be
/// verified - not silently return an unqualified yes. Uses a fabricated SCTID
/// that is in the codelist's member set but not in the fixture's concepts
/// table (the `diabetes` codelist in `codelist_dir` is hand-curated and does
/// not contain this id, so we build a synthetic one inline).
#[test]
fn valueset_validate_code_display_unverifiable_when_member_absent_from_db() {
    let (_d, db) = build_db();
    let dir = codelist_dir();
    let reg = valuesets::load_registry(dir.path(), "http://x");
    // Use a real member from the diabetes codelist so membership is true.
    let set: HashSet<String> = reg
        .get("diabetes")
        .unwrap()
        .members
        .iter()
        .map(|(i, _)| i.clone())
        .collect();
    // Pick a member id and drop it from the concepts table to simulate the
    // "member but not in loaded build" state.
    let _set = set; // keep the real-member path for reference
    let c = conn(&db);
    // Build a minimal member set containing only a code absent from the DB.
    let absent_set: HashSet<String> = std::iter::once("9999999999".to_string()).collect();

    let no_display = ops::validate_code_in_set(
        &c,
        &absent_set,
        "9999999999",
        "http://x/ValueSet/test",
        None,
    )
    .unwrap();
    assert_eq!(
        param_bool(&no_display, "result"),
        Some(true),
        "membership is true even when the concept is absent from the DB"
    );

    let with_display = ops::validate_code_in_set(
        &c,
        &absent_set,
        "9999999999",
        "http://x/ValueSet/test",
        Some("Some display term"),
    )
    .unwrap();
    assert_eq!(
        param_bool(&with_display, "result"),
        Some(true),
        "membership stays true even when display cannot be verified"
    );
    let message = param_str(&with_display, "message").unwrap();
    assert!(
        message.contains("could not be verified"),
        "should explain the display could not be checked: {message}"
    );
}

#[test]
fn code_system_resource_identifies_snomed_without_claiming_an_edition() {
    let (_d, db) = build_db();
    let c = conn(&db);

    let cs = ops::code_system_resource(&c).unwrap();
    assert_eq!(cs["resourceType"], "CodeSystem");
    assert_eq!(cs["id"], "sct");
    assert_eq!(cs["url"], "http://snomed.info/sct");
    assert_eq!(cs["content"], "not-present");
    assert!(cs.get("version").is_none());
    assert!(cs.get("count").is_none());
}

#[test]
fn http_valueset_round_trip() {
    let (_d, db) = build_db();
    let dir = codelist_dir();
    let cpath = dir.path().to_path_buf();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", Some(cpath), None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");

    let bundle: Value = serde_json::from_str(&get_with_retry(&format!("{base}/ValueSet"))).unwrap();
    assert_eq!(bundle["resourceType"], "Bundle");
    assert_eq!(bundle["total"], 2);

    let vs: Value =
        serde_json::from_str(&get_with_retry(&format!("{base}/ValueSet/dm-plus"))).unwrap();
    assert_eq!(vs["resourceType"], "ValueSet");
    assert_eq!(
        vs["compose"]["include"][0]["concept"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let exp: Value =
        serde_json::from_str(&get_with_retry(&format!("{base}/ValueSet/dm-plus/$expand"))).unwrap();
    assert_eq!(exp["expansion"]["total"], 3);

    // Both direct expansion routes apply the same effective count cap.
    let by_id: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/dm-plus/$expand?count=5000"
    )))
    .unwrap();
    assert_eq!(expansion_count(&by_id), Some(1000));
    let by_url: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?url={base}/ValueSet/dm-plus&count=5000"
    )))
    .unwrap();
    assert_eq!(expansion_count(&by_url), Some(1000));

    let err = ureq::get(&format!(
        "{base}/ValueSet/dm-plus/$expand?count=not-a-number"
    ))
    .call()
    .unwrap_err();
    assert!(matches!(err, ureq::Error::StatusCode(400)));
}

#[test]
fn http_valueset_status_filter_and_canonical_url_override() {
    let (_d, db) = build_db();
    let dir = codelist_dir_override_and_status();
    let cpath = dir.path().to_path_buf();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", Some(cpath), None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");

    let all: Value = serde_json::from_str(&get_with_retry(&format!("{base}/ValueSet"))).unwrap();
    assert_eq!(all["total"], 2);

    let drafts: Value =
        serde_json::from_str(&get_with_retry(&format!("{base}/ValueSet?status=draft"))).unwrap();
    assert_eq!(drafts["total"], 1);
    assert_eq!(drafts["entry"][0]["resource"]["id"], "draft");

    let active: Value =
        serde_json::from_str(&get_with_retry(&format!("{base}/ValueSet?status=active"))).unwrap();
    assert_eq!(active["total"], 1);
    assert_eq!(active["entry"][0]["resource"]["id"], "published");
    assert_eq!(
        active["entry"][0]["resource"]["url"],
        "https://tx.nhs.uk/ValueSet/published-list"
    );

    // A status matching nothing returns an empty Bundle, not a 404.
    let none: Value =
        serde_json::from_str(&get_with_retry(&format!("{base}/ValueSet?status=retired"))).unwrap();
    assert_eq!(none["total"], 0);

    // The full resource read also carries the overridden canonical URL.
    let vs: Value =
        serde_json::from_str(&get_with_retry(&format!("{base}/ValueSet/published"))).unwrap();
    assert_eq!(vs["url"], "https://tx.nhs.uk/ValueSet/published-list");
}

#[test]
fn http_codesystem_round_trip() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");

    let bundle: Value =
        serde_json::from_str(&get_with_retry(&format!("{base}/CodeSystem"))).unwrap();
    assert_eq!(bundle["resourceType"], "Bundle");
    assert_eq!(bundle["total"], 1);
    assert_eq!(bundle["entry"][0]["resource"]["id"], "sct");

    let cs: Value =
        serde_json::from_str(&get_with_retry(&format!("{base}/CodeSystem/sct"))).unwrap();
    assert_eq!(cs["resourceType"], "CodeSystem");
    assert_eq!(cs["url"], "http://snomed.info/sct");
    assert_eq!(cs["content"], "not-present");
    assert!(cs.get("version").is_none());
    assert!(cs.get("count").is_none());

    // An unknown id is a 404, not a fallback to the one resource this server has.
    let err = ureq::get(&format!("{base}/CodeSystem/nope"))
        .call()
        .unwrap_err();
    assert!(matches!(err, ureq::Error::StatusCode(404)));

    // A search filter that names a different system/id yields an empty Bundle.
    let empty: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/CodeSystem?url=http://example.org/not-snomed"
    )))
    .unwrap();
    assert_eq!(empty["total"], 0);
}

#[test]
fn concept_map_translate() {
    let (_d, db) = build_db_all();
    let c = conn(&db);

    // SNOMED MI -> ICD-10 I21.9 (by FHIR system URIs).
    let v = ops::translate(
        &c,
        "http://snomed.info/sct",
        "22298006",
        "http://hl7.org/fhir/sid/icd-10",
    )
    .unwrap();
    assert_eq!(param_bool(&v, "result"), Some(true));
    let matches: Vec<&Value> = v["parameter"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["name"] == "match")
        .collect();
    assert_eq!(matches.len(), 1);
    let coding = matches[0]["part"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "concept")
        .unwrap()["valueCoding"]
        .clone();
    assert_eq!(coding["code"], "I219");
    assert_eq!(coding["system"], "http://hl7.org/fhir/sid/icd-10");

    // The fixture's ExtendedMap rows all use correlationId 447561005 ("not
    // specified"), which is why this stays "relatedto" - the conservative
    // default, not evidence the equivalence logic never ran.
    let equivalence = matches[0]["part"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "equivalence")
        .unwrap()["valueCode"]
        .clone();
    assert_eq!(equivalence, "relatedto");

    // Bare names + reverse (ICD-10 -> SNOMED) carries the SNOMED display.
    let r = ops::translate(&c, "icd10", "I219", "snomed").unwrap();
    let cd = r["parameter"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "match")
        .unwrap()["part"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "concept")
        .unwrap()["valueCoding"]
        .clone();
    assert_eq!(cd["code"], "22298006");
    assert_eq!(cd["display"], "Myocardial infarction");

    // No map -> result=false (asthma has no ICD-10 map in the fixture).
    let none = ops::translate(&c, "snomed", "195967001", "icd10").unwrap();
    assert_eq!(param_bool(&none, "result"), Some(false));

    // Unsupported system -> error (400).
    assert!(ops::translate(&c, "http://example.org/nope", "X", "snomed").is_err());
}

/// R11: `$translate`'s reported equivalence must reflect the RF2
/// ExtendedMap's actual `correlationId`, not the fixed "relatedto" every
/// match used to report regardless of what the map data claims. The
/// committed fixture's own rows all carry the "not specified" correlation
/// (covered above), so this proves the other real values by writing them
/// into the crossmaps table directly - the same technique R11 already uses
/// elsewhere in this file to exercise data the fixture cannot express.
#[test]
fn concept_map_translate_reports_the_real_correlation_equivalence() {
    let (_d, db) = build_db_all();
    let c = conn(&db);

    let equivalence_of = |correlation: &str| -> String {
        c.execute(
            "UPDATE crossmaps SET correlation = ?1
             WHERE source_code = '73211009' AND target_system = 'icd10'",
            [correlation],
        )
        .unwrap();
        let v = ops::translate(
            &c,
            "http://snomed.info/sct",
            "73211009",
            "http://hl7.org/fhir/sid/icd-10",
        )
        .unwrap();
        v["parameter"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "match")
            .unwrap()["part"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "equivalence")
            .unwrap()["valueCode"]
            .as_str()
            .unwrap()
            .to_string()
    };

    assert_eq!(equivalence_of("447557004"), "equivalent");
    assert_eq!(equivalence_of("447558009"), "wider");
    assert_eq!(equivalence_of("447559001"), "narrower");
    assert_eq!(equivalence_of("447560006"), "inexact");
    // Back to "not specified" - the conservative default is still reachable
    // after other correlations have been reported, not a one-way switch.
    assert_eq!(equivalence_of("447561005"), "relatedto");
}

#[test]
fn http_metadata_and_lookup_round_trip() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");

    let meta: Value = serde_json::from_str(&get_with_retry(&format!("{base}/metadata"))).unwrap();
    assert_eq!(meta["resourceType"], "CapabilityStatement");
    assert_eq!(meta["fhirVersion"], "4.0.1");

    // ?mode=terminology returns a TerminologyCapabilities advertising SNOMED CT.
    let tc: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/metadata?mode=terminology"
    )))
    .unwrap();
    assert_eq!(tc["resourceType"], "TerminologyCapabilities");
    assert_eq!(tc["codeSystem"][0]["uri"], "http://snomed.info/sct");
    assert!(tc["date"].is_string());

    let url = format!("{base}/CodeSystem/$lookup?system=http://snomed.info/sct&code=22298006");
    let lookup: Value = serde_json::from_str(&get_with_retry(&url)).unwrap();
    assert_eq!(lookup["resourceType"], "Parameters");
    assert_eq!(param_str(&lookup, "display"), Some("Myocardial infarction"));

    // A batch Bundle runs several operations in one POST to the base, returning
    // a batch-response with a per-entry status (one entry deliberately fails).
    let batch_body = r#"{"resourceType":"Bundle","type":"batch","entry":[
        {"request":{"method":"GET","url":"CodeSystem/$lookup?system=http://snomed.info/sct&code=22298006"}},
        {"request":{"method":"GET","url":"CodeSystem/$lookup?system=http://snomed.info/sct&code=99999999"}},
        {"request":{"method":"GET","url":"ValueSet/$expand?url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs%3Decl%2F22298006&includeDesignations=true"}}
    ]}"#;
    let resp = ureq::post(&format!("{base}/"))
        .header("Content-Type", "application/fhir+json")
        .send(batch_body)
        .unwrap()
        .into_body()
        .read_to_string()
        .unwrap();
    let br: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(br["type"], "batch-response");
    assert_eq!(br["entry"].as_array().unwrap().len(), 3);
    assert_eq!(br["entry"][0]["response"]["status"], "200");
    assert_eq!(br["entry"][0]["resource"]["resourceType"], "Parameters");
    assert_eq!(br["entry"][1]["response"]["status"], "404"); // unknown code
    assert_eq!(br["entry"][2]["response"]["status"], "200");
    // `expansion.parameter[count]` is the page size, not the match count; the
    // encoded implicit ECL url must resolve to the single focus concept.
    assert_eq!(br["entry"][2]["resource"]["expansion"]["total"], 1);
    assert_eq!(
        expansion_designations(&br["entry"][2]["resource"], "22298006"),
        vec![
            "Myocardial infarction (disorder)".to_string(),
            "Heart attack".to_string(),
        ]
    );

    let oversized_entries = (0..101)
        .map(|_| {
            serde_json::json!({
                "request": {
                    "method": "GET",
                    "url": "CodeSystem/$lookup?code=22298006"
                }
            })
        })
        .collect::<Vec<_>>();
    let oversized = serde_json::json!({
        "resourceType": "Bundle",
        "type": "batch",
        "entry": oversized_entries,
    });
    let err = ureq::post(&format!("{base}/"))
        .header("Content-Type", "application/fhir+json")
        .send(&oversized.to_string())
        .unwrap_err();
    assert!(matches!(err, ureq::Error::StatusCode(400)));
}

/// R53, live over real HTTP: a compound (boolean) ECL `$expand` request still
/// succeeds normally through the production request path in `serve/mod.rs` -
/// which now computes a per-request deadline and installs/clears a SQLite
/// progress-handler guard around every ECL evaluation - and a second request
/// on the same small connection pool succeeds too. That second round trip is
/// the regression this guards against: if `DeadlineGuard` were left armed on
/// a pooled connection after the first request (instead of being cleared on
/// drop), the reused connection would carry a stale, already-expired
/// deadline into this unrelated later request and spuriously interrupt it.
#[test]
fn http_expand_compound_ecl_round_trip_and_pool_stays_healthy() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        // A pool of 2 makes connection reuse across the two requests below
        // near-certain.
        serve_listener(db, "/", None, None, 2, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    let url = format!(
        "{base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl/%3C%3C73211009%20OR%2022298006"
    );

    for _ in 0..2 {
        let v: Value = serde_json::from_str(&get_with_retry(&url)).unwrap();
        assert_eq!(v["resourceType"], "ValueSet");
        let mut codes = contains_codes(&v);
        codes.sort();
        assert_eq!(codes, ["22298006", "44054006", "46635009", "73211009"]);
    }
}

/// Bug-audit lead (`spec/roadmap.md`, "ECL parser robustness at the serve
/// boundary"): a hostile, pathologically nested ECL expression reaching
/// `$expand` over real HTTP must be rejected with a clean FHIR error, not
/// crash or hang the server process. `ecl::parse`'s `MAX_DEPTH` guard is
/// already covered directly in `src/ecl/parse.rs` (e.g.
/// `deeply_nested_parens_are_rejected_not_stack_overflowed`), but that only
/// proves the parser function itself is safe when called in-process; this
/// exercises the same pathological input through the actual FHIR/HTTP
/// boundary a remote client uses, and confirms the server is still healthy
/// afterwards (a stack-overflow abort would kill the whole process, not just
/// the one request, so a follow-up request on the same server is the real
/// proof of survival).
#[test]
fn http_expand_rejects_pathologically_nested_ecl_without_crashing_server() {
    let (_d, db) = build_db();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 2, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");

    // Well beyond the parser's nesting cap (200), but small enough that the
    // request itself can't be rejected by an unrelated HTTP layer limit
    // (header/URI size) - the failure under test must come from the ECL
    // depth guard, not from truncation upstream of it.
    let nested: String = "(".repeat(600) + "1" + &")".repeat(600);
    let url = format!(
        "{base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl/{}",
        urlencoding_parens(&nested)
    );
    let err = ureq::get(&url).call().unwrap_err();
    assert!(matches!(err, ureq::Error::StatusCode(400)), "{err:?}");

    // The process must still be alive and serving: a plain, valid expansion
    // on the same listener proves the pathological request didn't abort it.
    let v: Value = serde_json::from_str(&get_with_retry(&format!(
        "{base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl/22298006"
    )))
    .unwrap();
    assert_eq!(v["expansion"]["total"], 1);
}

fn urlencoding_parens(s: &str) -> String {
    s.replace('(', "%28").replace(')', "%29")
}

/// GET with a short retry loop while the background server starts accepting.
fn get_with_retry(url: &str) -> String {
    for _ in 0..50 {
        if let Ok(resp) = ureq::get(url).call() {
            return resp.into_body().read_to_string().unwrap();
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("server did not come up at {url}");
}

/// R11: `$validate-code` is where a client learns a code from an old record is
/// no longer valid, so it must say *why* and *what replaces it*, not merely
/// that the code is inactive.
#[test]
fn validate_code_on_an_inactive_concept_names_the_reason_and_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let ndjson = dir.path().join("syn.ndjson");
    let db = dir.path().join("syn.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![fixture_dir()],
        locale: "en-GB".to_string(),
        output: Some(ndjson.clone()),
        include_inactive: true,
        refsets: RefsetMode::All,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input: ndjson,
        output: Some(db.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();

    let value = ops::validate_code(&conn(&db), "9468002", None).unwrap();
    let messages: Vec<String> = value["parameter"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["name"] == "message")
        .map(|p| p["valueString"].as_str().unwrap_or_default().to_string())
        .collect();
    let joined = messages.join(" | ");

    assert!(
        joined.contains("inactive") && joined.contains("Duplicate"),
        "should give the inactivation reason: {joined}"
    );
    assert!(
        joined.contains("22298006"),
        "should name the replacement concept: {joined}"
    );
}
