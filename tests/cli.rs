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

fn build_ndjson(dir: &std::path::Path) -> PathBuf {
    let ndjson = dir.join("fixture.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--locale", "en-GB", "--output"])
        .arg(&ndjson)
        .assert()
        .success();
    ndjson
}

/// Build a SNOMED CT SQLite database from the fixture (ndjson -> sqlite), for
/// commands like `codelist validate` that resolve a database up front.
fn build_db(dir: &std::path::Path) -> PathBuf {
    let ndjson = build_ndjson(dir);
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

fn build_history_db(dir: &std::path::Path) -> PathBuf {
    let ndjson = dir.join("fixture.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args([
            "--locale",
            "en-GB",
            "--include-inactive",
            "--refsets",
            "all",
            "--output",
        ])
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

fn build_fst(dir: &std::path::Path) -> PathBuf {
    let ndjson = build_ndjson(dir);
    let index = dir.join("fixture.fst");
    sct()
        .args(["fst", "build", "--ndjson"])
        .arg(&ndjson)
        .arg("--output")
        .arg(&index)
        .assert()
        .success();
    index
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
fn history_reports_an_inactive_concepts_reason_and_replacements() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_history_db(dir.path());
    sct()
        .args(["history", "9468002", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::contains("INACTIVE - Duplicate"))
        .stdout(predicate::str::contains(
            "Replaced by: [22298006] Myocardial infarction",
        ))
        .stdout(predicate::str::contains("Same as: [195967001] Asthma"));
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

#[test]
fn ids_conflicts_with_explicit_structured_formats() {
    let cases: &[&[&str]] = &[
        &["lookup", "22298006", "--ids", "--format", "json"],
        &["lexical", "heart", "--ids", "--format", "yaml"],
        &["semantic", "heart", "--ids", "--format", "json"],
        &[
            "refset",
            "members",
            "991381000000107",
            "--ids",
            "--format",
            "json",
        ],
    ];
    for args in cases {
        sct()
            .args(*args)
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains("cannot be used with"));
    }
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
fn info_format_json_emits_structured_output() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = build_ndjson(tmp.path());

    let output = sct()
        .args(["info", "--format", "json"])
        .arg(&ndjson)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "sct info --format json should succeed"
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("stdout should be valid JSON, not the human-readable text layout");
    assert_eq!(value["format"], "ndjson");
    assert!(value["concept_count"].as_u64().unwrap() > 0);
    assert!(value["hierarchies"].is_array());
}

#[test]
fn info_payload_refset_json_reports_verified_family_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("fixture.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--refsets", "all", "--output"])
        .arg(&ndjson)
        .assert()
        .success();
    let sidecar = tmp.path().join("fixture.refsets.ndjson");

    let output = sct()
        .args(["info", "--format", "json"])
        .arg(sidecar)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "sct info should inspect the sidecar"
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["format"], "refset_ndjson");
    assert_eq!(value["record_count"], 15);
    assert_eq!(value["complex_map_count"], 2);
    assert_eq!(value["extended_map_count"], 10);
    assert_eq!(value["attribute_value_count"], 3);
    assert!(value["source_content_fingerprint"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
}

#[test]
fn sqlite_default_output_is_named_after_its_input() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("uk-monolith-42.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .arg("--output")
        .arg(&ndjson)
        .assert()
        .success();

    // No --output: the input's stem carries through to the database name, and
    // the derived name is announced so the next command can consume it.
    sct()
        .current_dir(tmp.path())
        .args(["sqlite", "--ndjson"])
        .arg(&ndjson)
        .assert()
        .success()
        .stderr(predicate::str::contains("Output: uk-monolith-42.db"));
    assert!(
        tmp.path().join("uk-monolith-42.db").exists(),
        "database should be named after the NDJSON input"
    );
    assert!(
        !tmp.path().join("snomed.db").exists(),
        "the old fixed default must no longer be used"
    );
}

#[test]
fn canonical_input_name_still_yields_the_canonical_output_names() {
    // The canonical names are the naming rule applied to a canonical input, not
    // a separate case: snomed.ndjson still produces snomed.db.
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("snomed.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .arg("--output")
        .arg(&ndjson)
        .assert()
        .success();

    sct()
        .current_dir(tmp.path())
        .args(["sqlite", "--ndjson"])
        .arg(&ndjson)
        .assert()
        .success();
    assert!(tmp.path().join("snomed.db").exists());

    sct()
        .current_dir(tmp.path())
        .args(["parquet", "--ndjson"])
        .arg(&ndjson)
        .assert()
        .success();
    assert!(tmp.path().join("snomed.parquet").exists());

    sct()
        .current_dir(tmp.path())
        .args(["fst", "build", "--ndjson"])
        .arg(&ndjson)
        .assert()
        .success();
    assert!(tmp.path().join("snomed.fst").exists());
}

#[test]
fn build_commands_name_every_artefact_after_the_input() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("uk-monolith-42.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .arg("--output")
        .arg(&ndjson)
        .assert()
        .success();

    for (args, expected) in [
        (vec!["parquet"], "uk-monolith-42.parquet"),
        (vec!["markdown"], "uk-monolith-42-concepts"),
    ] {
        let mut cmd = sct();
        cmd.current_dir(tmp.path());
        cmd.args(&args).arg("--ndjson").arg(&ndjson);
        cmd.assert().success();
        assert!(
            tmp.path().join(expected).exists(),
            "{args:?} should have produced {expected}"
        );
    }

    // `fst build` is a subcommand, so it does not fit the loop above.
    sct()
        .current_dir(tmp.path())
        .args(["fst", "build", "--ndjson"])
        .arg(&ndjson)
        .assert()
        .success();
    assert!(tmp.path().join("uk-monolith-42.fst").exists());
}

#[test]
fn stdin_input_falls_back_to_the_canonical_name() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("uk-monolith-42.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .arg("--output")
        .arg(&ndjson)
        .assert()
        .success();

    // Piped input has no name to inherit, so the canonical stem is used.
    sct()
        .current_dir(tmp.path())
        .args(["parquet", "--ndjson", "-"])
        .pipe_stdin(&ndjson)
        .unwrap()
        .assert()
        .success();
    assert!(tmp.path().join("snomed.parquet").exists());
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

#[test]
fn ndjson_stdout_is_a_valid_canonical_artefact() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("stdout.ndjson");
    let db = tmp.path().join("stdout.db");

    let output = sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--locale", "en-GB", "--output", "-"])
        .output()
        .unwrap();
    assert!(output.status.success(), "sct ndjson failed");
    std::fs::write(&ndjson, output.stdout).unwrap();

    // The SQLite importer verifies the provenance content fingerprint while
    // consuming the records, so success proves stdout emitted a complete,
    // canonical artefact rather than a placeholder or partial stream.
    sct()
        .args(["sqlite", "--ndjson"])
        .arg(&ndjson)
        .arg("--output")
        .arg(&db)
        .assert()
        .success();
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

#[test]
fn codelist_ecl_conflicts_with_include_descendants() {
    sct()
        .args([
            "codelist",
            "add",
            "list.codelist",
            "--ecl",
            "<<73211009 MINUS <<46635009",
            "--include-descendants",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn codelist_add_deduplicates_repeated_stdin_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    let file = tmp.path().join("deduplicated.codelist");
    sct()
        .args(["codelist", "new"])
        .arg(&file)
        .args(["--title", "Deduplicated", "--no-edit"])
        .assert()
        .success();

    let mut command = sct();
    command
        .args(["codelist", "add"])
        .arg(&file)
        .args(["-", "--db"])
        .arg(&db)
        .write_stdin("22298006\n22298006 |Myocardial infarction|\n22298006\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Added 1 concept"));

    let contents = std::fs::read_to_string(&file).unwrap();
    assert_eq!(contents.matches("22298006").count(), 1);
    sct()
        .args(["codelist", "validate"])
        .arg(&file)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
}

// --- R4: single-lookup miss exit-code contracts ------------------------------
//
// A single-item lookup that finds nothing is an error, not an empty success:
// it writes a hint to stderr and exits non-zero, unlike a search command's
// zero-results case (which stays exit 0 with machine-clean stdout).

/// `parents` and `attributes` serialise as `ConceptRef` (`{"id", "fsn"}`) with
/// no `term`/`preferred_term` field, so the text renderer must read `fsn` -
/// every parent and attribute line otherwise renders as a bare `?`.
#[test]
fn lookup_text_shows_real_terms_for_parents_and_attributes() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    // 46635009 Type 1 diabetes mellitus has a real, non-root IS-A parent.
    sct()
        .args(["lookup", "46635009", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::contains("[73211009] Diabetes mellitus"))
        .stdout(predicate::str::contains("?").not());

    // 22298006 Myocardial infarction carries both attribute types.
    sct()
        .args(["lookup", "22298006", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "finding_site: [74281007] Myocardium structure",
        ))
        .stdout(predicate::str::contains(
            "associated_morphology: [55641003] Infarct",
        ))
        .stdout(predicate::str::contains("?").not());
}

#[test]
fn lookup_missing_sctid_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    for ids in [false, true] {
        let mut command = sct();
        command.args(["lookup", "999999999"]);
        if ids {
            command.arg("--ids");
        }
        command
            .arg("--db")
            .arg(&db)
            .assert()
            .failure()
            .code(1)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("not found"));
    }
}

#[test]
fn lookup_bad_checksum_gets_a_helpful_note_by_default() {
    // 73211009 is Diabetes mellitus in the fixture; 73211008 is a one-digit
    // mutation that both fails Verhoeff and matches no fixture concept, so
    // this exercises the "not found, and also fails checksum" warning path.
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    sct()
        .args(["lookup", "73211008", "--db"])
        .arg(&db)
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("not found"))
        .stderr(predicate::str::contains("check-digit validation"));
}

#[test]
fn lookup_strict_checksum_config_fails_fast_before_querying_the_db() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    let config = tmp.path().join("strict.toml");
    std::fs::write(&config, "[lookup]\nstrict_sctid_checksum = true\n").unwrap();

    sct()
        .args(["lookup", "73211008", "--db"])
        .arg(&db)
        .env("SCT_CONFIG", &config)
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("check-digit validation"))
        .stderr(predicate::str::contains("not found").not());
}

