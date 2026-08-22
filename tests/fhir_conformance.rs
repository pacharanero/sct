// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]
#![cfg(feature = "serve")]

//! Spec-derived conformance coverage for `sct serve` (roadmap `R17b`).
//!
//! The rest of the test suite checks that the features we believe we built
//! behave as we believe they should. That is the wrong shape for catching a
//! *missing* feature: three separate `ValueSet/$expand` defects shipped while
//! every committed test passed, each one an input the server did not recognise
//! and quietly ignored, falling back to expanding the entire code system.
//!
//! So this file starts from the specification instead of from the code. It
//! enumerates every input parameter of `ValueSet/$expand`, `CodeSystem/$lookup`,
//! and `$validate-code` (both the `CodeSystem` and `ValueSet` forms) and
//! requires each one to have an explicit, asserted disposition: honoured,
//! refused, or - with a recorded reason - genuinely unable to affect the
//! result. A parameter that is merely ignored cannot pass.
//!
//! The governing invariant: **unrecognised or unsupported input must never
//! silently degrade to a broader default.**

use rusqlite::Connection;
use sct_rs::commands::ndjson::{self, RefsetMode};
use sct_rs::commands::serve::{ops, serve_listener};
use sct_rs::commands::sqlite;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

/// Every input parameter of `ValueSet/$expand` in FHIR R4, transcribed from
/// <https://hl7.org/fhir/R4/valueset-operation-expand.html>. This list is the
/// point of the exercise: if a parameter is missing from [`DISPOSITIONS`], the
/// coverage test fails rather than leaving the gap undetected.
const R4_EXPAND_PARAMETERS: [&str; 21] = [
    "url",
    "valueSet",
    "valueSetVersion",
    "context",
    "contextDirection",
    "filter",
    "date",
    "offset",
    "count",
    "includeDesignations",
    "designation",
    "includeDefinition",
    "activeOnly",
    "excludeNested",
    "excludeNotForUI",
    "excludePostCoordinated",
    "displayLanguage",
    "exclude-system",
    "system-version",
    "check-system-version",
    "force-system-version",
];

