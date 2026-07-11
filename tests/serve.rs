// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct serve` FHIR R4 tests over the synthetic RF2 fixture. Exercises the
//! operation logic directly (FHIR semantics) plus one live HTTP round-trip.
//! Gated on `--features serve`.
#![cfg(feature = "serve")]

use rusqlite::Connection;
use sct_rs::commands::ndjson::{self, RefsetMode};
use sct_rs::commands::serve::{ops, serve_listener, valuesets};
use sct_rs::commands::sqlite;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

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
        output: db.clone(),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    (dir, db)
}

/// Build the fixture DB, optionally with the transitive-closure table so the
/// `$expand` fast path exercises its TCT SQL form.
fn build_db_with(tct: bool) -> (tempfile::TempDir, PathBuf) {
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
        output: db.clone(),
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
}

#[test]
fn expand_ecl_filter_and_combined() {
    let (_d, db) = build_db();
    let c = conn(&db);

    let v = ops::expand(&c, Some("<<73211009"), None, 100, 0, false).unwrap();
    assert_eq!(v["resourceType"], "ValueSet");
    assert_eq!(v["expansion"]["total"], 3);
    let mut codes = contains_codes(&v);
    codes.sort();
    assert_eq!(codes, ["44054006", "46635009", "73211009"]);

    let v = ops::expand(&c, None, Some("diabetes"), 100, 0, false).unwrap();
    assert!(contains_codes(&v).contains(&"73211009".to_string()));

    // ECL ∩ text filter: clinical findings under root, filtered to "diabetes".
    let v = ops::expand(&c, Some("<<404684003"), Some("diabetes"), 100, 0, false).unwrap();
    let codes = contains_codes(&v);
    assert!(codes.contains(&"73211009".to_string()));
    assert!(!codes.contains(&"22298006".to_string())); // MI is not a "diabetes" match
}

#[test]
fn expand_pagination() {
    let (_d, db) = build_db();
    let v = ops::expand(&conn(&db), Some("<<73211009"), None, 2, 0, false).unwrap();
    assert_eq!(v["expansion"]["total"], 3); // total reflects the full set
    assert_eq!(contains_codes(&v).len(), 2); // page is capped at count
}

#[test]
fn expand_fast_path_with_tct_matches() {
    // Same hierarchy expansion against a DB that has the transitive-closure
    // table - exercises the TCT branch of the SQL fast path.
    let (_d, db) = build_db_with(true);
    let v = ops::expand(&conn(&db), Some("<<73211009"), None, 100, 0, false).unwrap();
    assert_eq!(v["expansion"]["total"], 3);
    let mut codes = contains_codes(&v);
    codes.sort();
    assert_eq!(codes, ["44054006", "46635009", "73211009"]);
}

#[test]
fn expand_refset_member_fast_path() {
    let (_d, db) = build_db();
    let v = ops::expand(&conn(&db), Some("^991381000000107"), None, 100, 0, false).unwrap();
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
    )
    .unwrap();
    assert_eq!(contains_codes(&v), ["22298006"]);
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

#[test]
fn valueset_expand_members_reconciles_display() {
    let (_d, db) = build_db();
    let dir = codelist_dir();
    let reg = valuesets::load_registry(dir.path(), "http://x");
    let members = reg.get("dm-plus").unwrap().members.clone();

    let v = ops::expand_members(&conn(&db), &members, 100, 0, false).unwrap();
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
    let p = ops::expand_members(&conn(&db), &members, 2, 0, false).unwrap();
    assert_eq!(p["expansion"]["total"], 3);
    assert_eq!(contains_codes(&p).len(), 2);
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

    let yes =
        ops::validate_code_in_set(&c, &set, "46635009", "http://x/ValueSet/diabetes").unwrap();
    assert_eq!(param_bool(&yes, "result"), Some(true));
    let no = ops::validate_code_in_set(&c, &set, "22298006", "http://x/ValueSet/diabetes").unwrap();
    assert_eq!(param_bool(&no, "result"), Some(false));
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
        {"request":{"method":"GET","url":"CodeSystem/$lookup?system=http://snomed.info/sct&code=99999999"}}
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
    assert_eq!(br["entry"].as_array().unwrap().len(), 2);
    assert_eq!(br["entry"][0]["response"]["status"], "200");
    assert_eq!(br["entry"][0]["resource"]["resourceType"], "Parameters");
    assert_eq!(br["entry"][1]["response"]["status"], "404"); // unknown code
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
