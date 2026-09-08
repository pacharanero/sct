// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Issue #131: component identity and argument-order precedence through real RF2 -> NDJSON -> SQLite.

use rusqlite::Connection;
use sct_rs::commands::ndjson::{self, RefsetMode};
use sct_rs::commands::sqlite;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const DESCRIPTION: &str = "Snapshot/Terminology/sct2_Description_Snapshot-en_SYN_20260101.txt";
const RELATIONSHIP: &str = "Snapshot/Terminology/sct2_Relationship_Snapshot_SYN_20260101.txt";
const LANGUAGE: &str = "Snapshot/Refset/Language/der2_cRefset_LanguageSnapshot-en_SYN_20260101.txt";
const MI: &str = "22298006";
const TYPE1: &str = "46635009";

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Derive an override from the committed row, retaining all untouched RF2 columns.
fn changed_row(root: &Path, file: &str, id: &str, changes: &[(usize, &str)]) -> String {
    let text = fs::read_to_string(root.join(file)).unwrap();
    let mut columns: Vec<_> = text
        .lines()
        .find(|line| line.split('\t').next() == Some(id))
        .expect("fixture component exists")
        .split('\t')
        .collect();
    for &(column, value) in changes {
        columns[column] = value;
    }
    columns.join("\t")
}

fn replace_row(root: &Path, file: &str, id: &str, changes: &[(usize, &str)]) {
    let replacement = changed_row(root, file, id, changes);
    let text = fs::read_to_string(root.join(file)).unwrap();
    let lines: Vec<_> = text
        .lines()
        .map(|line| {
            if line.split('\t').next() == Some(id) {
                replacement.as_str()
            } else {
                line
            }
        })
        .collect();
    fs::write(root.join(file), format!("{}\n", lines.join("\n"))).unwrap();
}

fn extension(base: &Path, root: &Path, file: &str, row: &str) {
    let text = fs::read_to_string(base.join(file)).unwrap();
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("{}\n{row}\n", text.lines().next().unwrap())).unwrap();
}

struct Build {
    _dir: tempfile::TempDir,
    lines: Vec<String>,
    records: Vec<Value>,
    db: Connection,
}

impl Build {
    fn record(&self, id: &str) -> &Value {
        self.records.iter().find(|r| r["id"] == id).unwrap()
    }

    fn query(&self, sql: &str) -> Vec<Value> {
        self.db
            .prepare(sql)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect()
    }

    fn assert_same(&self, other: &Self) {
        // Compare actual concept lines, including array order, but not source/timestamp provenance.
        assert_eq!(self.lines.len(), other.lines.len());
        for (expected, actual) in self.lines.iter().zip(&other.lines) {
            assert_eq!(expected, actual);
        }
        for sql in [
            "SELECT json_array(id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
                               parents, children_count, attributes, active) FROM concepts ORDER BY id",
            "SELECT json_array(child_id, parent_id) FROM concept_isa ORDER BY child_id, parent_id",
            "SELECT json_array(source_id, type_id, destination_id, group_num)
             FROM concept_relationships ORDER BY source_id, type_id, destination_id, group_num",
        ] {
            assert_eq!(self.query(sql), other.query(sql), "{sql}");
        }
    }
}

