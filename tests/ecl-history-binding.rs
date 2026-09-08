// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Issue #132: history binds to its subexpression, not an enclosing refinement.

use rusqlite::{Connection, OpenFlags};
use sct_rs::commands::{ndjson, sqlite};
use sct_rs::sdk::Snomed;
use std::path::PathBuf;
use std::process::Command;

const MI: &[&str] = &["22298006"];
const WITH_HISTORY: &[&str] = &["22298006", "9468002"];
#[rustfmt::skip]
const CASES: &[(&str, &[&str])] = &[
    ("<<404684003 : 363698007 = 74281007", MI),
    ("<<404684003 : 363698007 = 74281007 {{ + HISTORY-MOD }}", MI),
    ("<<404684003 : 363698007 = (74281007) {{ + HISTORY-MOD }}", MI),
    ("<<404684003 : 363698007 = (74281007 {{ + HISTORY-MOD }})", MI),
    ("(<<404684003 : 363698007 = 74281007) {{ + HISTORY-MOD }}", WITH_HISTORY),
    ("<<404684003 {{ + HISTORY-MOD }} : 363698007 = 74281007", MI),
    ("<<404684003 : 363698007 {{ + HISTORY-MOD }} = 74281007", MI),
    ("<<404684003 : 363698007 = 74281007 {{ + HISTORY-MOD }}, 116676008 = 55641003", MI),
    ("<<404684003 : 363698007 = 74281007 {{ + HISTORY-MOD }} AND 116676008 = 55641003", MI),
    ("<<404684003 : { 363698007 = 74281007 {{ + HISTORY-MOD }}, 116676008 = 55641003 }", MI),
    ("(<<404684003 : { 363698007 = 74281007, 116676008 = 55641003 }) {{ + HISTORY-MOD }}", WITH_HISTORY),
    ("<<404684003 : 363698007 = 74281007 {{ + HISTORY (900000000000526001) }}", MI),
    ("(<<404684003 : 363698007 = 74281007) {{ + HISTORY (900000000000526001) }}", WITH_HISTORY),
    ("(<<404684003 : 363698007 = 74281007) {{ + HISTORY (900000000000527005) }}", MI),
];

fn build(tct: bool) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("synthetic.ndjson");
    let db = dir.path().join("synthetic.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")],
        locale: "en-GB".into(),
        output: Some(input.clone()),
        // Keep history sources present so SDK/CLI and FHIR see the same universe.
        include_inactive: true,
        refsets: ndjson::RefsetMode::All,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input,
        output: Some(db.clone()),
        transitive_closure: tct,
        include_self: false,
    })
    .unwrap();

    let sdk = Snomed::open(&db).unwrap();
    assert_eq!(sdk.has_transitive_closure(), tct);
    for (id, name, active) in [
        ("22298006", "Myocardial infarction", true),
        ("74281007", "Myocardium structure", true),
        ("9468002", "Inactive example disorder", false),
    ] {
        let concept = sdk.concept(id).unwrap().expect("fixture concept present");
        assert_eq!(concept.preferred_term, name, "{id}");
        assert_eq!(concept.active, active, "{id}");
    }
    // Independent fixture evidence, not an oracle derived from ECL evaluation.
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    for (attribute, value) in [("363698007", "74281007"), ("116676008", "55641003")] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM concept_relationships
             WHERE source_id = ?1 AND type_id = ?2 AND destination_id = ?3 AND group_num = 1",
                ["22298006", attribute, value],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM concept_history
         WHERE source_id = ?1 AND association = ?2 AND target_id = ?3",
            ["9468002", "replaced_by", "22298006"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    (dir, db)
}

fn check(
    failures: &mut Vec<String>,
    context: &str,
    actual: Result<Vec<String>, String>,
    expected: &[&str],
) {
    match actual {
        Ok(mut ids) => {
            ids.sort();
            if ids != expected {
                failures.push(format!("{context}: expected {expected:?}, got {ids:?}"));
            }
        }
        Err(error) => failures.push(format!("{context}: {error}")),
    }
}

#[test]
fn history_binding_sdk_and_cli() {
    let mut failures = Vec::new();
    for tct in [false, true] {
        let (dir, db) = build(tct);
        let sdk = Snomed::open(&db).unwrap();
        for &(expression, expected) in CASES {
            check(
                &mut failures,
                &format!("SDK tct={tct}: {expression}"),
                sdk.expand(expression).map_err(|e| e.to_string()),
                expected,
            );
            let output = Command::new(env!("CARGO_BIN_EXE_sct"))
                .args(["ecl", "expand", expression, "--format", "json", "--db"])
                .arg(&db)
                .current_dir(dir.path())
                .env("SCT_DATA_HOME", dir.path())
                .output()
                .unwrap();
            let actual = if output.status.success() {
                serde_json::from_slice::<Vec<String>>(&output.stdout).map_err(|e| e.to_string())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).into_owned())
            };
            check(
                &mut failures,
                &format!("CLI tct={tct}: {expression}"),
                actual,
                expected,
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(feature = "serve")]
#[test]
fn history_binding_fhir_preserves_active_only() {
    use sct_rs::commands::serve::ops;

    let mut failures = Vec::new();
    for tct in [false, true] {
        let (_dir, db) = build(tct);
        let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        for &(expression, expected) in CASES {
            for active_only in [false, true] {
                let expected = if active_only { MI } else { expected };
                let context = format!("FHIR tct={tct} active_only={active_only}: {expression}");
                let actual = ops::expand(
                    &conn,
                    Some(expression),
                    None,
                    100,
                    0,
                    false,
                    active_only,
                    None,
                    None,
                )
                .map_err(|e| format!("{e:?}"))
                .map(|value| {
                    if value["expansion"]["total"] != serde_json::json!(expected.len()) {
                        failures.push(format!("{context}: incorrect total: {value}"));
                    }
                    value["expansion"]["contains"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|entry| entry["code"].as_str().unwrap().to_owned())
                        .collect()
                });
                check(&mut failures, &context, actual, expected);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