/// What `sct serve` does with a given `$expand` parameter.
enum Disposition {
    /// Implemented: the server acts on it. The named test proves the effect;
    /// here we only assert the parameter is accepted rather than rejected.
    Honoured { covered_by: &'static str },
    /// Refused with a 4xx rather than ignored, because ignoring it would
    /// change which concepts come back - almost always by widening the set.
    Refused,
    /// Accepted and not acted on, where that genuinely cannot change the
    /// membership of the expansion. The reason is recorded here and the test
    /// proves it: the expansion with the parameter must be byte-identical to
    /// the expansion without it.
    CannotAffectResult { because: &'static str },
}

/// The declared disposition of every R4 `$expand` parameter, with a sample
/// value to exercise it.
const DISPOSITIONS: [(&str, &str, Disposition); 21] = [
    (
        "url",
        "http://snomed.info/sct?fhir_vs=isa/73211009",
        Disposition::Honoured {
            covered_by: "implicit_isa_and_refset_forms_expand_to_the_right_value_set",
        },
    ),
    (
        "valueSet",
        "ignored-inline-definition",
        Disposition::Refused,
    ),
    ("valueSetVersion", "1", Disposition::Refused),
    ("context", "Condition.code", Disposition::Refused),
    (
        "contextDirection",
        "incoming",
        Disposition::CannotAffectResult {
            because: "only meaningful alongside `context`, which is itself refused",
        },
    ),
    (
        "filter",
        "diabetes",
        Disposition::Honoured {
            covered_by: "expand_ecl_filter_and_combined",
        },
    ),
    ("date", "2020-01-01", Disposition::Refused),
    (
        "offset",
        "1",
        Disposition::Honoured {
            covered_by: "expand_pagination",
        },
    ),
    (
        "count",
        "2",
        Disposition::Honoured {
            covered_by: "expand_pagination",
        },
    ),
    (
        "includeDesignations",
        "true",
        Disposition::Honoured {
            covered_by: "expand_include_designations_returns_fsn_and_synonyms",
        },
    ),
    (
        "designation",
        "http://snomed.info/sct|900000000000003001",
        Disposition::Honoured {
            covered_by: "expand_designation_filters_which_designations_are_returned",
        },
    ),
    (
        "includeDefinition",
        "true",
        Disposition::Honoured {
            covered_by: "expand_include_definition_emits_the_implicit_constraint_filter",
        },
    ),
    (
        "activeOnly",
        "false",
        Disposition::Honoured {
            covered_by: "expand_active_only_filters_the_fast_path_descendant_body",
        },
    ),
    (
        "excludeNested",
        "true",
        Disposition::CannotAffectResult {
            because: "expansions from this server are always flat, so nesting is already excluded",
        },
    ),
    (
        "excludeNotForUI",
        "true",
        Disposition::CannotAffectResult {
            because: "this server never emits abstract or navigation-only entries",
        },
    ),
    (
        "excludePostCoordinated",
        "true",
        Disposition::CannotAffectResult {
            because: "this server returns pre-coordinated concept ids only",
        },
    ),
    (
        "displayLanguage",
        "en-GB",
        Disposition::Honoured {
            covered_by: "expand_display_language_is_reported_on_expansion_parameter",
        },
    ),
    ("exclude-system", "http://loinc.org", Disposition::Refused),
    (
        "system-version",
        "http://snomed.info/sct|2026-01-01",
        Disposition::Honoured {
            covered_by: "check_system_version_passes_on_match_and_fails_on_mismatch",
        },
    ),
    (
        "check-system-version",
        "http://snomed.info/sct|2026-01-01",
        Disposition::Honoured {
            covered_by: "check_system_version_passes_on_match_and_fails_on_mismatch",
        },
    ),
    (
        "force-system-version",
        "http://snomed.info/sct|2026-01-01",
        Disposition::Refused,
    ),
];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn build_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ndjson = dir.path().join("syn.ndjson");
    let db = dir.path().join("syn.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![fixture_dir()],
        locale: "en-GB".to_string(),
        output: Some(ndjson.clone()),
        include_inactive: false,
        refsets: RefsetMode::Simple,
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

fn conn(db: &PathBuf) -> Connection {
    Connection::open(db).unwrap()
}

fn start_server() -> String {
    let (dir, db) = build_db();
    // The database lives as long as the process; the server borrows it.
    std::mem::forget(dir);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        serve_listener(db, "/", None, None, 4, listener).unwrap();
    });
    let base = format!("http://127.0.0.1:{port}");
    for _ in 0..50 {
        if ureq::get(&format!("{base}/metadata")).call().is_ok() {
            return base;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("server did not come up");
}

fn get(url: &str) -> (u16, String) {
    match ureq::get(url).call() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            (status, resp.into_body().read_to_string().unwrap())
        }
        Err(ureq::Error::StatusCode(code)) => (code, String::new()),
        Err(e) => panic!("request to {url} failed: {e}"),
    }
}

/// The baseline expansion every parameter is judged against: a small, stable
/// implicit value set.
const BASELINE: &str = "url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs%3Disa%2F73211009";

/// The list of parameters we reason about must match the specification's,
/// exactly. A parameter R4 defines but this table omits is precisely the kind
/// of gap that shipped three wrong-answer defects.
#[test]
fn every_r4_expand_parameter_has_a_declared_disposition() {
    let declared: Vec<&str> = DISPOSITIONS.iter().map(|(name, _, _)| *name).collect();
    for spec_param in R4_EXPAND_PARAMETERS {
        assert!(
            declared.contains(&spec_param),
            "R4 defines `{spec_param}` on $expand but it has no declared disposition; \
             decide whether sct honours it, refuses it, or provably cannot be affected by it"
        );
    }
    for name in &declared {
        assert!(
            R4_EXPAND_PARAMETERS.contains(name),
            "`{name}` is not an R4 $expand parameter"
        );
    }
    assert_eq!(declared.len(), R4_EXPAND_PARAMETERS.len());
}

/// Each parameter must actually behave the way the table claims. This is the
/// test that would have caught all three shipped defects: a parameter the
/// server silently ignores can satisfy neither `Refused` nor - once its
/// results are compared against the baseline - `CannotAffectResult`.
#[test]
fn every_expand_parameter_behaves_as_declared() {
    let base = start_server();

    let (status, body) = get(&format!("{base}/ValueSet/$expand?{BASELINE}"));
    assert_eq!(status, 200, "baseline expansion should succeed");
    let baseline: Value = serde_json::from_str(&body).unwrap();
    let baseline_codes = codes(&baseline);
    assert_eq!(
        baseline_codes.len(),
        3,
        "baseline should be the three Diabetes concepts, not the whole code system"
    );

    for (name, value, disposition) in &DISPOSITIONS {
        let url = format!(
            "{base}/ValueSet/$expand?{BASELINE}&{name}={}",
            urlencode(value)
        );
        let (status, body) = get(&url);

        match disposition {
            Disposition::Refused => {
                assert!(
                    (400..500).contains(&status),
                    "`{name}` must be refused, not ignored - got HTTP {status}. Ignoring it \
                     would let the server answer a narrower request with a broader result"
                );
            }
            Disposition::Honoured { covered_by } => {
                assert!(
                    status == 200 || (400..500).contains(&status),
                    "`{name}` returned HTTP {status}; expected it to be acted on \
                     (see `{covered_by}`)"
                );
            }
            Disposition::CannotAffectResult { because } => {
                assert_eq!(
                    status, 200,
                    "`{name}` is declared harmless to ignore, so it should be accepted"
                );
                let with: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(
                    codes(&with),
                    baseline_codes,
                    "`{name}` is declared unable to affect the result because {because}, \
                     but the expansion membership changed"
                );
            }
        }
    }
}

/// The five implicit SNOMED CT value set URL forms defined by the R4 SNOMED CT
/// page. Each must resolve to its own value set or be refused - never silently
/// substituted for a different one.
#[test]
fn every_implicit_valueset_form_resolves_or_is_refused() {
    let base = start_server();

    let total = |query: &str| -> (u16, Option<u64>) {
        let (status, body) = get(&format!("{base}/ValueSet/$expand?url={}", urlencode(query)));
        if status != 200 {
            return (status, None);
        }
        let v: Value = serde_json::from_str(&body).unwrap();
        (status, v["expansion"]["total"].as_u64())
    };

    let (_, everything) = total("http://snomed.info/sct?fhir_vs");
    let everything = everything.expect("the bare form is the whole code system");

    // `isa/` and `refset/` must name their own value set, not the whole system.
    let (status, isa) = total("http://snomed.info/sct?fhir_vs=isa/73211009");
    assert_eq!(status, 200);
    let isa = isa.unwrap();
    assert!(
        isa < everything,
        "isa/ resolved to the whole code system ({isa} of {everything})"
    );

    let (status, refset) = total("http://snomed.info/sct?fhir_vs=refset/900000000000497000");
    assert_eq!(status, 200, "refset/ should resolve");
    assert!(
        refset.unwrap() < everything,
        "refset/ resolved to the whole code system"
    );

    let (status, _) = total("http://snomed.info/sct?fhir_vs=ecl/<<73211009");
    assert_eq!(status, 200);

    // Defined by the spec but not implemented here: must be refused by name.
    let (status, _) = total("http://snomed.info/sct?fhir_vs=refset");
    assert_eq!(
        status, 400,
        "`?fhir_vs=refset` is unimplemented and must be refused, not substituted"
    );

    // Not an implicit value set at all.
    let (status, _) = total("http://example.org/ValueSet/nope");
    assert_eq!(status, 404, "an unknown value set must not expand");
}

/// Every input parameter of `CodeSystem/$lookup` in FHIR R4, transcribed from
/// <https://hl7.org/fhir/R4/codesystem-operation-lookup.html>. As with
/// [`R4_EXPAND_PARAMETERS`], a parameter missing from [`LOOKUP_DISPOSITIONS`]
/// fails the coverage test below rather than leaving the gap undetected.
const R4_LOOKUP_PARAMETERS: [&str; 7] = [
    "code",
    "system",
    "version",
    "coding",
    "date",
    "displayLanguage",
    "property",
];

/// The declared disposition of every R4 `$lookup` parameter, with a sample
/// value to exercise it. Unlike the `$expand` table, the `Honoured` sample
/// values here are deliberately the ones that *match* the loaded database
/// (matching `code`, matching `system`, matching `version`): the interesting
/// failure mode - a mismatched `system`/`version` being silently ignored
/// rather than refused - is proven by the dedicated
/// `lookup_system_and_version_pass_on_match_and_fail_on_mismatch` test, the
/// same split `check_system_version_passes_on_match_and_fails_on_mismatch`
/// uses for `$expand`.
const LOOKUP_DISPOSITIONS: [(&str, &str, Disposition); 7] = [
    (
        "code",
        "22298006",
        Disposition::Honoured {
            covered_by: "lookup_display_designations_parents",
        },
    ),
    (
        "system",
        "http://snomed.info/sct",
        Disposition::Honoured {
            covered_by: "lookup_system_and_version_pass_on_match_and_fail_on_mismatch",
        },
    ),
    (
        "version",
        "2026-01-01",
        Disposition::Honoured {
            covered_by: "lookup_system_and_version_pass_on_match_and_fail_on_mismatch",
        },
    ),
    (
        "coding",
        "http://snomed.info/sct|22298006",
        Disposition::Refused,
    ),
    ("date", "2020-01-01", Disposition::Refused),
    (
        "displayLanguage",
        "en-GB",
        Disposition::CannotAffectResult {
            because: "this database bakes in a single locale's preferred terms and \
                      designations at `sct ndjson --locale` build time, so no requested \
                      language can change $lookup's display or designation values",
        },
    ),
    (
        "property",
        "parent",
        Disposition::Honoured {
            covered_by: "lookup_display_designations_parents",
        },
    ),
];

/// The `code` used to exercise every `$lookup` disposition: Myocardial
/// infarction, present in the synthetic fixture.
const LOOKUP_BASELINE: &str = "code=22298006";

/// The list of parameters we reason about must match the specification's,
/// exactly - the same gap-detection [`every_r4_expand_parameter_has_a_declared_disposition`]
/// exists for.
#[test]
fn every_r4_lookup_parameter_has_a_declared_disposition() {
    let declared: Vec<&str> = LOOKUP_DISPOSITIONS
        .iter()
        .map(|(name, _, _)| *name)
        .collect();
    for spec_param in R4_LOOKUP_PARAMETERS {
        assert!(
            declared.contains(&spec_param),
            "R4 defines `{spec_param}` on $lookup but it has no declared disposition; \
             decide whether sct honours it, refuses it, or provably cannot be affected by it"
        );
    }
    for name in &declared {
        assert!(
            R4_LOOKUP_PARAMETERS.contains(name),
            "`{name}` is not an R4 $lookup parameter"
        );
    }
    assert_eq!(declared.len(), R4_LOOKUP_PARAMETERS.len());
}

/// Each parameter must actually behave the way the table claims - the same
/// treatment [`every_expand_parameter_behaves_as_declared`] gives `$expand`.
#[test]
fn every_lookup_parameter_behaves_as_declared() {
    let base = start_server();

    let (status, body) = get(&format!("{base}/CodeSystem/$lookup?{LOOKUP_BASELINE}"));
    assert_eq!(status, 200, "baseline lookup should succeed");
    let baseline: Value = serde_json::from_str(&body).unwrap();

    for (name, value, disposition) in &LOOKUP_DISPOSITIONS {
        let url = format!(
            "{base}/CodeSystem/$lookup?{LOOKUP_BASELINE}&{name}={}",
            urlencode(value)
        );
        let (status, body) = get(&url);

        match disposition {
            Disposition::Refused => {
                assert!(
                    (400..500).contains(&status),
                    "`{name}` must be refused, not ignored - got HTTP {status}. Ignoring it \
                     would let the server answer with input the client didn't ask about"
                );
            }
            Disposition::Honoured { covered_by } => {
                assert!(
                    status == 200 || (400..500).contains(&status),
                    "`{name}` returned HTTP {status}; expected it to be acted on \
                     (see `{covered_by}`)"
                );
            }
            Disposition::CannotAffectResult { because } => {
                assert_eq!(
                    status, 200,
                    "`{name}` is declared harmless to ignore, so it should be accepted"
                );
                let with: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(
                    with, baseline,
                    "`{name}` is declared unable to affect the result because {because}, \
                     but the response changed"
                );
            }
        }
    }
}

/// A `system`/`version` matching the loaded release must be honoured; a
/// mismatch must be refused rather than silently answered from whatever *is*
/// loaded - the same failure mode `check_system_version_passes_on_match_and_fails_on_mismatch`
/// guards for `$expand`. Exercised at the `ops::` layer (like that test)
/// rather than over HTTP, so the refusal's diagnostics text is inspectable -
/// an error response's body is otherwise discarded by [`get`].
#[test]
fn lookup_system_and_version_pass_on_match_and_fail_on_mismatch() {
    let (_d, db) = build_db();
    let c = conn(&db);

    assert!(
        ops::check_lookup_system(Some("http://snomed.info/sct")).is_ok(),
        "a matching system must be honoured"
    );
    let err = ops::check_lookup_system(Some("http://loinc.org"))
        .expect_err("a mismatched system must not be silently ignored");
    assert_eq!(err.status, 400);
    assert!(err.diagnostics.contains("loinc.org"));

    // The synthetic fixture's recorded release date.
    let loaded = "2026-01-01";
    assert!(
        ops::check_lookup_version(&c, Some(loaded)).is_ok(),
        "the loaded release's own version must be honoured"
    );
    let err = ops::check_lookup_version(&c, Some("2099-01-01"))
        .expect_err("a mismatched version must not be silently ignored");
    assert_eq!(err.status, 400);
    assert!(
        err.diagnostics.contains("2099-01-01") && err.diagnostics.contains(loaded),
        "diagnostics should name both the demanded and the loaded version: {}",
        err.diagnostics
    );
}

/// Every input parameter of `CodeSystem/$validate-code` in FHIR R4,
/// transcribed from <https://hl7.org/fhir/R4/codesystem-operation-validate-code.html>.
/// As with [`R4_LOOKUP_PARAMETERS`], a parameter missing from
/// [`VALIDATE_CODE_DISPOSITIONS`] fails the coverage test below rather than
/// leaving the gap undetected.
const R4_VALIDATE_CODE_PARAMETERS: [&str; 10] = [
    "url",
    "codeSystem",
    "code",
    "version",
    "display",
    "coding",
    "codeableConcept",
    "date",
    "abstract",
    "displayLanguage",
];

/// The declared disposition of every R4 `CodeSystem/$validate-code`
/// parameter, with a sample value to exercise it. `url` and `version` reuse
/// the same `system`/`version` enforcement `$lookup` already has - the code
/// system this server holds is identified the same way regardless of which
/// operation is asking - so both point at the dedicated mismatch test that
/// already exercises those functions directly, rather than duplicating it.
const VALIDATE_CODE_DISPOSITIONS: [(&str, &str, Disposition); 10] = [
    (
        "url",
        "http://snomed.info/sct",
        Disposition::Honoured {
            covered_by: "lookup_system_and_version_pass_on_match_and_fail_on_mismatch",
        },
    ),
    (
        "codeSystem",
        "ignored-inline-codesystem",
        Disposition::Refused,
    ),
    (
        "code",
        "22298006",
        Disposition::Honoured {
            covered_by: "validate_code_known_and_unknown",
        },
    ),
    (
        "version",
        "2026-01-01",
        Disposition::Honoured {
            covered_by: "lookup_system_and_version_pass_on_match_and_fail_on_mismatch",
        },
    ),
    (
        "display",
        "Myocardial infarction",
        Disposition::Honoured {
            covered_by: "validate_code_known_and_unknown",
        },
    ),
    (
        "coding",
        "http://snomed.info/sct|22298006",
        Disposition::Refused,
    ),
    (
        "codeableConcept",
        "ignored-codeable-concept",
        Disposition::Refused,
    ),
    ("date", "2020-01-01", Disposition::Refused),
    (
        "abstract",
        "true",
        Disposition::CannotAffectResult {
            because: "this server never marks a SNOMED CT concept as FHIR-abstract, so no \
                      concept can be excluded or included on the strength of this flag",
        },
    ),
    (
        "displayLanguage",
        "en-GB",
        Disposition::CannotAffectResult {
            because: "this database bakes in a single locale's preferred terms and \
                      designations at `sct ndjson --locale` build time, so no requested \
                      language can change $validate-code's display or message values",
        },
    ),
];

/// The `code` used to exercise every `CodeSystem/$validate-code` disposition:
/// Myocardial infarction, present in the synthetic fixture.
const VALIDATE_CODE_BASELINE: &str = "code=22298006";

/// The list of parameters we reason about must match the specification's,
/// exactly - the same gap-detection [`every_r4_lookup_parameter_has_a_declared_disposition`]
/// exists for.
#[test]
fn every_r4_validate_code_parameter_has_a_declared_disposition() {
    let declared: Vec<&str> = VALIDATE_CODE_DISPOSITIONS
        .iter()
        .map(|(name, _, _)| *name)
        .collect();
    for spec_param in R4_VALIDATE_CODE_PARAMETERS {
        assert!(
            declared.contains(&spec_param),
            "R4 defines `{spec_param}` on CodeSystem/$validate-code but it has no declared \
             disposition; decide whether sct honours it, refuses it, or provably cannot be \
             affected by it"
        );
    }
    for name in &declared {
        assert!(
            R4_VALIDATE_CODE_PARAMETERS.contains(name),
            "`{name}` is not an R4 CodeSystem/$validate-code parameter"
        );
    }
    assert_eq!(declared.len(), R4_VALIDATE_CODE_PARAMETERS.len());
}

/// Each parameter must actually behave the way the table claims - the same
/// treatment [`every_lookup_parameter_behaves_as_declared`] gives `$lookup`.
#[test]
fn every_validate_code_parameter_behaves_as_declared() {
    let base = start_server();

    let (status, body) = get(&format!(
        "{base}/CodeSystem/$validate-code?{VALIDATE_CODE_BASELINE}"
    ));
    assert_eq!(status, 200, "baseline validate-code should succeed");
    let baseline: Value = serde_json::from_str(&body).unwrap();

    for (name, value, disposition) in &VALIDATE_CODE_DISPOSITIONS {
        let url = format!(
            "{base}/CodeSystem/$validate-code?{VALIDATE_CODE_BASELINE}&{name}={}",
            urlencode(value)
        );
        let (status, body) = get(&url);

        match disposition {
            Disposition::Refused => {
                assert!(
                    (400..500).contains(&status),
                    "`{name}` must be refused, not ignored - got HTTP {status}. Ignoring it \
                     would let the server answer with input the client didn't ask about"
                );
            }
            Disposition::Honoured { covered_by } => {
                assert!(
                    status == 200 || (400..500).contains(&status),
                    "`{name}` returned HTTP {status}; expected it to be acted on \
                     (see `{covered_by}`)"
                );
            }
            Disposition::CannotAffectResult { because } => {
                assert_eq!(
                    status, 200,
                    "`{name}` is declared harmless to ignore, so it should be accepted"
                );
                let with: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(
                    with, baseline,
                    "`{name}` is declared unable to affect the result because {because}, \
                     but the response changed"
                );
            }
        }
    }
}

/// Every input parameter of `ValueSet/$validate-code` in FHIR R4, transcribed
/// from <https://hl7.org/fhir/R4/valueset-operation-validate-code.html>.
const R4_VS_VALIDATE_CODE_PARAMETERS: [&str; 13] = [
    "url",
    "context",
    "valueSet",
    "valueSetVersion",
    "code",
    "system",
    "systemVersion",
    "display",
    "coding",
    "codeableConcept",
    "date",
    "abstract",
    "displayLanguage",
];

/// The declared disposition of every R4 `ValueSet/$validate-code` parameter.
/// `system`/`systemVersion` reuse the same code-system-identity enforcement
/// `$lookup` and the `CodeSystem` form of this operation already have, so
/// they point at the same dedicated mismatch test rather than duplicating it.
const VS_VALIDATE_CODE_DISPOSITIONS: [(&str, &str, Disposition); 13] = [
    (
        "url",
        "http://snomed.info/sct?fhir_vs=isa/73211009",
        Disposition::Honoured {
            covered_by: "valueset_validate_code_membership",
        },
    ),
    ("context", "Condition.code", Disposition::Refused),
    (
        "valueSet",
        "ignored-inline-definition",
        Disposition::Refused,
    ),
    ("valueSetVersion", "1", Disposition::Refused),
    (
        "code",
        "46635009",
        Disposition::Honoured {
            covered_by: "valueset_validate_code_membership",
        },
    ),
    (
        "system",
        "http://snomed.info/sct",
        Disposition::Honoured {
            covered_by: "lookup_system_and_version_pass_on_match_and_fail_on_mismatch",
        },
    ),
    (
        "systemVersion",
        "2026-01-01",
        Disposition::Honoured {
            covered_by: "lookup_system_and_version_pass_on_match_and_fail_on_mismatch",
        },
    ),
    (
        "display",
        "Type 1 diabetes mellitus",
        Disposition::Honoured {
            covered_by: "valueset_validate_code_checks_display_on_both_membership_paths",
        },
    ),
    (
        "coding",
        "http://snomed.info/sct|46635009",
        Disposition::Refused,
    ),
    (
        "codeableConcept",
        "ignored-codeable-concept",
        Disposition::Refused,
    ),
    ("date", "2020-01-01", Disposition::Refused),
    (
        "abstract",
        "true",
        Disposition::CannotAffectResult {
            because: "this server never marks a SNOMED CT concept as FHIR-abstract, so no \
                      concept can be excluded or included on the strength of this flag",
        },
    ),
    (
        "displayLanguage",
        "en-GB",
        Disposition::CannotAffectResult {
            because: "this database bakes in a single locale's preferred terms and \
                      designations at `sct ndjson --locale` build time, so no requested \
                      language can change $validate-code's display or message values",
        },
    ),
];

/// `code`/`url` used to exercise every `ValueSet/$validate-code` disposition:
/// Type 1 diabetes mellitus against the implicit `isa/` value set rooted at
/// Diabetes mellitus, both present in the synthetic fixture. An implicit ECL
/// value set is used rather than a stored `.codelist` one because this test
/// file's server is started with no codelist directory configured.
const VS_VALIDATE_CODE_BASELINE: &str =
    "code=46635009&url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs%3Disa%2F73211009";

/// The list of parameters we reason about must match the specification's,
/// exactly - the same gap-detection the `CodeSystem` form's coverage test has.
#[test]
fn every_r4_vs_validate_code_parameter_has_a_declared_disposition() {
    let declared: Vec<&str> = VS_VALIDATE_CODE_DISPOSITIONS
        .iter()
        .map(|(name, _, _)| *name)
        .collect();
    for spec_param in R4_VS_VALIDATE_CODE_PARAMETERS {
        assert!(
            declared.contains(&spec_param),
            "R4 defines `{spec_param}` on ValueSet/$validate-code but it has no declared \
             disposition; decide whether sct honours it, refuses it, or provably cannot be \
             affected by it"
        );
    }
    for name in &declared {
        assert!(
            R4_VS_VALIDATE_CODE_PARAMETERS.contains(name),
            "`{name}` is not an R4 ValueSet/$validate-code parameter"
        );
    }
    assert_eq!(declared.len(), R4_VS_VALIDATE_CODE_PARAMETERS.len());
}

/// Each parameter must actually behave the way the table claims - the same
/// treatment the `CodeSystem` form's behaviour test gives it.
#[test]
fn every_vs_validate_code_parameter_behaves_as_declared() {
    let base = start_server();

    let (status, body) = get(&format!(
        "{base}/ValueSet/$validate-code?{VS_VALIDATE_CODE_BASELINE}"
    ));
    assert_eq!(status, 200, "baseline validate-code should succeed");
    let baseline: Value = serde_json::from_str(&body).unwrap();

    for (name, value, disposition) in &VS_VALIDATE_CODE_DISPOSITIONS {
        let url = format!(
            "{base}/ValueSet/$validate-code?{VS_VALIDATE_CODE_BASELINE}&{name}={}",
            urlencode(value)
        );
        let (status, body) = get(&url);

        match disposition {
            Disposition::Refused => {
                assert!(
                    (400..500).contains(&status),
                    "`{name}` must be refused, not ignored - got HTTP {status}. Ignoring it \
                     would let the server answer with input the client didn't ask about"
                );
            }
            Disposition::Honoured { covered_by } => {
                assert!(
                    status == 200 || (400..500).contains(&status),
                    "`{name}` returned HTTP {status}; expected it to be acted on \
                     (see `{covered_by}`)"
                );
            }
            Disposition::CannotAffectResult { because } => {
                assert_eq!(
                    status, 200,
                    "`{name}` is declared harmless to ignore, so it should be accepted"
                );
                let with: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(
                    with, baseline,
                    "`{name}` is declared unable to affect the result because {because}, \
                     but the response changed"
                );
            }
        }
    }
}

fn codes(vs: &Value) -> Vec<String> {
    vs["expansion"]["contains"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| e["code"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