#[test]
fn lookup_missing_ctv3_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    for ids in [false, true] {
        let mut command = sct();
        command.args(["lookup", "ZZZZZ"]);
        if ids {
            command.arg("--ids");
        }
        command
            .arg("--db")
            .arg(&db)
            .assert()
            .failure()
            .code(1)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(
                "No SNOMED CT mapping found for CTV3 code",
            ));
    }
}

#[test]
fn lookup_ctv3_structured_output_is_one_valid_document() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    let output = sct()
        .args(["lookup", "X200", "--format", "json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["id"], "22298006");
    assert_eq!(value["preferred_term"], "Myocardial infarction");
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
        .stdout(predicate::str::is_empty())
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
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn lexical_empty_search_keeps_stdout_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    sct()
        .args(["lexical", "definitely-no-such-concept", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("No results"));
}

#[test]
fn fst_empty_search_keeps_stdout_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let index = build_fst(tmp.path());
    sct()
        .args(["fst", "search", "definitely-no-such-concept", "--index"])
        .arg(&index)
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("No results"));
}

/// R11: a retired concept in the FST index is flagged the same way
/// `sct lexical` flags one, since `sct fst search` is a separate query engine
/// with no reason to disagree about which concepts are current.
#[test]
fn fst_search_flags_a_retired_concept() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("inactive.ndjson");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--include-inactive", "--output"])
        .arg(&ndjson)
        .assert()
        .success();
    let index = tmp.path().join("inactive.fst");
    sct()
        .args(["fst", "build", "--ndjson"])
        .arg(&ndjson)
        .arg("--output")
        .arg(&index)
        .assert()
        .success();

    sct()
        .args(["fst", "search", "Inactive example disorder", "--index"])
        .arg(&index)
        .assert()
        .success()
        .stdout(predicate::str::contains("⚠ [INACTIVE] 9468002"));

    // An active concept in the same index carries no marker.
    sct()
        .args(["fst", "search", "Diabetes mellitus", "--index"])
        .arg(&index)
        .assert()
        .success()
        .stdout(predicate::str::contains("[INACTIVE]").not());
}

