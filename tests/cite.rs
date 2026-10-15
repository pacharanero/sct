// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::{params, Connection};
use sct_rs::citation::Citation;
use sct_rs::commands::ndjson::{self, RefsetMode};
use sct_rs::commands::sqlite;
use sct_rs::provenance::{self, Provenance};
use serde::Deserialize;

fn sct() -> Command {
    Command::cargo_bin("sct").expect("sct binary builds")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn build_db(dir: &Path) -> PathBuf {
    let ndjson = dir.join("citation.ndjson");
    let db = dir.join("citation.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![fixture_dir()],
        locale: "en-GB".to_string(),
        output: Some(ndjson.clone()),
        include_inactive: true,
        refsets: RefsetMode::All,
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

fn read_provenance(db: &Path) -> Provenance {
    provenance::read_sqlite(&Connection::open(db).unwrap())
        .unwrap()
        .unwrap()
}

fn output(db: &Path, format: &str) -> String {
    let assert = sct()
        .args(["cite", "--db"])
        .arg(db)
        .args(["--format", format])
        .assert()
        .success()
        .stderr("");
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

#[test]
fn real_pipeline_citations_preserve_exact_recorded_identity() {
    let dir = tempfile::tempdir().unwrap();
    let db = build_db(dir.path());
    let conn = Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE metadata SET value = ?1 WHERE key = 'sct_version'",
        params!["0.0.1-recorded-producer"],
    )
    .unwrap();
    drop(conn);

    let recorded = read_provenance(&db);
    assert!(recorded.content_fingerprint.is_some());
    assert_eq!(recorded.companions.len(), 2);

    let text = output(&db, "text");
    let bibtex = output(&db, "bibtex");
    let csljson = output(&db, "csljson");
    for rendered in [&text, &bibtex, &csljson] {
        assert!(rendered.contains(recorded.content_fingerprint.as_deref().unwrap()));
        assert!(rendered.contains("0.0.1-recorded-producer"));
        assert!(rendered.contains("inferred during ingestion from RF2 input naming"));
        for companion in &recorded.companions {
            assert!(rendered.contains(&companion.content_fingerprint));
        }
        for source_path in &recorded.source_paths {
            assert!(!rendered.contains(source_path));
        }
        assert!(!rendered.contains("doi"));
    }
    for companion in &recorded.companions {
        assert!(text.contains(&companion.kind));
        assert!(csljson.contains(&companion.kind));
    }
    assert!(text.contains(&format!("Reader: sct {}", env!("CARGO_PKG_VERSION"))));
    assert_eq!(bibtex.matches("@misc{").count(), 3);

    let entries: serde_json::Value = serde_json::from_str(&csljson).unwrap();
    let entries = entries.as_array().unwrap();
    assert_eq!(entries.len(), 3);
    let software = entries
        .iter()
        .find(|entry| entry["id"] == "sct-software")
        .unwrap();
    assert_eq!(software["version"], "0.0.1-recorded-producer");
    assert_eq!(software["author"][0]["family"], "Baw");
    assert!(software.get("issued").is_none());
    let artefact = entries
        .iter()
        .find(|entry| entry["id"] == "sct-local-artefact")
        .unwrap();
    assert!(artefact.get("DOI").is_none());
    assert!(artefact.get("URL").is_none());

    for alias in ["csl-json", "json"] {
        let alias_value: serde_json::Value = serde_json::from_str(&output(&db, alias)).unwrap();
        assert_eq!(alias_value, serde_json::Value::Array(entries.clone()));
    }
}

#[test]
fn cite_does_not_mutate_a_moved_database() {
    let built = tempfile::tempdir().unwrap();
    let original = build_db(built.path());
    let moved = tempfile::tempdir().unwrap();
    let db = moved.path().join("renamed.db");
    std::fs::rename(original, &db).unwrap();

    let before_bytes = std::fs::read(&db).unwrap();
    let before_files = directory_entries(moved.path());
    let text = output(&db, "text");
    assert!(text.contains("Local derived artefact"));
    assert_eq!(std::fs::read(&db).unwrap(), before_bytes);
    for created in directory_entries(moved.path()).difference(&before_files) {
        let name = created.file_name().unwrap().to_string_lossy();
        assert!(
            name == "renamed.db-wal" || name == "renamed.db-shm",
            "unexpected file created by citation: {}",
            created.display()
        );
    }
}

#[test]
fn missing_partial_and_malformed_provenance_are_distinct() {
    let dir = tempfile::tempdir().unwrap();

    let absent = dir.path().join("absent.db");
    Connection::open(&absent).unwrap();
    sct()
        .args(["cite", "--db"])
        .arg(&absent)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no usable citation provenance"));

    let partial = dir.path().join("partial.db");
    let conn = Connection::open(&partial).unwrap();
    provenance::create_sqlite_table(&conn).unwrap();
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('release_id', 'LocalRelease_20260101')",
        [],
    )
    .unwrap();
    drop(conn);
    let partial_json: serde_json::Value = serde_json::from_str(&output(&partial, "json")).unwrap();
    let artefact_note = partial_json[1]["note"].as_str().unwrap();
    assert!(artefact_note.contains("fingerprint is unavailable"));
    assert!(artefact_note.contains("producer version is unavailable"));

    let malformed = dir.path().join("malformed.db");
    let conn = Connection::open(&malformed).unwrap();
    provenance::create_sqlite_table(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO metadata (key, value) VALUES ('edition_label', 'International');
         INSERT INTO metadata (key, value) VALUES ('companions', 'not-json');",
    )
    .unwrap();
    drop(conn);
    sct()
        .args(["cite", "--db"])
        .arg(&malformed)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "parsing SQLite provenance metadata key `companions`",
        ));
}

