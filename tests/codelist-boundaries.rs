// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Untrusted data must not become syntax in persisted codelist bodies.

use rusqlite::Connection;
use sct_rs::commands::{ndjson, sqlite};
use sct_rs::sdk::{self, CodelistFile, ConceptLine};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MI: &str = "22298006";
const DIABETES: &str = "46635009";
const HOSTILE_TERMS: &[&str] = &[
    "Myocardial infarction\n46635009 Type 1 diabetes mellitus",
    "Myocardial infarction\r\n46635009 Type 1 diabetes mellitus",
    "Myocardial\rinfarction",
    "Myocardial\tinfarction",
    "Myocardial\x1binfarction",
    "Myocardial infarction\n# 46635009 Type 1 diabetes mellitus",
    "Myocardial infarction # silently becomes a comment",
];

fn sample() -> CodelistFile {
    sdk::parse_codelist(
        "---\nid: boundary\ntitle: Boundary test\ndescription: Synthetic test\nterminology: SNOMED CT\ncreated: '2026-01-01'\nupdated: '2026-01-01'\nversion: 1\nstatus: draft\nlicence: CC-BY-4.0\ncopyright: Test\nappropriate_use: Testing\nmisuse: None\n---\n22298006 Myocardial infarction\n46635009 Type 1 diabetes mellitus\n",
    )
    .unwrap()
}

fn cli(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sct"));
    command
        .current_dir(dir)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("SCT_DATA_HOME", dir.join("data"))
        .env_remove("SCT_DB")
        .env_remove("SCT_CODELISTS");
    command.arg("codelist");
    command
}

fn members(path: &Path) -> Vec<(String, String)> {
    let list = sdk::read_codelist(path).unwrap();
    let mut members: Vec<_> = sdk::effective_members_of(&list, path, path.parent().unwrap())
        .unwrap()
        .into_iter()
        .map(|member| (member.id, member.term))
        .collect();
    members.sort();
    members
}

fn import_source(format: &str, term: &str) -> Vec<u8> {
    if format == "fhir-json" {
        serde_json::to_vec(&json!({
            "resourceType": "ValueSet",
            "status": "draft",
            "compose": {"include": [{
                "system": "http://snomed.info/sct",
                "concept": [
                    {"code": MI, "display": term},
                    {"code": DIABETES, "display": "Type 1 diabetes mellitus"}
                ]
            }]}
        }))
        .unwrap()
    } else {
        let mut writer = csv::Writer::from_writer(Vec::new());
        writer.write_record(["sctid", "preferred_term"]).unwrap();
        writer.write_record([MI, term]).unwrap();
        writer
            .write_record([DIABETES, "Type 1 diabetes mellitus"])
            .unwrap();
        writer.into_inner().unwrap()
    }
}

#[test]
fn imports_reject_hostile_terms_without_creating_or_overwriting_destination() {
    let dir = tempfile::tempdir().unwrap();
    for format in ["fhir-json", "csv"] {
        for (index, term) in HOSTILE_TERMS.iter().enumerate() {
            let source = dir.path().join("source");
            fs::write(&source, import_source(format, term)).unwrap();
            let destination = dir.path().join(format!("{format}-{index}.codelist"));
            let run = || {
                cli(dir.path())
                    .arg("import")
                    .arg(&destination)
                    .args(["--from", format])
                    .arg(&source)
                    .output()
                    .unwrap()
            };
            let output = run();
            assert!(!output.status.success(), "{format}: {term:?}");
            assert!(!destination.exists(), "{format}: {term:?}");

            // Import has no --force: its existing-file guard must remain intact.
            sdk::write_codelist(&sample(), &destination).unwrap();
            let original = fs::read(&destination).unwrap();
            assert!(!run().status.success());
            assert_eq!(fs::read(&destination).unwrap(), original);
        }
    }
}