fn build(sources: &[PathBuf], include_inactive: bool) -> Build {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("concepts.ndjson");
    let db = dir.path().join("concepts.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: sources.to_vec(),
        locale: "en-GB".into(),
        output: Some(output.clone()),
        include_inactive,
        refsets: RefsetMode::Simple,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input: output.clone(),
        output: Some(db.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    let lines: Vec<String> = fs::read_to_string(output)
        .unwrap()
        .lines()
        .filter(|line| sct_rs::provenance::try_parse_ndjson_line(line).is_none())
        .map(str::to_owned)
        .collect();
    let result = Build {
        records: lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect(),
        lines,
        db: Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap(),
        _dir: dir,
    };

    // Every assertion on NDJSON below must also hold in the derived database.
    let concepts = result.query(
        "SELECT json_array(id, fsn, preferred_term, json(synonyms), json(parents),
                           children_count, json(attributes)) FROM concepts ORDER BY id",
    );
    let expected: Vec<_> = result
        .records
        .iter()
        .map(|r| {
            json!([
                r["id"],
                r["fsn"],
                r["preferred_term"],
                r["synonyms"],
                r["parents"],
                r["children_count"],
                r["attributes"]
            ])
        })
        .collect();
    assert_eq!(concepts, expected);
    let mut isa = Vec::new();
    let mut relationships = Vec::new();
    for record in &result.records {
        for parent in record["parents"].as_array().unwrap() {
            isa.push(json!([record["id"], parent["id"]]));
        }
        for rel in record["relationships"].as_array().unwrap() {
            relationships.push(json!([
                record["id"],
                rel["type_id"],
                rel["destination_id"],
                rel["group"]
            ]));
        }
    }
    for (sql, mut expected) in [
        ("SELECT json_array(child_id, parent_id) FROM concept_isa", isa),
        ("SELECT json_array(source_id, type_id, destination_id, group_num) FROM concept_relationships", relationships),
    ] {
        let mut actual = result.query(sql);
        actual.sort_by_key(Value::to_string);
        expected.sort_by_key(Value::to_string);
        assert_eq!(actual, expected, "{sql}");
    }
    result
}

#[derive(Clone, Copy)]
enum Family {
    Description,
    Isa,
    Attribute,
    Language,
}

fn prefer_heart_attack(base: &Path) {
    // Give GB a visibly different preference from US, avoiding the any-preferred fallback.
    replace_row(base, LANGUAGE, "9000053", &[(6, "900000000000549004")]);
    replace_row(base, LANGUAGE, "9000055", &[(6, "900000000000548007")]);
}

fn check_layer(family: Family, active: bool) {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("z-base");
    let overlay = dir.path().join("a-extension");
    copy_tree(&fixture(), &base);
    let (file, id, replacement) = match family {
        Family::Description => (
            DESCRIPTION,
            "5000028",
            vec![(7, "Synthetic replacement term")],
        ),
        Family::Isa => (RELATIONSHIP, "7000010", vec![(5, "404684003")]),
        Family::Attribute => (RELATIONSHIP, "7000023", vec![(5, "123037004"), (6, "2")]),
        Family::Language => {
            prefer_heart_attack(&base);
            (LANGUAGE, "9000055", vec![(5, "5000023")])
        }
    };
    // Both timestamp and lexical path order disagree with last-argument-wins.
    let mut changes = vec![(1, "20250101"), (2, if active { "1" } else { "0" })];
    if active {
        changes.extend(replacement);
    }
    extension(
        &base,
        &overlay,
        file,
        &changed_row(&base, file, id, &changes),
    );

    for include_inactive in [false, true] {
        let baseline = build(std::slice::from_ref(&base), include_inactive);
        assert_eq!(
            baseline.record(MI)["fsn"],
            "Myocardial infarction (disorder)"
        );
        assert_eq!(
            baseline.record(TYPE1)["preferred_term"],
            "Type 1 diabetes mellitus"
        );
        assert_eq!(
            baseline.record(TYPE1)["parents"],
            json!([{"id":"73211009", "fsn":"Diabetes mellitus (disorder)"}])
        );
        let sources = [base.clone(), overlay.clone()];
        let layered = build(&sources, include_inactive);
        match family {
            Family::Description => {
                assert_eq!(
                    layered.record(MI)["preferred_term"],
                    "Myocardial infarction"
                );
                assert_eq!(
                    layered.record(MI)["synonyms"],
                    if active {
                        json!(["Synthetic replacement term"])
                    } else {
                        json!([])
                    }
                );
            }
            Family::Isa => {
                assert_eq!(
                    layered.record(TYPE1)["parents"],
                    if active {
                        json!([{"id":"404684003", "fsn":"Clinical finding (finding)"}])
                    } else {
                        json!([])
                    }
                );
                assert_eq!(layered.record("73211009")["children_count"], 1);
                let old_count = baseline.record("404684003")["children_count"]
                    .as_u64()
                    .unwrap();
                assert_eq!(
                    layered.record("404684003")["children_count"],
                    old_count + u64::from(active)
                );
            }
            Family::Attribute => {
                let mi = layered.record(MI);
                let mut expected =
                    vec![json!({"type_id":"116676008", "destination_id":"55641003", "group":1})];
                if active {
                    expected.push(
                        json!({"type_id":"363698007", "destination_id":"123037004", "group":2}),
                    );
                    assert_eq!(
                        mi["attributes"]["finding_site"],
                        json!([{"id":"123037004", "fsn":"Body structure (body structure)"}])
                    );
                } else {
                    assert!(mi["attributes"].get("finding_site").is_none());
                }
                let mut actual = mi["relationships"].as_array().unwrap().clone();
                actual.sort_by_key(Value::to_string);
                expected.sort_by_key(Value::to_string);
                assert_eq!(actual, expected);
            }
            Family::Language => {
                assert_eq!(baseline.record(MI)["preferred_term"], "Heart attack");
                assert_eq!(
                    layered.record(MI)["preferred_term"],
                    "Myocardial infarction"
                );
                assert_eq!(layered.record(MI)["synonyms"], json!(["Heart attack"]));
                assert_eq!(
                    layered.record(TYPE1)["preferred_term"],
                    "Type 1 diabetes mellitus"
                );
            }
        }
        // Reverse the exact same paths: later base must restore its original projection.
        baseline.assert_same(&build(&[overlay.clone(), base.clone()], include_inactive));
        layered.assert_same(&build(&sources, include_inactive));
    }
}

#[test]
fn inactive_description_retracts_old_term() {
    check_layer(Family::Description, false);
}

#[test]
fn active_description_replaces_old_term() {
    check_layer(Family::Description, true);
}

#[test]
fn inactive_isa_retracts_old_parent_and_child_count() {
    check_layer(Family::Isa, false);
}

#[test]
fn active_isa_replaces_old_parent_and_child_count() {
    check_layer(Family::Isa, true);
}

#[test]
fn inactive_attribute_retracts_old_value() {
    check_layer(Family::Attribute, false);
}

#[test]
fn active_attribute_replaces_old_value_and_group() {
    check_layer(Family::Attribute, true);
}

#[test]
fn inactive_language_member_retracts_old_preference() {
    check_layer(Family::Language, false);
}

#[test]
fn active_language_member_replaces_old_component() {
    check_layer(Family::Language, true);
}

#[test]
fn loading_full_base_twice_is_idempotent_and_deterministic() {
    for include_inactive in [false, true] {
        let base = fixture();
        let once = build(std::slice::from_ref(&base), include_inactive);
        assert_eq!(once.records.len(), if include_inactive { 23 } else { 22 });
        assert_eq!(once.record(MI)["preferred_term"], "Myocardial infarction");
        assert_eq!(once.record(MI)["synonyms"], json!(["Heart attack"]));
        assert_eq!(
            once.record(MI)["relationships"].as_array().unwrap().len(),
            2
        );
        assert_eq!(once.record("73211009")["children_count"], 2);
        assert_eq!(
            once.record(TYPE1)["preferred_term"],
            "Type 1 diabetes mellitus"
        );
        let sources = [base.clone(), base];
        let twice = build(&sources, include_inactive);
        once.assert_same(&twice);
        twice.assert_same(&build(&sources, include_inactive));
    }
}

#[test]
fn retiring_one_language_uuid_preserves_another_active_member_of_the_same_pair() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("z-base");
    let overlay = dir.path().join("a-extension");
    copy_tree(&fixture(), &base);
    prefer_heart_attack(&base);
    let retired = "a0000000-0000-4000-8000-000000000001";
    let survivor = "a0000000-0000-4000-8000-000000000002";
    replace_row(&base, LANGUAGE, "9000055", &[(0, retired)]);
    let duplicate = changed_row(&base, LANGUAGE, retired, &[(0, survivor)]);
    let text = fs::read_to_string(base.join(LANGUAGE)).unwrap();
    fs::write(base.join(LANGUAGE), format!("{text}{duplicate}\n")).unwrap();
    let inactive = changed_row(&base, LANGUAGE, retired, &[(1, "20250101"), (2, "0")]);
    extension(&base, &overlay, LANGUAGE, &inactive);
    for include_inactive in [false, true] {
        let sources = [base.clone(), overlay.clone()];
        let layered = build(&sources, include_inactive);
        assert_eq!(layered.record(MI)["preferred_term"], "Heart attack");
        build(std::slice::from_ref(&base), include_inactive).assert_same(&layered);
        layered.assert_same(&build(&sources, include_inactive));
    }
}
