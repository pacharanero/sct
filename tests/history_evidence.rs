// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! R93: explicit history operations are gated on the provenance-bound `history`
//! companion marker, not on the always-present `concept_history` table.

use sct_rs::commands::{ndjson, sqlite};
use sct_rs::sdk::{Snomed, Terminology};
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn build(rf2: &Path, refsets: ndjson::RefsetMode) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("synthetic.ndjson");
    let db = dir.path().join("synthetic.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![rf2.to_path_buf()],
        locale: "en-GB".into(),
        output: Some(input.clone()),
        include_inactive: true,
        refsets,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input,
        output: Some(db.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    (dir, db)
}

fn expand(db: &Path, expr: &str) -> Result<Vec<String>, String> {
    let mut ids = Snomed::open(db)
        .unwrap()
        .expand(expr)
        .map_err(|e| e.to_string())?;
    ids.sort();
    Ok(ids)
}

#[test]
fn simple_build_refuses_explicit_history_requests() {
    let (_dir, db) = build(&fixture(), ndjson::RefsetMode::Simple);
    let sdk = Snomed::open(&db).unwrap();

    let error = expand(&db, "22298006 {{ + HISTORY-MIN }}").unwrap_err();
    assert!(error.contains("--refsets all"), "unexpected error: {error}");
    assert!(sdk.history("9468002").is_err());
    assert!(sdk
        .map_forwarding_history(Terminology::Ctv3, "X", Terminology::Snomed)
        .is_err());

    // Requests that do not ask for history are unaffected.
    assert_eq!(expand(&db, "22298006").unwrap(), vec!["22298006"]);
    assert!(sdk.map(Terminology::Ctv3, "X", Terminology::Snomed).is_ok());
    assert!(sdk.concept_history("9468002").unwrap().is_some());
}

#[test]
fn all_refsets_build_supplies_history() {
    let (_dir, db) = build(&fixture(), ndjson::RefsetMode::All);
    assert_eq!(
        expand(&db, "195967001 {{ + HISTORY-MIN }}").unwrap(),
        vec!["195967001", "9468002"]
    );
    assert_eq!(
        Snomed::open(&db).unwrap().history("9468002").unwrap().len(),
        2
    );
}

#[test]
fn header_only_association_file_is_loaded_but_empty() {
    let dir = tempfile::tempdir().unwrap();
    let rf2 = dir.path().join("rf2");
    copy_dir(&fixture(), &rf2);
    let assoc = walk(&rf2)
        .into_iter()
        .find(|p| p.to_string_lossy().contains("AssociationSnapshot"))
        .expect("association file in fixture");
    let header = std::fs::read_to_string(&assoc)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    std::fs::write(&assoc, format!("{header}\n")).unwrap();

    let (_built, db) = build(&rf2, ndjson::RefsetMode::All);
    let sdk = Snomed::open(&db).unwrap();
    assert_eq!(
        expand(&db, "195967001 {{ + HISTORY-MIN }}").unwrap(),
        vec!["195967001"]
    );
    assert!(sdk.history("9468002").unwrap().is_empty());
    assert!(sdk
        .map_forwarding_history(Terminology::Ctv3, "X", Terminology::Snomed)
        .is_ok());
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn copy_dir(from: &Path, to: &Path) {
    for path in walk(from) {
        let dest = to.join(path.strip_prefix(from).unwrap());
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(&path, dest).unwrap();
    }
}
