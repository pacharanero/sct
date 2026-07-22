// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! End-to-end ECL tests: build a small SNOMED CT SQLite database through the
//! real `sct sqlite` pipeline (exercising the schema-v4 `concept_relationships`
//! table), then parse + evaluate ECL expressions against it. See `spec/ecl.md`.

use indexmap::IndexMap;
use sct_rs::commands::sqlite;
use sct_rs::ecl;
use sct_rs::schema::{ConceptRecord, ConceptRef, Relationship, SCHEMA_VERSION};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Minimal builder for a concept record. `parents` and `relationships` drive
/// the `concept_isa` and `concept_relationships` tables respectively.
fn rec(
    id: &str,
    pt: &str,
    parents: &[&str],
    refsets: &[&str],
    rels: &[(&str, &str)],
) -> ConceptRecord {
    ConceptRecord {
        id: id.into(),
        fsn: format!("{pt} (finding)"),
        preferred_term: pt.into(),
        synonyms: vec![],
        hierarchy: "Clinical finding".into(),
        hierarchy_path: vec![],
        parents: parents
            .iter()
            .map(|p| ConceptRef {
                id: (*p).into(),
                fsn: String::new(),
            })
            .collect(),
        children_count: 0,
        active: true,
        definition_status: "900000000000074008".into(),
        module: "900000000000207008".into(),
        effective_time: "20260101".into(),
        attributes: IndexMap::new(),
        ctv3_codes: vec![],
        read2_codes: vec![],
        refsets: refsets.iter().map(|s| (*s).into()).collect(),
        relationships: rels
            .iter()
            .map(|(t, d)| Relationship {
                type_id: (*t).into(),
                destination_id: (*d).into(),
                group: 0,
            })
            .collect(),
        crossmaps: vec![],
        schema_version: SCHEMA_VERSION,
    }
}