// --- R7: explicit stdin batches for read commands ---------------------------

#[test]
fn lookup_stdin_batch_is_structured_ordered_and_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    let mut command = sct();
    command
        .args(["lookup", "-", "--format", "json", "--db"])
        .arg(&db)
        .write_stdin("# comment\n22298006 |Myocardial infarction|\nX200\n22298006\n");
    let output = command.output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["input"], "22298006");
    assert_eq!(
        items[0]["result"][0]["preferred_term"],
        "Myocardial infarction"
    );
    assert_eq!(items[1]["input"], "X200");
    assert_eq!(items[1]["result"][0]["id"], "22298006");
    assert_eq!(items[2]["input"], "22298006");

    let mut command = sct();
    command
        .args(["lookup", "-", "--ids", "--db"])
        .arg(&db)
        .write_stdin("22298006\nX200\n22298006\n")
        .assert()
        .success()
        .stdout("22298006\n22298006\n22298006\n");

    let mut command = sct();
    command
        .args(["lookup", "-", "--ids", "--db"])
        .arg(&db)
        .write_stdin("22298006\n999999999\n")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("999999999 not found"));

    let mut command = sct();
    command
        .args(["lookup", "-", "--format", "json", "--db"])
        .arg(&db)
        .write_stdin("22298006\n999999999\n")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("999999999 not found"));
}