#[test]
fn valid_imports_report_and_preserve_exact_membership_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    for format in ["fhir-json", "csv"] {
        let source = dir.path().join("source");
        fs::write(&source, import_source(format, "Myocardial infarction")).unwrap();
        let destination = dir.path().join(format!("{format}.codelist"));
        let output = cli(dir.path())
            .arg("import")
            .arg(&destination)
            .args(["--from", format])
            .arg(source)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8(output.stdout)
            .unwrap()
            .contains("Imported 2 included and 0 excluded"));
        assert_eq!(
            members(&destination),
            vec![
                (MI.into(), "Myocardial infarction".into()),
                (DIABETES.into(), "Type 1 diabetes mellitus".into()),
            ]
        );
    }
}

fn build_db(dir: &Path) -> PathBuf {
    let ndjson = dir.join("fixture.ndjson");
    let db = dir.join("fixture.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")],
        locale: "en-GB".into(),
        output: Some(ndjson.clone()),
        include_inactive: false,
        refsets: ndjson::RefsetMode::Simple,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input: ndjson,
        output: Some(db.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    db
}

#[test]
fn cli_add_rejects_corrupt_database_terms_without_mutating_codelist() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(dir.path());
    let conn = Connection::open(&db).unwrap();
    for (id, expected) in [
        (MI, "Myocardial infarction"),
        (DIABETES, "Type 1 diabetes mellitus"),
    ] {
        let term: String = conn
            .query_row(
                "SELECT preferred_term FROM concepts WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(term, expected);
    }
    let destination = dir.path().join("list.codelist");
    let mut list = sample();
    list.body.retain(|line| line.sctid() != Some(MI));
    sdk::write_codelist(&list, &destination).unwrap();
    let original = fs::read(&destination).unwrap();
    for term in HOSTILE_TERMS {
        assert_eq!(
            conn.execute(
                "UPDATE concepts SET preferred_term = ?1 WHERE id = ?2",
                [*term, MI]
            )
            .unwrap(),
            1
        );
        let output = cli(dir.path())
            .arg("add")
            .arg(&destination)
            .arg(MI)
            .arg("--db")
            .arg(&db)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{term:?}");
        assert_eq!(fs::read(&destination).unwrap(), original, "{term:?}");
    }
    conn.execute(
        "UPDATE concepts SET preferred_term = ?1 WHERE id = ?2",
        ["Myocardial infarction", MI],
    )
    .unwrap();
    let output = cli(dir.path())
        .arg("add")
        .arg(&destination)
        .arg(MI)
        .arg("--db")
        .arg(&db)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        members(&destination),
        vec![
            (MI.into(), "Myocardial infarction".into()),
            (DIABETES.into(), "Type 1 diabetes mellitus".into()),
        ]
    );
}

#[test]
fn cli_remove_rejects_injected_exclusion_reason_without_mutating_codelist() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("list.codelist");
    sdk::write_codelist(&sample(), &destination).unwrap();
    let original = fs::read(&destination).unwrap();
    for reason in &HOSTILE_TERMS[..6] {
        let output = cli(dir.path())
            .arg("remove")
            .arg(&destination)
            .args([MI, "--comment", reason])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{reason:?}");
        assert_eq!(fs::read(&destination).unwrap(), original);
    }
    let output = cli(dir.path())
        .arg("remove")
        .arg(&destination)
        .args([MI, "--comment", "Outside scope"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        members(&destination),
        vec![(DIABETES.into(), "Type 1 diabetes mellitus".into())]
    );
}