/// Build the fixture database and return its path (plus the tempdir guard).
fn build_db() -> (PathBuf, tempfile::TempDir) {
    // Hierarchy:
    //   138875005 (root)
    //     └ 404684003 Clinical finding
    //         ├ 73211009 Diabetes mellitus
    //         │   ├ 46635009 Type 1 DM      (refset 447562003 member)
    //         │   └ 44054006 Type 2 DM      (refset 447562003 member)
    //         ├ 22298006 Myocardial infarction  (finding site 363698007 = 74281007)
    //         └ 74281007 Myocardium structure
    //   447562003 (a refset concept)
    const FINDING_SITE: &str = "363698007";
    const ASSOC_MORPH: &str = "116676008";
    let records = vec![
        rec("138875005", "SNOMED CT Concept", &[], &[], &[]),
        rec("447562003", "Example refset", &["138875005"], &[], &[]),
        rec("404684003", "Clinical finding", &["138875005"], &[], &[]),
        rec("73211009", "Diabetes mellitus", &["404684003"], &[], &[]),
        rec(
            "46635009",
            "Type 1 diabetes mellitus",
            &["73211009"],
            &["447562003"],
            &[],
        ),
        rec(
            "44054006",
            "Type 2 diabetes mellitus",
            &["73211009"],
            &["447562003"],
            &[],
        ),
        rec("74281007", "Myocardium structure", &["404684003"], &[], &[]),
        rec("55641003", "Infarct", &["404684003"], &[], &[]),
        rec(
            "22298006",
            "Myocardial infarction",
            &["404684003"],
            &[],
            // finding site = Myocardium, associated morphology = Infarct
            &[(FINDING_SITE, "74281007"), (ASSOC_MORPH, "55641003")],
        ),
    ];

    let dir = tempfile::tempdir().unwrap();
    let ndjson = dir.path().join("fixture.ndjson");
    {
        let mut f = std::fs::File::create(&ndjson).unwrap();
        for r in &records {
            writeln!(f, "{}", serde_json::to_string(r).unwrap()).unwrap();
        }
    }
    let db = dir.path().join("fixture.db");
    sqlite::run(sqlite::Args {
        input: ndjson,
        output: db.clone(),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    (db, dir)
}

fn expand(db: &Path, ecl: &str) -> Vec<String> {
    let mut v = ecl::expand_path(db, ecl).unwrap();
    v.sort();
    v
}

#[test]
fn descendant_or_self() {
    let (db, _d) = build_db();
    assert_eq!(
        expand(&db, "<<73211009"),
        vec!["44054006", "46635009", "73211009"]
    );
}

#[test]
fn descendant_not_self() {
    let (db, _d) = build_db();
    assert_eq!(expand(&db, "<73211009"), vec!["44054006", "46635009"]);
}

#[test]
fn ancestor_or_self() {
    let (db, _d) = build_db();
    assert_eq!(
        expand(&db, ">>46635009"),
        vec!["138875005", "404684003", "46635009", "73211009"]
    );
}

#[test]
fn children_and_parents() {
    let (db, _d) = build_db();
    assert_eq!(expand(&db, "<!73211009"), vec!["44054006", "46635009"]);
    assert_eq!(expand(&db, ">!46635009"), vec!["73211009"]);
}

#[test]
fn member_of_refset() {
    let (db, _d) = build_db();
    assert_eq!(expand(&db, "^447562003"), vec!["44054006", "46635009"]);
}

#[test]
fn boolean_ops() {
    let (db, _d) = build_db();
    assert_eq!(
        expand(&db, "<<73211009 MINUS 46635009"),
        vec!["44054006", "73211009"]
    );
    assert_eq!(
        expand(&db, "46635009 OR 22298006"),
        vec!["22298006", "46635009"]
    );
    assert_eq!(
        expand(&db, "<<73211009 AND <44054006"),
        Vec::<String>::new()
    );
}

#[test]
fn attribute_refinement() {
    let (db, _d) = build_db();
    // Clinical findings whose finding site is (a descendant-or-self of) Myocardium.
    assert_eq!(
        expand(&db, "<<404684003 : 363698007 = <<74281007"),
        vec!["22298006"]
    );
    // Any finding site at all (wildcard value).
    assert_eq!(expand(&db, "<<404684003 : 363698007 = *"), vec!["22298006"]);
    // A finding site that is NOT Myocardium → no concept qualifies here.
    assert!(expand(&db, "<<404684003 : 363698007 != <<74281007").is_empty());
}

#[test]
fn refinement_wildcard_attribute_type() {
    let (db, _d) = build_db();
    // `* = <<74281007`: any attribute whose value is Myocardium-or-descendant.
    assert_eq!(
        expand(&db, "<<404684003 : * = <<74281007"),
        vec!["22298006"]
    );
    // `R`-less wildcard both sides: any concept with any relationship at all.
    assert_eq!(expand(&db, "<<404684003 : * = *"), vec!["22298006"]);
}

#[test]
fn refinement_conjunction_of_two_attribute_types() {
    let (db, _d) = build_db();
    // Both constraints must hold (comma = AND): MI has both.
    assert_eq!(
        expand(
            &db,
            "<<404684003 : 363698007 = 74281007, 116676008 = 55641003"
        ),
        vec!["22298006"]
    );
    // Second constraint points at the wrong value → no match.
    assert!(expand(
        &db,
        "<<404684003 : 363698007 = 74281007, 116676008 = 74281007"
    )
    .is_empty());
}

#[test]
fn refinement_attribute_group_is_a_conjunction_in_v1() {
    let (db, _d) = build_db();
    assert_eq!(
        expand(
            &db,
            "<<404684003 : { 363698007 = 74281007, 116676008 = 55641003 }"
        ),
        vec!["22298006"]
    );
}

#[test]
fn refinement_disjunction() {
    let (db, _d) = build_db();
    // OR between attribute constraints: either site OR a bogus morphology.
    assert_eq!(
        expand(
            &db,
            "<<404684003 : 363698007 = 74281007 OR 116676008 = 99999999"
        ),
        vec!["22298006"]
    );
}

#[test]
fn term_annotations_are_ignored() {
    let (db, _d) = build_db();
    assert_eq!(
        expand(&db, "<<73211009 |Diabetes mellitus|"),
        vec!["44054006", "46635009", "73211009"]
    );
}

#[test]
fn unsupported_ecl_errors_clearly() {
    let (db, _d) = build_db();
    let err = ecl::expand_path(&db, "<<73211009 [0..1]").unwrap_err();
    assert!(
        format!("{err:#}").contains("cardinality"),
        "expected a cardinality error, got: {err:#}"
    );
}