#[test]
fn lexical_stdin_batch_emits_one_structured_document() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    let mut command = sct();
    command
        .args(["lexical", "-", "--format", "json", "--limit", "3", "--db"])
        .arg(&db)
        .write_stdin("heart attack\ndiabetes\nheart attack\n");
    let output = command.output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["input"], "heart attack");
    assert_eq!(items[0]["result"][0]["id"], "22298006");
    assert_eq!(items[1]["input"], "diabetes");
    assert!(!items[1]["result"].as_array().unwrap().is_empty());
    assert_eq!(items[2], items[0]);

    let mut command = sct();
    command
        .args(["lexical", "-", "--format", "json", "--db"])
        .arg(&db)
        .write_stdin("heart attack\n\"\n")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty());
}

#[test]
fn refset_single_value_subcommands_accept_stdin_batches() {
    const REFSET: &str = "991381000000107";
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    for subcommand in ["info", "profile"] {
        let mut command = sct();
        command
            .args(["refset", subcommand, "-", "--format", "json", "--db"])
            .arg(&db)
            .write_stdin(format!("{REFSET}\n{REFSET}\n"));
        let output = command.output().unwrap();
        assert!(output.status.success(), "refset {subcommand} failed");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let items = value["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["input"], REFSET);
        assert_eq!(items[1], items[0]);
    }

    let mut command = sct();
    command
        .args(["refset", "members", "-", "--ids", "--db"])
        .arg(&db)
        .write_stdin(format!("{REFSET}\n{REFSET}\n"));
    let output = command.output().unwrap();
    assert!(output.status.success());
    let ids: Vec<_> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(ids.len(), 4);
    assert_eq!(&ids[..2], &ids[2..]);
    assert!(ids.iter().any(|id| id == "46635009"));
    assert!(ids.iter().any(|id| id == "44054006"));

    let mut command = sct();
    command
        .args(["refset", "info", "-", "--format", "json", "--db"])
        .arg(&db)
        .write_stdin(format!("{REFSET}\n999999999\n"))
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("999999999"));
}

#[test]
fn codelist_stdin_roots_expand_descendants_with_and_without_tct() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    for (name, with_tct) in [("recursive", false), ("indexed", true)] {
        if with_tct {
            sct().args(["tct", "--db"]).arg(&db).assert().success();
        }
        let file = tmp.path().join(format!("{name}.codelist"));
        sct()
            .args(["codelist", "new"])
            .arg(&file)
            .args(["--title", "Diabetes", "--no-edit"])
            .assert()
            .success();

        let mut command = sct();
        command
            .args(["codelist", "add"])
            .arg(&file)
            .args(["-", "--include-descendants", "--db"])
            .arg(&db)
            .write_stdin("73211009 |Diabetes mellitus|\n")
            .assert()
            .success();

        let contents = std::fs::read_to_string(file).unwrap();
        for id in ["73211009", "46635009", "44054006"] {
            assert!(contents.contains(id), "{name} output omitted {id}");
        }
    }
}

