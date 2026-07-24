// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! CLI contract tests (R18): run the real `sct` binary via `assert_cmd` against
//! tiny fixtures and assert on exit codes, generated files, and stdout/stderr.
//! These cover contract-level behaviour - argument parsing, default output file
//! naming, and command exit codes - that the in-process unit and end-to-end
//! tests do not exercise across the actual binary boundary.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn sct() -> Command {
    Command::cargo_bin("sct").expect("sct binary builds")
}

/// The committed synthetic RF2 Snapshot fixture (licence-free, generated).
fn rf2_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

/// Build a SNOMED CT SQLite database from the fixture (ndjson -> sqlite), for
/// commands like `codelist validate` that resolve a database up front.
fn build_db(dir: &std::path::Path) -> PathBuf {
    let ndjson = dir.join("fixture.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--locale", "en-GB", "--output"])
        .arg(&ndjson)
        .assert()
        .success();
    let db = dir.join("fixture.db");
    sct()
        .args(["sqlite", "--ndjson"])
        .arg(&ndjson)
        .arg("--output")
        .arg(&db)
        .assert()
        .success();
    db
}

// --- clap-level contracts ---------------------------------------------------

#[test]
fn version_flag_prints_version() {
    sct()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_flag_prints_usage() {
    sct()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage:"));
}

#[test]
fn unknown_subcommand_is_arg_error() {
    sct()
        .arg("definitely-not-a-command")
        .assert()
        .failure()
        .code(2)
        .stderr(
            predicate::str::contains("unrecognized").or(predicate::str::contains("unexpected")),
        );
}

#[test]
fn missing_required_argument_is_arg_error() {
    // `sct info` requires a <FILE> positional; clap rejects with exit code 2.
    sct().arg("info").assert().failure().code(2);
}

// --- pipeline + file-naming contracts (over the RF2 fixture) ----------------

#[test]
fn ndjson_sqlite_info_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("out.ndjson");
    let db = tmp.path().join("out.db");

    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--locale", "en-GB", "--output"])
        .arg(&ndjson)
        .assert()
        .success();
    assert!(
        ndjson.metadata().unwrap().len() > 0,
        "ndjson output should be non-empty"
    );

    sct()
        .args(["sqlite", "--ndjson"])
        .arg(&ndjson)
        .arg("--output")
        .arg(&db)
        .assert()
        .success();
    assert!(db.exists(), "sqlite database should be created");

    sct()
        .arg("info")
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::contains("Concepts"));
}

#[test]
fn sqlite_default_output_name_is_snomed_db() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("out.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .arg("--output")
        .arg(&ndjson)
        .assert()
        .success();

    // No --output: `sct sqlite` defaults to `snomed.db` in the working directory.
    sct()
        .current_dir(tmp.path())
        .args(["sqlite", "--ndjson"])
        .arg(&ndjson)
        .assert()
        .success();
    assert!(
        tmp.path().join("snomed.db").exists(),
        "default snomed.db should be created in CWD"
    );
}

#[test]
fn ndjson_default_output_is_slug_ndjson() {
    let tmp = tempfile::tempdir().unwrap();
    // No --output: `sct ndjson` writes `<release-slug>.ndjson` into the CWD.
    sct()
        .current_dir(tmp.path())
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .assert()
        .success();
    let ndjson_files: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "ndjson"))
        .collect();
    assert_eq!(
        ndjson_files.len(),
        1,
        "exactly one .ndjson should be produced, found: {ndjson_files:?}"
    );
}

// --- codelist exit-code contracts -------------------------------------------

#[test]
fn codelist_new_then_validate_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("test.codelist");

    sct()
        .args(["codelist", "new"])
        .arg(&file)
        .args(["--title", "Test list", "--no-edit"])
        .assert()
        .success();
    assert!(file.exists(), "codelist should be scaffolded");

    // `codelist validate` resolves a SNOMED CT database up front, so build one
    // from the fixture and pass it explicitly (CI has no ambient database). A
    // fresh draft has no concepts, so it validates cleanly.
    let db = build_db(tmp.path());
    sct()
        .args(["codelist", "validate"])
        .arg(&file)
        .arg("--db")
        .arg(&db)
        .assert()
        .success();
}

#[test]
fn codelist_validate_missing_file_fails() {
    let tmp = tempfile::tempdir().unwrap();
    sct()
        .args(["codelist", "validate"])
        .arg(tmp.path().join("nope.codelist"))
        .assert()
        .failure();
}

// --- R4: single-lookup miss exit-code contracts ------------------------------
//
// A single-item lookup that finds nothing is an error, not an empty success:
// it writes a hint to stderr and exits non-zero, unlike a search command's
// zero-results case (which stays exit 0 with machine-clean stdout).

#[test]
fn lookup_missing_sctid_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    sct()
        .args(["lookup", "999999999"])
        .arg("--db")
        .arg(&db)
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn lookup_missing_ctv3_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    sct()
        .args(["lookup", "ZZZZZ"])
        .arg("--db")
        .arg(&db)
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "No SNOMED CT mapping found for CTV3 code",
        ));
}

#[test]
fn refset_info_missing_refset_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    sct()
        .args(["refset", "info", "999999999"])
        .arg("--db")
        .arg(&db)
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn refset_profile_missing_refset_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    sct()
        .args(["refset", "profile", "999999999"])
        .arg("--db")
        .arg(&db)
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("not found"));
}
