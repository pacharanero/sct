// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Public SDK acceptance tests over the committed synthetic RF2 fixture.

use sct_rs::commands::ndjson::{self, RefsetMode};
use sct_rs::commands::sqlite;
use sct_rs::sdk::{SchemaCompatibility, SctError, SearchOptions, Snomed, Subsumption};
use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn build() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ndjson = dir.path().join("synthetic.ndjson");
    let db = dir.path().join("synthetic.db");

    ndjson::run(ndjson::Args {
        rf2_dirs: vec![fixture_dir()],
        locale: "en-GB".to_string(),
        output: Some(ndjson.clone()),
        include_inactive: false,
        refsets: RefsetMode::Simple,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input: ndjson,
        output: Some(db.clone()),
        transitive_closure: true,
        include_self: false,
    })
    .unwrap();

    (dir, db)
}

fn build_all() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ndjson = dir.path().join("synthetic.ndjson");
    let db = dir.path().join("synthetic.db");

    ndjson::run(ndjson::Args {
        rf2_dirs: vec![fixture_dir()],
        locale: "en-GB".to_string(),
        output: Some(ndjson.clone()),
        include_inactive: false,
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

    (dir, db)
}

#[test]
fn open_is_read_only_and_missing_paths_are_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.db");

    assert!(Snomed::open(&missing).is_err());
    assert!(!missing.exists());
}

#[test]
fn hierarchy_queries_fall_back_cleanly_without_transitive_closure() {
    let (_dir, db) = build_all();
    let snomed = Snomed::open(&db).unwrap();
    assert!(!snomed.has_transitive_closure());

    let ancestors = snomed.ancestors("46635009").unwrap();
    assert!(ancestors.iter().any(|concept| concept.id == "73211009"));
    let descendants = snomed.descendants("73211009", 100).unwrap();
    assert!(descendants.iter().any(|concept| concept.id == "46635009"));
    assert_eq!(
        snomed.subsumes("73211009", "46635009").unwrap(),
        Subsumption::Subsumes
    );
}

#[test]
fn provenance_and_known_concept_are_typed() {
    let (_dir, db) = build();
    let snomed = Snomed::open(&db).unwrap();
    assert_eq!(snomed.schema_compatibility(), SchemaCompatibility::Current);

    let provenance = snomed.provenance().expect("fixture provenance");
    assert_eq!(
        provenance.edition_label,
        "SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z"
    );
    assert_eq!(provenance.release_date, "2026-01-01");
    assert!(provenance
        .content_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| fingerprint.starts_with("sha256:")));
    assert_eq!(
        provenance.release_id,
        "SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z"
    );

    let concept = snomed
        .concept("22298006")
        .unwrap()
        .expect("myocardial infarction");
    assert_eq!(concept.preferred_term, "Myocardial infarction");
    assert_eq!(concept.hierarchy, "Clinical finding");
    assert!(concept.synonyms.iter().any(|term| term == "Heart attack"));
    assert!(concept.ctv3_codes.iter().any(|code| code == "X200"));
    assert_eq!(concept.definition_status, "900000000000074008");
    assert!(snomed.concept("999999999").unwrap().is_none());
}

#[test]
fn newer_database_schemas_fail_closed() {
    let (_dir, db) = build();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE concepts SET schema_version = schema_version + 1",
        [],
    )
    .unwrap();
    drop(conn);

    assert!(matches!(
        Snomed::open(&db),
        Err(SctError::UnsupportedSchema { .. })
    ));
}

#[test]
fn mixed_database_schema_versions_fail_closed() {
    let (_dir, db) = build();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE concepts SET schema_version = schema_version + 1 WHERE id = '22298006'",
        [],
    )
    .unwrap();
    drop(conn);

    assert!(matches!(
        Snomed::open(&db),
        Err(SctError::InconsistentSchema { .. })
    ));
}

