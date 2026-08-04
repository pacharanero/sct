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