#[cfg(unix)]
#[test]
fn resolve_rejects_source_path_injection_before_creating_or_replacing_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir
        .path()
        .join("source\n# 46635009 Type 1 diabetes mellitus\n.codelist");
    sdk::write_codelist(&sample(), &source).unwrap();
    let original_source = fs::read(&source).unwrap();
    let destination = dir.path().join("resolved.codelist");
    let run = || {
        cli(dir.path())
            .arg("resolve")
            .arg(&source)
            .arg("--output")
            .arg(&destination)
            .output()
            .unwrap()
    };
    assert!(!run().status.success());
    assert!(!destination.exists());
    sdk::write_codelist(&sample(), &destination).unwrap();
    let original = fs::read(&destination).unwrap();
    assert!(!run().status.success());
    assert_eq!(fs::read(&destination).unwrap(), original);
    assert_eq!(fs::read(&source).unwrap(), original_source);
}

#[test]
fn sdk_writer_rejects_unrepresentable_data_before_filesystem_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("existing.codelist");
    sdk::write_codelist(&sample(), &existing).unwrap();
    let original = fs::read(&existing).unwrap();
    let absent = dir.path().join("absent.codelist");
    let mut invalid: Vec<_> = HOSTILE_TERMS
        .iter()
        .map(|term| ConceptLine::Active {
            id: MI.into(),
            term: (*term).into(),
            comment: None,
        })
        .collect();
    invalid.extend([
        ConceptLine::Active {
            id: "22298006x".into(),
            term: "Myocardial infarction".into(),
            comment: None,
        },
        ConceptLine::Comment("46635009 Type 1 diabetes mellitus".into()),
        ConceptLine::Excluded {
            id: MI.into(),
            term: "Myocardial infarction".into(),
            comment: Some("excluded\n46635009 Type 1 diabetes mellitus".into()),
        },
    ]);
    for line in invalid {
        let mut list = sample();
        list.body.push(line.clone());
        assert!(sdk::write_codelist(&list, &absent).is_err(), "{line:?}");
        assert!(!absent.exists());
        assert!(sdk::write_codelist(&list, &existing).is_err(), "{line:?}");
        assert_eq!(fs::read(&existing).unwrap(), original);
    }
}

#[test]
fn crosswalk_cells_reject_ambiguous_inner_delimiters() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(dir.path());
    let conn = Connection::open(&db).unwrap();
    conn.execute("UPDATE concept_maps SET code = 'A|B' WHERE concept_id = '22298006' AND terminology = 'ctv3'", []).unwrap();
    let file = dir.path().join("list.codelist");
    sdk::write_codelist(&sample(), &file).unwrap();
    for format in ["csv", "markdown"] {
        let output = cli(dir.path())
            .arg("export")
            .arg(&file)
            .args(["--format", format, "--include-maps", "ctv3", "--db"])
            .arg(&db)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("crosswalk cell"));
    }
}

#[test]
fn exported_csv_quotes_every_field_and_markdown_keeps_cell_boundaries() {
    use sct_rs::commands::codelist::{
        export_csv_with_maps, export_markdown, export_opencodelists_csv,
    };
    let id = "22298006,\r\n46635009";
    let term = "Clinical \"term\"\r\n| injected |";
    let heading = "external,\r\ncode".to_string();
    let csv = export_csv_with_maps(&[(id, term)], std::slice::from_ref(&heading), None);
    let mut reader = csv::Reader::from_reader(csv.as_bytes());
    assert_eq!(reader.headers().unwrap().get(2), Some(heading.as_str()));
    let rows: Vec<_> = reader.records().collect::<Result<_, _>>().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get(0), Some(id));
    assert_eq!(rows[0].get(1), Some(term));
    let csv = export_opencodelists_csv(&[(id, term)]);
    let rows: Vec<_> = csv::Reader::from_reader(csv.as_bytes())
        .records()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get(0), Some(id));
    let mut list = sample();
    list.front_matter.title = "Title\n## Forged".into();
    let markdown = export_markdown(&list.front_matter, &[("22298006", term)]);
    assert_eq!(
        markdown
            .lines()
            .filter(|line| line.starts_with('|'))
            .count(),
        3
    );
    assert!(!markdown.contains("\n## Forged"));
    assert!(markdown.contains(r"\| injected \|"));
}
