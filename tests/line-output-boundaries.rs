// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

use assert_cmd::Command;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

fn sct(dir: &Path) -> Command {
    let mut command = Command::cargo_bin("sct").unwrap();
    command
        .current_dir(dir)
        .env("SCT_DATA_HOME", dir)
        .env("SCT_CONFIG_HOME", dir)
        .env_remove("SCT_CONFIG");
    command
}

fn build(dir: &Path) -> PathBuf {
    let ndjson = dir.join("fixture.ndjson");
    let db = dir.join("fixture.db");
    sct(dir)
        .args(["ndjson", "--rf2"])
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z"),
        )
        .args(["--refsets", "simple", "--output"])
        .arg(&ndjson)
        .assert()
        .success();
    sct(dir)
        .args(["sqlite", "--ndjson"])
        .arg(ndjson)
        .arg("--output")
        .arg(&db)
        .assert()
        .success();
    db
}

#[test]
fn text_records_keep_delimiters_out_of_values_but_json_preserves_them() {
    let dir = tempfile::tempdir().unwrap();
    let db = build(dir.path());
    let conn = Connection::open(&db).unwrap();
    let term = "Boundaryprobe\n46635009\tInjected\r\u{1b}[0m";
    conn.execute(
        "UPDATE concepts SET preferred_term = ?1, fsn = ?1 WHERE id = '22298006'",
        [term],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild')",
        [],
    )
    .unwrap();
    for args in [
        vec!["lexical", "Boundaryprobe"],
        vec!["refset", "members", "999001"],
    ] {
        conn.execute(
            "INSERT OR IGNORE INTO refset_members VALUES ('999001', '22298006')",
            [],
        )
        .unwrap();
        let output = sct(dir.path())
            .args(args)
            .args(["--db"])
            .arg(&db)
            .args([
                "--template",
                "{id}\t{pt}",
                "--template-fsn-suffix",
                "",
                "--no-provenance",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(text, "22298006\tBoundaryprobe 46635009 Injected  [0m\n");
    }
    let output = sct(dir.path())
        .args([
            "lookup",
            "22298006",
            "--format",
            "json",
            "--no-provenance",
            "--db",
        ])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["preferred_term"], term);
    conn.execute(
        "UPDATE concepts SET definition_status = '900000000000074008' WHERE id = '22298006'",
        [],
    )
    .unwrap();
    let output = sct(dir.path())
        .args(["proximal-primitives", "22298006", "--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "22298006\tBoundaryprobe 46635009 Injected  [0m\n"
    );
    let output = sct(dir.path())
        .args(["lookup", "-", "--no-provenance", "--db"])
        .arg(&db)
        .write_stdin("22298006\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(String::from_utf8(output).unwrap().lines().count(), 1);
}

#[test]
fn ids_reject_malformed_stored_values_before_single_or_batch_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let db = build(dir.path());
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE concepts SET preferred_term = 'Invalidprobe' WHERE id = '22298006'",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO refset_members VALUES ('999001', '22298006')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO concept_maps (code, terminology, concept_id) VALUES ('safe', 'ctv3', '22298006')", []).unwrap();
    let mut previous = "46635009".to_string();
    for bad in [
        "46635009\n22298006",
        "46635009\r22298006",
        "46635009\t22298006",
        "46635009 22298006",
        "-46635009",
        "",
        "\u{ff14}6635009",
    ] {
        conn.execute(
            "UPDATE concepts SET id = ?1, preferred_term = 'Invalidprobe' WHERE id = ?2",
            params![bad, previous],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM refset_members WHERE refset_id = '999002'", [])
            .unwrap();
        conn.execute("INSERT INTO refset_members VALUES ('999002', ?1)", [bad])
            .unwrap();
        conn.execute(
            "INSERT INTO refset_members VALUES ('999002', '22298006')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM concept_maps WHERE code = 'bad'", [])
            .unwrap();
        conn.execute(
            "INSERT INTO concept_maps (code, terminology, concept_id) VALUES ('bad', 'ctv3', ?1)",
            [bad],
        )
        .unwrap();
        for (args, stdin) in [
            (vec!["lexical", "Invalidprobe"], ""),
            (vec!["lexical", "-"], "Myocardial\nInvalidprobe\n"),
            (vec!["refset", "members", "999002"], ""),
            (vec!["refset", "members", "-"], "999001\n999002\n"),
            (vec!["lookup", "bad"], ""),
            (vec!["lookup", "-"], "safe\nbad\n"),
        ] {
            sct(dir.path())
                .args(args)
                .args(["--ids", "--db"])
                .arg(&db)
                .write_stdin(stdin)
                .assert()
                .failure()
                .stdout("");
        }
        previous = bad.to_string();
    }
    sct(dir.path())
        .args(["lookup", "22298006", "--ids", "--db"])
        .arg(&db)
        .assert()
        .success()
        .stdout("22298006\n");
}

#[test]
fn info_and_provenance_do_not_render_source_control_characters() {
    let dir = tempfile::tempdir().unwrap();
    let db = build(dir.path());
    let conn = Connection::open(&db).unwrap();
    let mut provenance = sct_rs::provenance::read_sqlite(&conn).unwrap().unwrap();
    provenance.edition_label = "Edition\nFORGED\t\u{1b}[0m".into();
    sct_rs::provenance::write_sqlite(&conn, &provenance).unwrap();
    conn.execute(
        "UPDATE concepts SET hierarchy = 'Hierarchy' || char(10) || 'FORGED' WHERE id = '22298006'",
        [],
    )
    .unwrap();
    let output = sct(dir.path())
        .args(["info"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert!(!text.contains("\nFORGED"));
    assert!(!text.contains(['\t', '\r', '\u{1b}']));
    assert!(!provenance.human_footer().contains("\nFORGED"));
    let output = sct(dir.path())
        .args(["info"])
        .arg(&db)
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["edition"], provenance.edition_label);
}

#[test]
fn fst_labels_are_single_line_while_stdio_json_preserves_the_term() {
    let dir = tempfile::tempdir().unwrap();
    build(dir.path());
    let source = std::fs::read_to_string(dir.path().join("fixture.ndjson")).unwrap();
    let mut record: serde_json::Value = source
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|value| value["id"] == "22298006")
        .unwrap();
    let term = "Boundaryprobe\n46635009\tInjected\r\u{1b}[0m";
    record["preferred_term"] = term.into();
    let input = dir.path().join("display.ndjson");
    std::fs::write(&input, serde_json::to_string(&record).unwrap() + "\n").unwrap();
    let index = dir.path().join("display.fst");
    sct(dir.path())
        .args(["fst", "build", "--ndjson"])
        .arg(&input)
        .arg("--output")
        .arg(&index)
        .assert()
        .success();
    let output = sct(dir.path())
        .args(["fst", "search", "Boundaryprobe", "--prefix", "--index"])
        .arg(&index)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.starts_with("22298006"));
    assert!(!text.contains(['\r', '\t', '\u{1b}']));
    let output = sct(dir.path())
        .args(["sayt", "--stdio", "--index"])
        .arg(&index)
        .write_stdin("Boundaryprobe\n")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(String::from_utf8_lossy(&output).lines().count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["hits"][0]["display"], term);
}