#[test]
fn search_matches_terms_synonyms_limits_and_hierarchies() {
    let (_dir, db) = build();
    let snomed = Snomed::open(&db).unwrap();

    let diabetes = snomed.search("diabetes", 10).unwrap();
    let ids: Vec<&str> = diabetes.iter().map(|hit| hit.id.as_str()).collect();
    assert!(ids.contains(&"73211009"));
    assert!(ids.contains(&"46635009"));
    assert!(ids.contains(&"44054006"));

    let heart = snomed.search("heart attack", 1).unwrap();
    assert_eq!(heart.len(), 1);
    assert_eq!(heart[0].id, "22298006");

    let filtered = snomed
        .search_with(SearchOptions::new("diabetes", 10).hierarchy("Clinical finding"))
        .unwrap();
    assert!(!filtered.is_empty());
    assert!(filtered
        .iter()
        .all(|hit| hit.hierarchy == "Clinical finding"));

    let fts_expression = snomed.search("heart AND attack", 10).unwrap();
    assert!(fts_expression.iter().any(|hit| hit.id == "22298006"));
    assert!(snomed
        .search_with(SearchOptions::new("heart AND attack", 10).literal())
        .unwrap()
        .is_empty());
    assert!(snomed
        .search_with(SearchOptions::new(r#"unmatched \" quote"#, 10).literal())
        .is_ok());
}

#[test]
fn hierarchy_subsumption_and_ecl_share_the_existing_engine() {
    let (_dir, db) = build();
    let snomed = Snomed::open(&db).unwrap();

    assert!(snomed.has_transitive_closure());
    let children = snomed.children("73211009", 10).unwrap();
    assert_eq!(
        children
            .iter()
            .map(|child| child.id.as_str())
            .collect::<Vec<_>>(),
        ["46635009", "44054006"]
    );

    let ancestors = snomed.ancestors("46635009").unwrap();
    assert_eq!(
        ancestors
            .iter()
            .map(|ancestor| ancestor.id.as_str())
            .collect::<Vec<_>>(),
        ["73211009", "404684003", "138875005"]
    );

    let descendants = snomed.descendants("73211009", 10).unwrap();
    assert_eq!(
        descendants
            .iter()
            .map(|child| child.id.as_str())
            .collect::<Vec<_>>(),
        ["46635009", "44054006"]
    );
    assert_eq!(snomed.descendants("73211009", 1).unwrap()[0].id, "46635009");

    assert_eq!(
        snomed.subsumes("73211009", "46635009").unwrap(),
        Subsumption::Subsumes
    );
    assert_eq!(
        snomed.subsumes("46635009", "73211009").unwrap(),
        Subsumption::SubsumedBy
    );
    assert_eq!(
        snomed.subsumes("46635009", "46635009").unwrap(),
        Subsumption::Equivalent
    );
    assert!(matches!(
        snomed.subsumes("999999999", "999999999"),
        Err(SctError::ConceptNotFound { .. })
    ));
    assert!(matches!(
        snomed.subsumes("not-an-sctid", "46635009"),
        Err(SctError::InvalidSctid { .. })
    ));
    assert_eq!(
        snomed.expand("<<73211009").unwrap(),
        ["44054006", "46635009", "73211009"]
    );
}

#[test]
fn transitive_closure_capability_tracks_live_database_status() {
    let (_dir, db) = build();
    let snomed = Snomed::open(&db).unwrap();
    assert!(snomed.transitive_closure_usable().unwrap());

    rusqlite::Connection::open(&db)
        .unwrap()
        .execute("DELETE FROM concept_ancestors_meta", [])
        .unwrap();

    assert!(!snomed.has_transitive_closure());
    assert!(!snomed.transitive_closure_usable().unwrap());
}

#[test]
fn descendants_apply_the_limit_without_a_transitive_closure_table() {
    let (_dir, db) = build_all();
    let snomed = Snomed::open(&db).unwrap();
    assert!(!snomed.has_transitive_closure());

    let descendants = snomed.descendants("73211009", 1).unwrap();
    assert_eq!(descendants.len(), 1);
    assert_eq!(descendants[0].id, "46635009");
}

#[test]
fn refset_queries_return_typed_members_comparisons_and_profiles() {
    let (_dir, db) = build();
    let snomed = Snomed::open(&db).unwrap();
    let refset_id = "991381000000107";

    let refsets = snomed.refsets().unwrap();
    assert_eq!(refsets.len(), 1);
    assert_eq!(refsets[0].id, refset_id);
    assert_eq!(refsets[0].member_count, 2);

    let summary = snomed.refset(refset_id).unwrap().expect("refset concept");
    assert_eq!(summary.preferred_term, "Example clinical reference set");

    let members = snomed.refset_members(refset_id, Some(10)).unwrap();
    assert_eq!(members.len(), 2);
    assert!(members.iter().any(|member| member.id == "46635009"));
    assert!(members.iter().any(|member| member.id == "44054006"));

    let comparison = snomed
        .refset_compare(refset_id, refset_id, Some(10))
        .unwrap();
    assert_eq!(comparison.only_in_a.count, 0);
    assert_eq!(comparison.only_in_b.count, 0);
    assert_eq!(comparison.in_both.count, 2);

    let profile = snomed.refset_profile(refset_id).unwrap();
    assert_eq!(profile.len(), 1);
    assert_eq!(profile[0].hierarchy, "Clinical finding");
    assert_eq!(profile[0].count, 2);
}

#[test]
fn refset_query_errors_are_not_reported_as_missing_concepts() {
    let (_dir, db) = build();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute("DROP TABLE refset_members", []).unwrap();
    drop(conn);

    let snomed = Snomed::open(&db).unwrap();
    assert!(matches!(
        snomed.refset("991381000000107"),
        Err(SctError::Query { .. })
    ));
}

#[test]
fn mapping_and_history_cover_classifications_and_inactive_concepts() {
    use sct_rs::sdk::Terminology;

    let (_dir, db) = build_all();
    let snomed = Snomed::open(&db).unwrap();

    let icd10 = snomed
        .map(Terminology::Snomed, "22298006", Terminology::Icd10)
        .unwrap();
    assert_eq!(icd10.len(), 1);
    assert_eq!(icd10[0].target, "I219");
    assert_eq!(icd10[0].display.as_deref(), Some("Myocardial infarction"));

    let reverse = snomed
        .map(Terminology::Icd10, "I21.9", Terminology::Snomed)
        .unwrap();
    assert_eq!(reverse[0].target, "22298006");

    let history = snomed.history("9468002").unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().any(|item| item.target == "22298006"));
    assert!(history.iter().any(|item| item.target == "195967001"));

    let forwarded = snomed
        .map_forwarding_history(Terminology::Snomed, "9468002", Terminology::Snomed)
        .unwrap();
    assert_eq!(
        forwarded
            .iter()
            .map(|mapping| mapping.target.as_str())
            .collect::<Vec<_>>(),
        ["22298006", "195967001"]
    );
}

#[test]
fn attached_fst_exposes_typed_exact_prefix_fuzzy_word_and_typeahead_hits() {
    let (dir, db) = build();
    let fst = dir.path().join("synthetic.fst");
    let input =
        std::io::BufReader::new(std::fs::File::open(dir.path().join("synthetic.ndjson")).unwrap());
    sct_rs::index::build(input, &mut std::fs::File::create(&fst).unwrap()).unwrap();

    let mut snomed = Snomed::open(&db).unwrap();
    assert!(!snomed.has_fst());
    assert!(snomed.autocomplete("myoc", 10, false).is_err());
    snomed.attach_fst(&fst).unwrap();
    assert!(snomed.has_fst());

    let exact = snomed.fst_exact("heart attack").unwrap();
    assert_eq!(exact[0].id, "22298006");
    assert_eq!(exact[0].display, "Myocardial infarction");

    assert!(snomed
        .fst_prefix("myoc", 10)
        .unwrap()
        .iter()
        .any(|hit| hit.id == "22298006"));
    assert!(snomed
        .fst_fuzzy("myocardial infarcton", 1, 10)
        .unwrap()
        .iter()
        .any(|hit| hit.id == "22298006"));
    assert!(snomed
        .fst_words(&["heart", "attack"], 10)
        .unwrap()
        .iter()
        .any(|hit| hit.id == "22298006"));
    assert!(snomed
        .autocomplete("myoc", 10, true)
        .unwrap()
        .iter()
        .any(|hit| hit.id == "22298006"));

    let ndjson = std::fs::read_to_string(dir.path().join("synthetic.ndjson")).unwrap();
    let without_provenance = ndjson.lines().skip(1).collect::<Vec<_>>().join("\n");
    let unprovenanced_fst = dir.path().join("unprovenanced.fst");
    sct_rs::index::build(
        std::io::Cursor::new(without_provenance),
        &mut std::fs::File::create(&unprovenanced_fst).unwrap(),
    )
    .unwrap();
    let mut unprovenanced = Snomed::open(&db).unwrap();
    assert!(matches!(
        unprovenanced.attach_fst(&unprovenanced_fst),
        Err(SctError::IndexProvenanceMissing { .. })
    ));

    let mut lines = ndjson.lines();
    let mut provenance: serde_json::Value =
        serde_json::from_str(lines.next().expect("provenance line")).unwrap();
    provenance["release_id"] = serde_json::json!("different-release");
    let mismatched_ndjson = std::iter::once(serde_json::to_string(&provenance).unwrap())
        .chain(lines.map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n");
    let mismatched_fst = dir.path().join("mismatched.fst");
    sct_rs::index::build(
        std::io::Cursor::new(mismatched_ndjson),
        &mut std::fs::File::create(&mismatched_fst).unwrap(),
    )
    .unwrap();
    let mut mismatched = Snomed::open(&db).unwrap();
    assert!(matches!(
        mismatched.attach_fst(&mismatched_fst),
        Err(SctError::IndexProvenanceMismatch { .. })
    ));

    let mut lines = ndjson.lines();
    let mut provenance: serde_json::Value =
        serde_json::from_str(lines.next().expect("provenance line")).unwrap();
    provenance
        .as_object_mut()
        .unwrap()
        .remove("content_fingerprint");
    let mut records = lines.map(str::to_string).collect::<Vec<_>>();
    records.pop();
    let different_content = std::iter::once(serde_json::to_string(&provenance).unwrap())
        .chain(records)
        .collect::<Vec<_>>()
        .join("\n");
    let different_content_fst = dir.path().join("different-content.fst");
    sct_rs::index::build(
        std::io::Cursor::new(different_content),
        &mut std::fs::File::create(&different_content_fst).unwrap(),
    )
    .unwrap();
    let mut different_content = Snomed::open(&db).unwrap();
    assert!(matches!(
        different_content.attach_fst(&different_content_fst),
        Err(SctError::IndexContentMismatch { .. })
    ));
}

#[test]
fn codelist_text_parses_and_renders_without_a_database() {
    let text = "---\n\
id: example\n\
title: Example\n\
description: Synthetic list\n\
terminology: SNOMED CT\n\
created: 2026-01-01\n\
updated: 2026-01-01\n\
version: 1\n\
status: draft\n\
licence: CC-BY-4.0\n\
copyright: Example\n\
appropriate_use: Testing\n\
misuse: Production\n\
---\n\n\
22298006 Myocardial infarction\n\
# 195967001 Asthma\n";

    let codelist = sct_rs::sdk::parse_codelist(text).unwrap();
    assert_eq!(codelist.front_matter.id, "example");
    assert_eq!(codelist.body[0].sctid(), Some("22298006"));
    assert_eq!(codelist.body[1].sctid(), Some("195967001"));

    let rendered = sct_rs::sdk::render_codelist(&codelist).unwrap();
    let reparsed = sct_rs::sdk::parse_codelist(&rendered).unwrap();
    assert_eq!(reparsed.front_matter.title, "Example");
    assert_eq!(reparsed.body.len(), 2);
}

#[test]
fn codelist_composition_rejects_url_includes_offline() {
    let text = "---\n\
id: example\n\
title: Example\n\
description: Synthetic list\n\
terminology: SNOMED CT\n\
created: 2026-01-01\n\
updated: 2026-01-01\n\
version: 1\n\
status: draft\n\
licence: CC-BY-4.0\n\
copyright: Example\n\
appropriate_use: Testing\n\
misuse: Production\n\
includes:\n\
  - https://example.invalid/remote.codelist\n\
---\n";
    let codelist = sct_rs::sdk::parse_codelist(text).unwrap();
    let dir = tempfile::tempdir().unwrap();

    let error =
        sct_rs::sdk::effective_members_of(&codelist, dir.path().join("root.codelist"), dir.path())
            .unwrap_err();
    assert!(format!("{error:?}").contains("URL includes are unavailable"));
}

/// R26: proximal primitive supertypes, validated against the committed
/// synthetic RF2 fixture and known concepts rather than a hand-built schema.
///
/// Every concept in the fixture is primitive, so the fixture alone cannot
/// exercise the pruning path. The definition statuses are therefore flipped
/// in place here to build the fully-defined cases, against the *real* schema
/// produced by `sct sqlite`.
#[test]
fn proximal_primitive_supertypes_resolve_against_the_fixture_hierarchy() {
    let (_dir, db) = build();

    // Fixture IS-A chain: 46635009 -> 73211009 -> 404684003 -> 138875005.
    // All fixture concepts are primitive, so each is its own proximal
    // primitive supertype.
    let snomed = Snomed::open(&db).unwrap();
    for id in ["22298006", "46635009", "73211009", "138875005"] {
        let result = snomed.proximal_primitive_supertypes(id).unwrap();
        assert_eq!(
            result.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec![id],
            "a primitive concept is its own proximal primitive supertype"
        );
    }
    drop(snomed);

    // Mark Type 1 diabetes mellitus fully-defined: its proximal primitive
    // supertype becomes its nearest primitive ancestor, Diabetes mellitus.
    set_defined(&db, &["46635009"]);
    let snomed = Snomed::open(&db).unwrap();
    let result = snomed.proximal_primitive_supertypes("46635009").unwrap();
    assert_eq!(
        result.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        vec!["73211009"],
        "46635009 (Type 1 diabetes mellitus) -> 73211009 (Diabetes mellitus)"
    );
    drop(snomed);

    // Mark Diabetes mellitus fully-defined too: the walk must skip it and
    // prune up to Clinical finding.
    set_defined(&db, &["73211009"]);
    let snomed = Snomed::open(&db).unwrap();
    let result = snomed.proximal_primitive_supertypes("46635009").unwrap();
    assert_eq!(
        result.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        vec!["404684003"],
        "both diabetes concepts defined -> 404684003 (Clinical finding)"
    );
}

/// A migrated database can leave `definition_status` NULL. That must produce
/// the actionable "data may be incomplete" error, not a raw column-type error.
#[test]
fn proximal_primitive_supertypes_report_null_definition_status_cleanly() {
    let (_dir, db) = build();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute("UPDATE concepts SET definition_status = NULL", [])
        .unwrap();

    let snomed = Snomed::open(&db).unwrap();
    let error = snomed
        .proximal_primitive_supertypes("46635009")
        .unwrap_err();
    let rendered = format!("{error:?}");
    assert!(
        rendered.contains("may be incomplete"),
        "expected an actionable error, got: {rendered}"
    );
    assert!(
        !rendered.contains("Invalid column type"),
        "raw rusqlite column-type error leaked to the caller: {rendered}"
    );
}

/// Flip concepts to fully-defined (`900000000000073002`) in an existing
/// database, so the fixture's all-primitive hierarchy can exercise pruning.
fn set_defined(db: &std::path::Path, ids: &[&str]) {
    let conn = rusqlite::Connection::open(db).unwrap();
    for id in ids {
        let updated = conn
            .execute(
                "UPDATE concepts SET definition_status = '900000000000073002' WHERE id = ?1",
                [id],
            )
            .unwrap();
        assert_eq!(updated, 1, "expected {id} in the fixture database");
    }
}

/// Build a database that keeps inactive concepts and loads the payload
/// refsets, which is what the inactive-concept story (R11) needs: the
/// inactivation indicator lives in an AttributeValue refset and the
/// replacements in an Association refset, and neither is loaded by the
/// default `--refsets simple` build.
fn build_with_inactive() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ndjson = dir.path().join("synthetic.ndjson");
    let db = dir.path().join("synthetic.db");

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
        transitive_closure: true,
        include_self: false,
    })
    .unwrap();

    (dir, db)
}

