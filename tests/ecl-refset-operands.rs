// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! ECL §6.1 memberOf selects active membership records, independently of
//! whether the reference set's metadata concept is loaded.

#[cfg(feature = "serve")]
use rusqlite::{Connection, OpenFlags};
use sct_rs::commands::{ndjson, sqlite};
use sct_rs::sdk::Snomed;
use std::path::{Path, PathBuf};

const REFSET: &str = "991381000000107";

fn build(include_inactive: bool, tct: bool, refset_state: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z");
    let rf2 = dir.path().join("rf2");
    for entry in walkdir::WalkDir::new(&source) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() {
            let dest = rf2.join(entry.path().strip_prefix(&source).unwrap());
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
    let concepts = rf2.join("Snapshot/Terminology/sct2_Concept_Snapshot_SYN_20260101.txt");
    let text = std::fs::read_to_string(&concepts).unwrap();
    let rows: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let mut fields: Vec<&str> = line.split('\t').collect();
            if fields[0] == REFSET {
                if refset_state == "absent" {
                    return None;
                }
                fields[2] = refset_state;
            }
            Some(fields.join("\t"))
        })
        .collect();
    std::fs::write(concepts, format!("{}\n", rows.join("\n"))).unwrap();

    // Add an active membership for an inactive concept, and an inactive
    // membership for known active MI. These must not be conflated.
    let members = rf2.join("Snapshot/Refset/Content/der2_Refset_SimpleSnapshot_SYN_20260101.txt");
    let mut text = std::fs::read_to_string(&members).unwrap();
    text.push_str(&format!(
        "r3\t20260101\t1\t900000000000207008\t{REFSET}\t9468002\n"
    ));
    text.push_str(&format!(
        "r4\t20260101\t0\t900000000000207008\t{REFSET}\t22298006\n"
    ));
    std::fs::write(members, text).unwrap();
    let input = dir.path().join("synthetic.ndjson");
    let db = dir.path().join("synthetic.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![rf2],
        locale: "en-GB".into(),
        output: Some(input.clone()),
        include_inactive,
        refsets: ndjson::RefsetMode::Simple,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input,
        output: Some(db.clone()),
        transitive_closure: tct,
        include_self: false,
    })
    .unwrap();
    (dir, db)
}

#[test]
fn membership_does_not_require_a_loaded_refset_concept() {
    // Source: https://docs.snomed.org/snomed-ct-specifications/snomed-ct-expression-constraint-language/behaviour-specification-with-examples/6.1-simple-expression-constraints#member-of
    for state in ["1", "0", "absent"] {
        for include_inactive in [false, true] {
            for tct in [false, true] {
                let (_dir, db) = build(include_inactive, tct, state);
                let sdk = Snomed::open(&db).unwrap();
                assert_eq!(
                    sdk.concept("46635009").unwrap().unwrap().preferred_term,
                    "Type 1 diabetes mellitus"
                );
                assert_eq!(
                    sdk.concept("22298006").unwrap().unwrap().preferred_term,
                    "Myocardial infarction"
                );
                assert_eq!(
                    sdk.concept(REFSET).unwrap().is_some(),
                    state == "1" || (state == "0" && include_inactive)
                );
                let mut expected = vec!["44054006", "46635009"];
                if include_inactive {
                    expected.push("9468002");
                }
                let expressions = [
                    format!("^{REFSET}"),
                    format!("^{REFSET} OR ^{REFSET}"),
                    format!("^({REFSET} AND {REFSET})"),
                    format!("^({REFSET} OR 99999999)"),
                    format!("^({REFSET} MINUS 99999999)"),
                    format!("^({REFSET} AND *)"),
                    "^*".into(),
                ];
                for expression in expressions {
                    let context = format!("{expression}: state={state}, include_inactive={include_inactive}, tct={tct}");
                    let mut actual = sdk.expand(&expression).unwrap();
                    actual.sort();
                    assert_eq!(actual, expected, "SDK {context}");
                    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sct"))
                        .args(["ecl", "expand", &expression, "--db"])
                        .arg(&db)
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "CLI {context}: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    let stdout = String::from_utf8(output.stdout).unwrap();
                    let mut actual: Vec<&str> = stdout.lines().collect();
                    actual.sort();
                    assert_eq!(actual, expected, "CLI {context}");
                    #[cfg(feature = "serve")]
                    for active_only in [false, true] {
                        let mut conn =
                            Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
                                .unwrap();
                        let response = sct_rs::commands::serve::ops::expand(
                            &mut conn,
                            Some(&expression),
                            None,
                            100,
                            0,
                            false,
                            active_only,
                            None,
                            None,
                        )
                        .unwrap();
                        let mut actual: Vec<&str> = response["expansion"]["contains"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|item| item["code"].as_str().unwrap())
                            .collect();
                        actual.sort();
                        let expected: Vec<&str> = expected
                            .iter()
                            .copied()
                            .filter(|id| !active_only || *id != "9468002")
                            .collect();
                        assert_eq!(actual, expected, "FHIR active_only={active_only} {context}");
                        assert_eq!(response["expansion"]["total"], expected.len());
                    }
                }
                for expression in [
                    "^99999999".to_string(),
                    format!("^({REFSET} MINUS {REFSET})"),
                    "99999999".into(),
                    "<<99999999".into(),
                ] {
                    assert!(sdk.expand(&expression).unwrap().is_empty(), "{expression}");
                }
            }
        }
    }
}
