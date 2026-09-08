// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn sct(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sct").unwrap();
    cmd.current_dir(dir).env("SCT_DATA_HOME", dir);
    cmd
}

fn build_db(dir: &Path) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z");
    sct(dir)
        .args(["ndjson", "--rf2"])
        .arg(fixture)
        .args(["--output", "fixture.ndjson"])
        .assert()
        .success();
    sct(dir)
        .args([
            "sqlite",
            "--ndjson",
            "fixture.ndjson",
            "--output",
            "fixture.db",
        ])
        .assert()
        .success();
    dir.join("fixture.db")
}

#[test]
fn compression_annotations_do_not_add_members() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(dir.path());
    for (args, stdin, expected) in [
        (
            vec![],
            "22298006 |Myocardial infarction mentioning 46635009 and 73211009|",
            vec!["22298006"],
        ),
        (vec!["-"], "22298006|numeric 46635009|\n", vec!["22298006"]),
        (vec![], "22298006\t46635009\n", vec!["22298006", "46635009"]),
        (
            vec!["22298006 |Myocardial infarction 73211009|", "46635009"],
            "",
            vec!["22298006", "46635009"],
        ),
        (
            vec!["22298006", "-"],
            "46635009 |Type 1 diabetes mellitus|",
            vec!["22298006", "46635009"],
        ),
        (
            vec![],
            "22298006 |line one\n46635009| 73211009",
            vec!["22298006", "73211009"],
        ),
    ] {
        let output = sct(dir.path())
            .args(["ecl", "compress", "--db"])
            .arg(&db)
            .args(args)
            .write_stdin(stdin)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let expr = String::from_utf8(output).unwrap();
        let expanded = sct(dir.path())
            .args(["ecl", "expand", expr.trim(), "--db"])
            .arg(&db)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let expanded = String::from_utf8(expanded).unwrap();
        let mut actual: Vec<_> = expanded.lines().collect();
        actual.sort_unstable();
        assert_eq!(actual, expected, "input {stdin:?}");
    }
}

#[test]
fn compression_rejects_malformed_input_on_stdin_and_in_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(dir.path());
    for malformed in [
        "22298006junk",
        "22298006,46635009",
        "22298006 OR 46635009",
        "22298006 |unterminated 46635009",
        "22298006 |term|46635009",
        "22298006 |term| trailing",
        "|22298006|",
        "+22298006",
    ] {
        for positional in [false, true] {
            let mut cmd = sct(dir.path());
            cmd.args(["ecl", "compress", "--db"]).arg(&db);
            if positional {
                cmd.arg(malformed);
            } else {
                cmd.write_stdin(malformed);
            }
            cmd.assert().failure().stdout("");
        }
    }
}

#[cfg(feature = "serve")]
#[test]
fn http_implicit_identifier_slots_are_not_expressions() {
    use serde_json::Value;
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(dir.path());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        sct_rs::commands::serve::serve_listener(db, "/", None, None, 2, listener).unwrap();
    });
    let base = format!("http://{addr}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if ureq::get(&format!("{base}isa/22298006")).call().is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "server startup timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    for form in ["isa", "refset"] {
        for suffix in [
            "",
            "22298006%20OR%2046635009",
            "22298006junk",
            "22298006%20%7Cterm%7C",
        ] {
            let err = ureq::get(&format!("{base}{form}/{suffix}"))
                .call()
                .unwrap_err();
            assert!(
                matches!(err, ureq::Error::StatusCode(400)),
                "{form}/{suffix}: {err:?}"
            );
        }
    }
    for (suffix, expected) in [
        ("isa/22298006", vec!["22298006"]),
        ("refset/991381000000107", vec!["44054006", "46635009"]),
        ("ecl/22298006%20OR%2046635009", vec!["22298006", "46635009"]),
    ] {
        let body = ureq::get(&format!("{base}{suffix}"))
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        let mut codes: Vec<_> = value["expansion"]["contains"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["code"].as_str().unwrap())
            .collect();
        codes.sort_unstable();
        assert_eq!(codes, expected);
    }
}