/// R11: an inactive concept reports why it was retired and what replaces it.
#[test]
fn inactive_concept_reports_its_reason_and_replacements() {
    let (_dir, db) = build_with_inactive();
    let snomed = Snomed::open(&db).unwrap();

    let concept = snomed.concept("9468002").unwrap().expect("in fixture");
    assert!(!concept.active);

    // 900000000000482003 is not itself in the fixture's concept file, so the
    // label has to come from the built-in table of standard values rather than
    // a join - otherwise the reader gets a bare SCTID.
    let reason = concept
        .inactivation_reason
        .expect("fixture records a reason");
    assert_eq!(reason.id, "900000000000482003");
    assert_eq!(reason.label, "Duplicate");

    let mut associations: Vec<(String, String, String)> = concept
        .historical_associations
        .iter()
        .map(|a| {
            (
                a.association.clone(),
                a.target.clone(),
                a.target_display.clone().unwrap_or_default(),
            )
        })
        .collect();
    associations.sort();
    assert_eq!(
        associations,
        vec![
            (
                "replaced_by".to_string(),
                "22298006".to_string(),
                "Myocardial infarction".to_string()
            ),
            (
                "same_as".to_string(),
                "195967001".to_string(),
                "Asthma".to_string()
            ),
        ],
        "both associations, with the replacement's term resolved"
    );
}