#[test]
fn unknown_format_fails_before_database_discovery() {
    let dir = tempfile::tempdir().unwrap();
    sct()
        .args(["cite", "--format", "not-a-format", "--db"])
        .arg(dir.path().join("missing.db"))
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid value"))
        .stderr(predicate::str::contains("not-a-format"))
        .stderr(predicate::str::contains("read-only").not());
}

#[cfg(unix)]
#[test]
fn cite_handles_a_closed_output_pipe_without_a_panic() {
    use std::process::Stdio;

    let dir = tempfile::tempdir().unwrap();
    let db = build_db(dir.path());
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sct"))
        .args(["cite", "--db"])
        .arg(&db)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let result = child.wait_with_output().unwrap();
    assert!(!String::from_utf8_lossy(&result.stderr).contains("panicked"));
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CitationCff {
    title: String,
    version: String,
    repository_code: String,
    license: String,
    authors: Vec<CffAuthor>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CffAuthor {
    family_names: String,
    given_names: String,
}

#[test]
fn citation_cff_matches_the_software_citation() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("CITATION.cff");
    let cff: CitationCff =
        serde_yaml_ng::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let software = Citation::from_provenance(&Provenance {
        type_tag: provenance::NDJSON_TYPE_TAG.to_string(),
        edition_label: "International".to_string(),
        release_date: "2026-01-01".to_string(),
        release_id: "SnomedCT_InternationalRF2_PRODUCTION_20260101T120000Z".to_string(),
        content_fingerprint: None,
        companions: Vec::new(),
        source_paths: vec!["redacted-by-model".to_string()],
        sct_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: String::new(),
    })
    .unwrap()
    .software;
    assert_eq!(cff.title, software.name);
    assert_eq!(cff.version, software.producer_version.unwrap());
    assert_eq!(cff.repository_code, software.repository);
    assert_eq!(cff.license, env!("CARGO_PKG_LICENSE"));
    assert_eq!(cff.authors.len(), 1);
    assert_eq!(cff.authors[0].family_names, software.author.family);
    assert_eq!(cff.authors[0].given_names, software.author.given);
}

fn directory_entries(path: &Path) -> BTreeSet<PathBuf> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}
