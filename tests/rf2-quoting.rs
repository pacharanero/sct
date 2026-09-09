// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Issue #137: literal RF2 quotes survive canonical and structured output.

use sct_rs::commands::{ndjson, sqlite};
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

#[test]
fn quoted_terms_survive_rf2_ndjson_sqlite_and_structured_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z");
    let relative = "Snapshot/Terminology/sct2_Description_Snapshot-en_SYN_20260101.txt";
    let original = fs::read_to_string(base.join(relative)).unwrap();
    let extension = dir.path().join("extension");
    let path = extension.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let fsn = "\"Myocardial infarction\" (disorder)";
    let pt = "\"Myocardial infarction\"";
    let synonyms = [
        "Heart attack",
        "\"Heart attack\"",
        "Myocardial infarction",
        "\"Leading quote",
        "Heart \"attack\" term",
        "\"\"Heart attack\"\"",
        "Unbalanced \"embedded quote",
    ];
    let mut rows = vec![original.lines().next().unwrap().to_owned()];
    // Override the committed FSN/PT identities, retaining language preferences.
    // New synonym identities must not collapse into their unquoted equivalents.
    for (id, source_id, term) in [
        ("5000026".to_owned(), "5000026", fsn),
        ("5000027".to_owned(), "5000027", pt),
    ]
    .into_iter()
    .chain(
        synonyms
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, term)| ((9900000 + i).to_string(), "5000028", *term)),
    ) {
        let mut columns: Vec<_> = original
            .lines()
            .find(|line| line.split('\t').next() == Some(source_id))
            .unwrap()
            .split('\t')
            .collect();
        columns[0] = &id;
        columns[1] = "20260201";
        columns[7] = term;
        rows.push(columns.join("\t"));
    }
    fs::write(path, format!("{}\n", rows.join("\n"))).unwrap();
    let canonical = dir.path().join("concepts.ndjson");
    let db = dir.path().join("concepts.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![base, extension],
        locale: "en-GB".into(),
        output: Some(canonical.clone()),
        include_inactive: false,
        refsets: ndjson::RefsetMode::Simple,
    })
    .unwrap();
    let record = fs::read_to_string(&canonical)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|record| record["id"] == "22298006")
        .expect("known MI concept survives the build");
    let assert_terms = |record: &Value| {
        assert_eq!(record["fsn"], fsn);
        assert_eq!(record["preferred_term"], pt);
        let mut actual: Vec<_> = record["synonyms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|term| term.as_str().unwrap())
            .collect();
        let mut expected = synonyms.to_vec();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(
            actual, expected,
            "quoted and ordinary synonyms must stay distinct"
        );
    };
    assert_terms(&record);
    sqlite::run(sqlite::Args {
        input: canonical,
        output: Some(db.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    let conn =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let stored: String = conn.query_row(
        "SELECT json_object('fsn', fsn, 'preferred_term', preferred_term, 'synonyms', json(synonyms)) FROM concepts WHERE id = ?1",
        ["22298006"], |row| row.get(0),
    ).unwrap();
    assert_terms(&serde_json::from_str(&stored).unwrap());
    for format in ["json", "yaml"] {
        let output = Command::new(env!("CARGO_BIN_EXE_sct"))
            .env("SCT_DATA_HOME", dir.path())
            .args(["lookup", "22298006", "--db"])
            .arg(&db)
            .args(["--format", format])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = if format == "json" {
            serde_json::from_slice(&output.stdout).unwrap()
        } else {
            serde_yaml_ng::from_slice(&output.stdout).unwrap()
        };
        assert_terms(&value);
    }
}