/// The fixture holds a *superseded* (`active = 0`) inactivation-indicator row
/// for Asthma, which is itself an active concept. Treating a retired indicator
/// row as current would report a live clinical code as retired - the most
/// dangerous possible direction for this feature to fail in.
#[test]
fn an_active_concept_is_never_reported_as_inactivated() {
    let (_dir, db) = build_with_inactive();
    let snomed = Snomed::open(&db).unwrap();

    let concept = snomed.concept("195967001").unwrap().expect("in fixture");
    assert!(concept.active, "Asthma is active in the fixture");
    assert_eq!(concept.inactivation_reason, None);
    assert!(concept.historical_associations.is_empty());
}

/// A database built before payload refsets were ingested has no
/// `attribute_value_refset_members` table at all. That must degrade to "reason
/// unknown" rather than failing the whole lookup.
#[test]
fn inactivation_reason_degrades_on_a_database_without_payload_refsets() {
    let (_dir, db) = build_with_inactive();
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE attribute_value_refset_members;")
        .unwrap();

    let snomed = Snomed::open(&db).unwrap();
    let concept = snomed.concept("9468002").unwrap().expect("in fixture");
    assert_eq!(
        concept.inactivation_reason, None,
        "no indicator table means no reason, not an error"
    );
    assert_eq!(
        concept.historical_associations.len(),
        2,
        "associations live in a different table and are unaffected"
    );
}