#[test]
fn codelist_export_ecl_round_trips_through_add_ecl() {
    // A codelist's active member set must survive compress (export --format
    // ecl) and re-expand (add --ecl) unchanged. Diabetes mellitus (73211009)
    // has two active is-a children in the fixture - Type 1 (46635009) and
    // Type 2 (44054006) - so "everything under Diabetes except Type 1" is the
    // same straddling-exclusion shape the ECL compressor's own unit tests
    // exercise, proven here through the real .codelist file format and the
    // full CLI surface end to end.
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    let original = tmp.path().join("original.codelist");
    sct()
        .args(["codelist", "new"])
        .arg(&original)
        .args(["--title", "Diabetes minus Type 1", "--no-edit"])
        .assert()
        .success();
    sct()
        .args(["codelist", "add"])
        .arg(&original)
        .args(["--ecl", "<<73211009 MINUS <<46635009", "--db"])
        .arg(&db)
        .assert()
        .success();

    let output = sct()
        .args(["codelist", "export"])
        .arg(&original)
        .args(["--format", "ecl", "--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let ecl_expr = String::from_utf8(output).unwrap().trim().to_string();
    assert!(
        !ecl_expr.is_empty(),
        "exported ECL expression should not be empty"
    );

    let round_tripped = tmp.path().join("round-tripped.codelist");
    sct()
        .args(["codelist", "new"])
        .arg(&round_tripped)
        .args(["--title", "Round-tripped", "--no-edit"])
        .assert()
        .success();
    sct()
        .args(["codelist", "add"])
        .arg(&round_tripped)
        .args(["--ecl", &ecl_expr, "--db"])
        .arg(&db)
        .assert()
        .success();

    // The fixture's Diabetes mellitus subtree has exactly these three
    // concepts, so presence/absence of all three fully determines the active
    // member set - proving set equality without depending on any particular
    // export format.
    let original_contents = std::fs::read_to_string(&original).unwrap();
    let round_tripped_contents = std::fs::read_to_string(&round_tripped).unwrap();
    for (id, expected_present) in [("73211009", true), ("44054006", true), ("46635009", false)] {
        assert_eq!(
            original_contents.contains(id),
            expected_present,
            "original codelist: unexpected presence of {id}"
        );
        assert_eq!(
            round_tripped_contents.contains(id),
            expected_present,
            "round-tripped codelist: unexpected presence of {id}"
        );
    }
}

#[test]
fn ecl_compress_recognises_exact_refset_membership() {
    // The fixture's simple refset 991381000000107 has exactly two active
    // members - 46635009 and 44054006 (the same Type 1/Type 2 diabetes
    // concepts the round-trip test above uses) - so compressing that exact
    // pair should be recognised as the refset's membership and emitted as a
    // single `^refsetId` cover clause against the real built database, per
    // the roadmap's verification contract (real DB, not just the in-memory
    // unit-test fixture in src/ecl/compress.rs).
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    let output = sct()
        .args(["ecl", "compress", "46635009", "44054006", "--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let ecl_expr = String::from_utf8(output).unwrap().trim().to_string();
    assert_eq!(ecl_expr, "^991381000000107");

    let output = sct()
        .args([
            "ecl", "compress", "46635009", "44054006", "--format", "json", "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["includes"], serde_json::json!(["991381000000107"]));
    assert_eq!(value["include_operator"], "^");
}

// --- R8: one missing-TCT instruction across CLI surfaces --------------------

#[test]
fn missing_tct_guidance_preserves_structured_stdout_and_stops_after_build() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    let output = sct()
        .args(["ecl", "expand", "<<73211009", "--format", "json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr.matches("no usable transitive-closure table").count(),
        1
    );
    assert!(stderr.contains("sct tct --db <db>"));

    sct()
        .args(["ecl", "expand", "73211009", "--format", "json", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stderr(predicate::str::contains("transitive-closure table").not());

    let output = sct()
        .args(["size", "--format", "json", "--sample", "1", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr.matches("no usable transitive-closure table").count(),
        1
    );

    sct().args(["tct", "--db"]).arg(&db).assert().success();
    sct()
        .args(["ecl", "expand", "<<73211009", "--format", "json", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stderr(predicate::str::contains("no usable transitive-closure table").not());
}

#[test]
fn info_reports_and_tct_repairs_missing_indexes() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    sct().args(["tct", "--db"]).arg(&db).assert().success();

    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute("DROP INDEX idx_ca_descendant", []).unwrap();
    drop(conn);

    let inspect = || {
        let output = sct()
            .args(["info", "--format", "json"])
            .arg(&db)
            .output()
            .unwrap();
        assert!(output.status.success());
        (
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    };
    let (before, stderr) = inspect();
    assert!(before["tct_row_count"].as_u64().unwrap() > 0);
    assert_eq!(before["tct_usable"], false);
    assert!(stderr.contains(sct_rs::ecl::TCT_REPAIR_GUIDANCE));

    sct()
        .arg("info")
        .arg(&db)
        .assert()
        .success()
        .stdout(
            predicate::str::contains("TCT:               not usable")
                .and(predicate::str::contains("sct tct --db").not()),
        )
        .stderr(predicate::str::contains("sct tct --db <db>"));

    sct()
        .args([
            "size",
            "--build-tct",
            "--format",
            "json",
            "--sample",
            "1",
            "--db",
        ])
        .arg(&db)
        .assert()
        .success();
    let (after, stderr) = inspect();
    assert_eq!(after["tct_usable"], true);
    assert!(!stderr.contains("no usable transitive-closure table"));
}

/// R11: an inactive concept in search output is prefixed with a flag that a
/// custom line template cannot remove, so a retired code is never mistaken for
/// a live one. Only reachable on an `--include-inactive` database; the default
/// build contains no inactive concepts at all.
#[test]
fn lexical_flags_inactive_concepts_in_search_results() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("inactive.ndjson");
    let db = tmp.path().join("inactive.db");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--include-inactive", "--refsets", "all", "--output"])
        .arg(&ndjson)
        .assert()
        .success();
    sct()
        .args(["sqlite", "--ndjson"])
        .arg(&ndjson)
        .arg("--output")
        .arg(&db)
        .assert()
        .success();

    sct()
        .args(["lexical", "inactive example", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::contains("[INACTIVE]"))
        .stdout(predicate::str::contains("9468002"));

    // A custom template must not be able to drop the flag.
    sct()
        .args(["lexical", "inactive example", "--db"])
        .arg(&db)
        .args(["--template", "{id}"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[INACTIVE]"));

    // An active concept carries no marker.
    sct()
        .args(["lexical", "diabetes mellitus", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::contains("[INACTIVE]").not());
}

/// R11: `--status inactive` answers "which concepts have been retired?", and
/// `--ids` must apply the same filter so a piped codelist matches what was
/// shown on screen.
#[test]
fn lexical_status_filter_selects_retired_concepts() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("inactive.ndjson");
    let db = tmp.path().join("inactive.db");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--include-inactive", "--refsets", "all", "--output"])
        .arg(&ndjson)
        .assert()
        .success();
    sct()
        .args(["sqlite", "--ndjson"])
        .arg(&ndjson)
        .arg("--output")
        .arg(&db)
        .assert()
        .success();

    // Retired only.
    sct()
        .args(["lexical", "disorder", "--status", "inactive", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::contains("9468002"))
        .stdout(predicate::str::contains("Diabetes").not());

    // Current only - the retired concept must not appear.
    sct()
        .args(["lexical", "disorder", "--status", "active", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::contains("9468002").not())
        .stdout(predicate::str::contains("[INACTIVE]").not());

    // --ids filters identically, so the piped set matches the shown set.
    sct()
        .args([
            "lexical", "disorder", "--status", "inactive", "--ids", "--db",
        ])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::diff("9468002\n"));
}

/// R11: a reference set keeps listing a member after SNOMED International
/// retires the concept, so a member row is not evidence the code is current.
/// `sct refset members` must therefore flag retired members too.
#[test]
fn refset_members_flag_retired_concepts() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("inactive.ndjson");
    let db = tmp.path().join("inactive.db");
    sct()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--include-inactive", "--refsets", "all", "--output"])
        .arg(&ndjson)
        .assert()
        .success();
    sct()
        .args(["sqlite", "--ndjson"])
        .arg(&ndjson)
        .arg("--output")
        .arg(&db)
        .assert()
        .success();

    // The fixture's refset lists active concepts; add the retired one so the
    // "refset outlives the concept" case is exercised.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT OR IGNORE INTO refset_members (refset_id, referenced_component_id)
             VALUES ('991381000000107', '9468002')",
            [],
        )
        .unwrap();

    sct()
        .args(["refset", "members", "991381000000107", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout(predicate::str::contains("⚠ [INACTIVE] 9468002"))
        // An active member in the same list carries no marker.
        .stdout(predicate::str::contains("[INACTIVE] 46635009").not());
}

// --- sct bench (R52) --------------------------------------------------------

/// Build a database that is missing one known concept, so `sct bench` has a
/// case whose input genuinely is not there. The provenance line is dropped
/// along with the concept because `sct sqlite` verifies the NDJSON content
/// fingerprint, which no longer matches once a record is removed.
fn build_db_without_concept(dir: &std::path::Path, concept_id: &str) -> PathBuf {
    let full = build_ndjson(dir);
    let text = std::fs::read_to_string(&full).expect("read fixture ndjson");
    let needle = format!("\"id\":\"{concept_id}\"");
    let filtered: String = text
        .lines()
        .filter(|line| !line.contains("\"_type\":\"sct_provenance\"") && !line.contains(&needle))
        .map(|line| format!("{line}\n"))
        .collect();
    assert!(
        filtered.len() < text.len(),
        "the fixture should contain {concept_id}"
    );
    let trimmed = dir.join("without-concept.ndjson");
    std::fs::write(&trimmed, filtered).expect("write filtered ndjson");

    let db = dir.join("without-concept.db");
    sct()
        .args(["sqlite", "--ndjson"])
        .arg(&trimmed)
        .arg("--output")
        .arg(&db)
        .assert()
        .success();
    db
}

/// Small sampling keeps the suite fast; the shape under test is the report,
/// not the timings.
fn bench(db: &std::path::Path) -> Command {
    let mut cmd = sct();
    cmd.args(["bench", "--db"])
        .arg(db)
        .args(["--samples", "2", "--warmup", "1"]);
    cmd
}

#[test]
fn bench_semantic_rejects_inapplicable_parent_options() {
    sct()
        .args(["bench", "--format", "markdown", "semantic"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "supports text, json, and yaml output",
        ));

    sct()
        .args(["bench", "--warmup", "2", "semantic", "--warmup", "3"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--warmup was supplied both before and after",
        ));
}

#[test]
fn bench_json_carries_the_shared_result_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    build_fst(tmp.path());

    let output = bench(&db)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("bench --format json emits valid JSON");

    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["run"]["tool"], "sct bench");
    assert!(value["host"]["os"].is_string());
    assert!(value["host"]["architecture"].is_string());
    assert_eq!(value["policy"]["samples"], 2);
    assert_eq!(value["policy"]["warmup"], 1);
    // The database is identified by file name, never by path.
    assert_eq!(value["dataset"]["database_file"], "fixture.db");
    assert!(value["dataset"]["concept_count"].as_u64().unwrap() > 0);

    let cases = value["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty());
    let lookup = cases
        .iter()
        .find(|c| c["id"] == "lookup_sctid")
        .expect("the lookup case is in the shipped set");
    assert_eq!(lookup["status"], "ok");

    // Raw samples are preserved, not only the aggregates.
    let sdk = &lookup["profiles"]["sdk"];
    assert_eq!(sdk["samples"].as_array().unwrap().len(), 2);
    assert!(sdk["summary"]["median_ns"].as_u64().unwrap() > 0);
    assert!(sdk["summary"]["p95_ns"].as_u64().is_some());
    // The cli profile pays for process startup, so it cannot be the faster one.
    let cli = &lookup["profiles"]["cli"];
    assert!(
        cli["summary"]["median_ns"].as_u64().unwrap()
            > sdk["summary"]["median_ns"].as_u64().unwrap()
    );
}

#[test]
fn bench_text_and_markdown_render() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    bench(&db)
        .args(["--format", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("sct bench"))
        .stdout(predicate::str::contains("lookup by SCTID"))
        .stdout(predicate::str::contains(
            "2 samples per case after 1 warm-up runs",
        ))
        .stdout(predicate::str::contains(
            "Single run on an uncontrolled machine",
        ))
        .stdout(predicate::str::contains("Share:"));

    bench(&db)
        .args(["--format", "markdown"])
        .assert()
        .success()
        .stdout(predicate::str::contains("| Operation |"))
        .stdout(predicate::str::contains("```text"));
}

#[test]
fn bench_html_is_self_contained() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    let report = tmp.path().join("report.html");

    bench(&db)
        .args(["--format", "html", "--output"])
        .arg(&report)
        .assert()
        .success();

    let html = std::fs::read_to_string(&report).expect("html report written");
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("<style>"));
    for forbidden in ["http://", "https://", "<script", "src=", "@import"] {
        assert!(
            !html.contains(forbidden),
            "self-contained HTML must not contain {forbidden}"
        );
    }
}

#[test]
fn bench_skips_cases_whose_concepts_are_absent() {
    let tmp = tempfile::tempdir().unwrap();
    // 22298006 (Myocardial infarction) drives the lookup and ancestors cases.
    let db = build_db_without_concept(tmp.path(), "22298006");

    let output = bench(&db)
        .args(["--format", "json"])
        .assert()
        // A missing concept degrades one case, it does not fail the run.
        .success()
        .get_output()
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let cases = value["cases"].as_array().unwrap();

    let lookup = cases.iter().find(|c| c["id"] == "lookup_sctid").unwrap();
    assert_eq!(lookup["status"], "skipped");
    assert!(lookup["skipped_reason"]
        .as_str()
        .unwrap()
        .contains("22298006"));
    // A skipped case is never timed against the missing row.
    assert!(lookup["profiles"].as_object().unwrap().is_empty());

    // Cases whose concepts are still present are measured as usual.
    let subsumption = cases.iter().find(|c| c["id"] == "subsumption").unwrap();
    assert_eq!(subsumption["status"], "ok");
    assert!(subsumption["profiles"]["sdk"]["summary"]["median_ns"]
        .as_u64()
        .is_some());

    bench(&db)
        .args(["--format", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Not measured"))
        .stdout(predicate::str::contains(
            "concept 22298006 is not present in this database",
        ));
}

#[test]
fn bench_output_never_leaks_a_path_or_a_hostname() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    build_fst(tmp.path());

    let dir = tmp.path().to_string_lossy().into_owned();
    let hostname = std::fs::read_to_string("/etc/hostname")
        .map(|h| h.trim().to_string())
        .unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();

    for format in ["text", "markdown", "json"] {
        let output = bench(&db)
            .args(["--format", format])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).expect("utf8 report");
        assert!(
            !stdout.contains(&dir),
            "{format} output leaked the database directory"
        );
        assert!(
            !stdout.contains(&db.to_string_lossy().into_owned()),
            "{format} output leaked the database path"
        );
        // The file name itself is expected; the path around it is not.
        assert!(stdout.contains("fixture.db"));
        if hostname.len() > 3 {
            assert!(
                !stdout.contains(&hostname),
                "{format} output leaked the hostname"
            );
        }
        if user.len() > 3 {
            assert!(
                !stdout.contains(&user),
                "{format} output leaked the username"
            );
        }
    }
}

#[test]
fn bench_no_provenance_withholds_release_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    let with = bench(&db)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let with: serde_json::Value = serde_json::from_slice(&with.stdout).unwrap();
    assert!(with["dataset"]["release_id"].is_string());

    let without = bench(&db)
        .args(["--format", "json", "--no-provenance"])
        .assert()
        .success()
        .get_output()
        .clone();
    let without: serde_json::Value = serde_json::from_slice(&without.stdout).unwrap();
    assert!(without["dataset"]["release_id"].is_null());
    assert!(without["dataset"]["edition"].is_null());
    assert_eq!(without["dataset"]["provenance_suppressed"], true);
    // ... but the non-identifying facts survive.
    assert!(without["dataset"]["concept_count"].as_u64().unwrap() > 0);
    assert!(without["dataset"]["schema_version"].as_u64().is_some());
}

#[test]
fn bench_baseline_flags_only_out_of_band_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    let baseline = tmp.path().join("baseline.json");

    bench(&db)
        .args(["--format", "json", "--output"])
        .arg(&baseline)
        .assert()
        .success();

    let output = bench(&db)
        .args(["--format", "json", "--baseline"])
        .arg(&baseline)
        .assert()
        .success()
        .get_output()
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let deltas = value["baseline"].as_array().expect("baseline deltas");
    assert!(!deltas.is_empty(), "the baseline shares every case id");
    for delta in deltas {
        let verdict = delta["verdict"].as_str().unwrap();
        assert!(
            ["noise", "faster", "slower"].contains(&verdict),
            "unexpected verdict {verdict}"
        );
        let change = delta["change_pct"].as_f64().unwrap();
        // Anything inside the band must be called noise, not a regression.
        if change.abs() <= 15.0 {
            assert_eq!(verdict, "noise");
        }
    }
}

#[test]
fn bench_artefact_profile_reports_sizes_without_timing() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());
    build_fst(tmp.path());

    let output = sct()
        .args(["bench", "--db"])
        .arg(&db)
        .args(["--profiles", "artefact", "--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(value["policy"]["profiles"], serde_json::json!(["artefact"]));
    assert!(
        value["dataset"]["artefacts"]["database_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(value["dataset"]["artefacts"]["fst_bytes"].as_u64().unwrap() > 0);
    // Static inspection produces no samples at all.
    assert!(value["cases"].as_array().unwrap().is_empty());
}

// --- R59: CLI stdout/stderr and exit-code discipline ---------------------

/// Assert the `sct` CLI honours the AGENTS.md contract: machine-readable
/// output on stdout, human hints on stderr, exit 0 success / 1 unresolved
/// single-item lookup / 2 usage error. Each case is driven against the real
/// binary using the committed fixture.
#[test]
fn cli_stdout_stderr_exit_code_discipline() {
    let tmp = tempfile::tempdir().unwrap();
    let db = build_db(tmp.path());

    // 1. Known SCTID lookup: result on stdout, exit 0, stderr can be empty.
    sct()
        .args(["lookup", "22298006", "--db"])
        .arg(&db)
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("Myocardial infarction"))
        .stderr(predicate::str::is_empty().or(predicate::str::contains("edition")));

    // 2. Unknown SCTID lookup: empty stdout, hint on stderr, exit 1.
    sct()
        .args(["lookup", "999999999", "--db"])
        .arg(&db)
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("not found"));

    // 3. Lexical search with no matches: empty stdout, hint on stderr, exit 0.
    sct()
        .args(["lexical", "definitely-no-such-concept", "--db"])
        .arg(&db)
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("No results"));

    // 4. Same search with --format json: empty array on stdout, exit 0.
    sct()
        .args([
            "lexical",
            "definitely-no-such-concept",
            "--format",
            "json",
            "--db",
        ])
        .arg(&db)
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("[]").or(predicate::str::contains("\"results\":[]")))
        .stderr(predicate::str::is_empty());

    // 5. Usage error (unknown flag): exit 2, message on stderr, empty stdout.
    sct()
        .args(["lookup", "22298006", "--definitely-not-a-flag"])
        .assert()
        .failure()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("unexpected").or(predicate::str::contains("invalid")));
}
