// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Integration tests for the RF2 parsing layer.
//!
//! These tests exercise the public `sct_rs::rf2` API: the row-level parsers
//! and the aggregated `Rf2Dataset::load`. They write tiny, hand-crafted TSV
//! fixtures to temp files to keep the tests hermetic.

use std::io::Write;
use tempfile::NamedTempFile;

use sct_rs::rf2::{
    parse_attribute_value, parse_complex_map, parse_concepts, parse_descriptions,
    parse_extended_map, parse_lang_refset, parse_relationships, parse_simple_map,
    parse_simple_refset, Acceptability, Rf2Dataset, Rf2Files, IS_A, PREFERRED, TYPE_FSN,
};

fn tsv_file(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

const QUOTE_VALUES: &[&str] = &[
    "\"Heart attack\"",
    "\"Leading quote",
    "Trailing quote\"",
    "Heart \"attack\" term",
    "\"\"Heart attack\"\"",
    "Unbalanced \"embedded quote",
];
const DESCRIPTION_HEADER: &str = "id\teffectiveTime\tactive\tmoduleId\tconceptId\tlanguageCode\ttypeId\tterm\tcaseSignificanceId\n";

#[test]
fn description_quotes_are_literal_rf2_data() {
    for term in QUOTE_VALUES {
        let f = tsv_file(&format!(
            "{DESCRIPTION_HEADER}999001\t20260101\t1\t0\t22298006\ten\t900000000000013009\t{term}\t0\n"
        ));
        let rows =
            parse_descriptions(f.path()).unwrap_or_else(|error| panic!("term {term:?}: {error:#}"));
        assert_eq!(rows.len(), 1);
        assert_eq!(&rows[0].term, term);
        assert_eq!(rows[0].concept_id, "22298006");
    }
}

#[test]
fn simple_map_quotes_are_literal_rf2_data() {
    for target in QUOTE_VALUES {
        let f = tsv_file(&format!(
            "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tmapTarget\n\
             member\t20260101\t1\t0\t900000000000497000\t22298006\t{target}\n"
        ));
        let rows = parse_simple_map(f.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(&rows[0].map_target, target);
    }
}

#[test]
fn payload_map_quotes_are_literal_in_all_formats() {
    // Representative coverage of checked-header and ExtendedMap reader families.
    let header = "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tmapGroup\tmapPriority\tmapRule\tmapAdvice\tmapTarget\tcorrelationId";
    for (tail_header, tail_value) in [
        ("", ""),
        ("\tmapCategoryId", "\t447637006"),
        ("\tmapBlock", "\t4"),
    ] {
        for value in QUOTE_VALUES {
            for field in 0..3 {
                let mut payload = ["TRUE", "CHECK TARGET", "I219"];
                payload[field] = value;
                let f = tsv_file(&format!(
                    "{header}{tail_header}\nmember\t20260101\t1\t0\t991401000000101\t22298006\t2\t3\t{}\t{}\t{}\t447561005{tail_value}\n",
                    payload[0], payload[1], payload[2]
                ));
                let actual = if tail_header.is_empty() {
                    let rows = parse_complex_map(f.path()).unwrap();
                    assert_eq!(rows.len(), 1);
                    let row = &rows[0];
                    assert_eq!((row.map_group, row.map_priority), (2, 3));
                    [
                        row.map_rule.clone(),
                        row.map_advice.clone(),
                        row.map_target.clone(),
                    ]
                } else {
                    let rows = parse_extended_map(f.path()).unwrap();
                    assert_eq!(rows.len(), 1);
                    let row = &rows[0];
                    assert_eq!((row.map_group, row.map_priority), (2, 3));
                    assert_eq!(row.map_block, (tail_header == "\tmapBlock").then_some(4));
                    assert_eq!(
                        row.map_category_id.as_deref(),
                        (tail_header == "\tmapCategoryId").then_some("447637006")
                    );
                    [
                        row.map_rule.clone(),
                        row.map_advice.clone(),
                        row.map_target.clone(),
                    ]
                };
                assert_eq!(
                    actual, payload,
                    "format {tail_header:?}, field {field}, value {value:?}"
                );
            }
        }
    }
}

#[test]
fn quoted_multiline_cannot_join_physical_rf2_records() {
    // CSV sees one valid nine-column row; RF2 sees a truncated record followed
    // by another physical record, not a legal multiline description field.
    let f = tsv_file(&format!(
        "{DESCRIPTION_HEADER}999001\t20260101\t1\t0\t22298006\ten\t900000000000013009\t\"broken\n\
         999002\t20260101\t1\t0\t22298006\ten\t900000000000013009\tcontinued\"\t0\n"
    ));
    assert!(
        parse_descriptions(f.path()).is_err(),
        "RF2 physical records must not be joined by CSV quoting"
    );
    let files = Rf2Files {
        description_files: vec![f.path().into()],
        ..Rf2Files::default()
    };
    assert!(Rf2Dataset::load(&files, false).is_err());
}

// --- Concept parsing ---

#[test]
fn parse_concepts_empty() {
    let f = tsv_file("id\teffectiveTime\tactive\tmoduleId\tdefinitionStatusId\n");
    let rows = parse_concepts(f.path()).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn parse_concepts_active_row() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tdefinitionStatusId\n\
         138875005\t20020131\t1\t900000000000207008\t900000000000074008\n",
    );
    let rows = parse_concepts(f.path()).unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.id, "138875005");
    assert_eq!(r.effective_time, "20020131");
    assert!(r.active);
    assert_eq!(r.module_id, "900000000000207008");
}

#[test]
fn parse_concepts_inactive_row() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tdefinitionStatusId\n\
         123456789\t20020131\t0\t900000000000207008\t900000000000074008\n",
    );
    let rows = parse_concepts(f.path()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].active);
}

// --- Description parsing ---

#[test]
fn parse_descriptions_fsn_row() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tconceptId\tlanguageCode\ttypeId\tterm\tcaseSignificanceId\n\
         999001\t20020131\t1\t900000000000207008\t138875005\ten\t900000000000003001\tSNOMED CT Concept (SNOMED RT+CTV3)\t900000000000020002\n",
    );
    let rows = parse_descriptions(f.path()).unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.concept_id, "138875005");
    assert_eq!(r.language_code, "en");
    assert_eq!(r.type_id, TYPE_FSN);
    assert_eq!(r.term, "SNOMED CT Concept (SNOMED RT+CTV3)");
}

// --- Relationship parsing ---

#[test]
fn parse_relationships_is_a() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tsourceId\tdestinationId\trelationshipGroup\ttypeId\tcharacteristicTypeId\tmodifierId\n\
         100\t20020131\t1\t900000000000207008\t22298006\t414795007\t0\t116680003\t900000000000011006\t900000000000451002\n",
    );
    let rows = parse_relationships(f.path()).unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.source_id, "22298006");
    assert_eq!(r.destination_id, "414795007");
    assert_eq!(r.type_id, IS_A);
    assert!(r.active);
}

// --- Lang refset parsing ---

#[test]
fn parse_lang_refset_preferred() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tacceptabilityId\n\
         aaa\t20020131\t1\t900000000000207008\t900000000000508004\t999001\t900000000000548007\n",
    );
    let rows = parse_lang_refset(f.path()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].active);
    assert_eq!(rows[0].refset_id, "900000000000508004");
    assert_eq!(rows[0].referenced_component_id, "999001");
    assert_eq!(rows[0].acceptability_id, PREFERRED);
}

// --- Simple map parsing ---

#[test]
fn parse_simple_map_active_row() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tmapTarget\n\
         uuid1\t20200101\t1\t900000000000207008\t900000000000497000\t22298006\tX76Hb\n\
         uuid2\t20200101\t0\t900000000000207008\t900000000000497000\t22298006\tOLD00\n",
    );
    let rows = parse_simple_map(f.path()).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].active);
    assert_eq!(rows[0].refset_id, "900000000000497000");
    assert_eq!(rows[0].referenced_component_id, "22298006");
    assert_eq!(rows[0].map_target, "X76Hb");
    assert!(!rows[1].active);
}

#[test]
fn parse_complex_map_preserves_member_envelope_and_payload() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tmapGroup\tmapPriority\tmapRule\tmapAdvice\tmapTarget\tcorrelationId\n\
         10000000-0000-4000-8000-000000000001\t20260101\t0\t900000000000207008\t991401000000101\t22298006\t2\t3\tIFA 246075003\tCHECK TARGET\tLEGACY-01\t447561005\n",
    );
    let rows = parse_complex_map(f.path()).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, "10000000-0000-4000-8000-000000000001");
    assert_eq!(row.effective_time, "20260101");
    assert!(!row.active);
    assert_eq!(row.module_id, "900000000000207008");
    assert_eq!(row.refset_id, "991401000000101");
    assert_eq!(row.referenced_component_id, "22298006");
    assert_eq!(row.map_group, 2);
    assert_eq!(row.map_priority, 3);
    assert_eq!(row.map_rule, "IFA 246075003");
    assert_eq!(row.map_advice, "CHECK TARGET");
    assert_eq!(row.map_target, "LEGACY-01");
    assert_eq!(row.correlation_id, "447561005");
}

#[test]
fn parse_attribute_value_preserves_inactive_member() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tvalueId\n\
         20000000-0000-4000-8000-000000000001\t20260101\t0\t900000000000207008\t900000000000489007\t9468002\t900000000000482003\n",
    );
    let rows = parse_attribute_value(f.path()).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert!(!row.active);
    assert_eq!(row.referenced_component_id, "9468002");
    assert_eq!(row.value_id, "900000000000482003");
}

#[test]
fn parse_extended_map_preserves_null_target_and_map_block() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tmapGroup\tmapPriority\tmapRule\tmapAdvice\tmapTarget\tcorrelationId\tmapBlock\n\
         30000000-0000-4000-8000-000000000001\t20260101\t1\t900000000000207008\t999002271000000101\t195967001\t1\t1\t\tNULL MAP\t\t447561005\t4\n",
    );
    let rows = parse_extended_map(f.path()).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].map_target.is_empty());
    assert_eq!(rows[0].map_block, Some(4));
    assert_eq!(rows[0].map_category_id, None);
}

#[test]
fn payload_map_parsers_reject_malformed_numbers() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tmapGroup\tmapPriority\tmapRule\tmapAdvice\tmapTarget\tcorrelationId\n\
         uuid\t20260101\t1\t900000000000207008\t991401000000101\t22298006\tnot-a-number\t1\t\t\tX\t447561005\n",
    );
    let error = parse_complex_map(f.path()).unwrap_err().to_string();
    assert!(error.contains("parsing"), "unexpected error: {error}");
}

#[test]
fn layered_extended_map_member_uses_latest_snapshot_row() {
    let header = "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tmapGroup\tmapPriority\tmapRule\tmapAdvice\tmapTarget\tcorrelationId\tmapBlock\n";
    let first = tsv_file(&format!(
        "{header}member-1\t20250101\t1\t900000000000207008\t999002271000000101\t22298006\t1\t1\t\tOLD\tI210\t447561005\t1\n"
    ));
    let second = tsv_file(&format!(
        "{header}member-1\t20260101\t0\t900000000000207008\t999002271000000101\t22298006\t1\t1\t\tRETIRED\tI211\t447561005\t1\n"
    ));
    let files = Rf2Files {
        extended_map_files: vec![first.path().to_path_buf(), second.path().to_path_buf()],
        ..Rf2Files::default()
    };

    let dataset = Rf2Dataset::load(&files, false).unwrap();
    assert_eq!(dataset.extended_map_members.len(), 1);
    assert!(!dataset.extended_map_members[0].active);
    assert_eq!(dataset.extended_map_members[0].effective_time, "20260101");
    assert_eq!(dataset.extended_map_members[0].map_target, "I211");
    assert!(
        dataset.extended_maps.is_empty(),
        "an inactive latest member must retract the earlier active query projection"
    );
}

#[test]
fn layered_inactive_rows_retract_existing_projections() {
    let concepts_base = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tdefinitionStatusId\n\
         404684003\t20250101\t1\t900000000000207008\t900000000000074008\n\
         9468002\t20250101\t1\t900000000000207008\t900000000000074008\n",
    );
    let concepts_extension = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tdefinitionStatusId\n\
         9468002\t20260101\t0\t900000000000207008\t900000000000074008\n",
    );
    let description_header = "id\teffectiveTime\tactive\tmoduleId\tconceptId\tlanguageCode\ttypeId\tterm\tcaseSignificanceId\n";
    let description_base = tsv_file(&format!(
        "{description_header}100001\t20250101\t1\t0\t404684003\ten\t900000000000013009\tClinical finding\t0\n"
    ));
    let description_extension = tsv_file(&format!(
        "{description_header}100001\t20260101\t0\t0\t404684003\ten\t900000000000013009\tClinical finding\t0\n"
    ));
    let relationship_header = "id\teffectiveTime\tactive\tmoduleId\tsourceId\tdestinationId\trelationshipGroup\ttypeId\tcharacteristicTypeId\tmodifierId\n";
    let relationship_base = tsv_file(&format!(
        "{relationship_header}200001\t20250101\t1\t0\t404684003\t138875005\t0\t116680003\t0\t0\n"
    ));
    let relationship_extension = tsv_file(&format!(
        "{relationship_header}200001\t20260101\t0\t0\t404684003\t138875005\t0\t116680003\t0\t0\n"
    ));
    let language_header =
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tacceptabilityId\n";
    let language_base = tsv_file(&format!(
        "{language_header}language-1\t20250101\t1\t0\t900000000000508004\t100001\t900000000000548007\n"
    ));
    let language_extension = tsv_file(&format!(
        "{language_header}language-1\t20260101\t0\t0\t900000000000508004\t100001\t900000000000548007\n"
    ));
    let simple_map_header =
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tmapTarget\n";
    let simple_map_base = tsv_file(&format!(
        "{simple_map_header}map-1\t20250101\t1\t900000000000207008\t900000000000497000\t404684003\tX100\n"
    ));
    let simple_map_extension = tsv_file(&format!(
        "{simple_map_header}map-1\t20260101\t0\t900000000000207008\t900000000000497000\t404684003\tX100\n"
    ));
    let simple_header = "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\n";
    let simple_base = tsv_file(&format!(
        "{simple_header}simple-1\t20250101\t1\t900000000000207008\t991381000000107\t404684003\n"
    ));
    let simple_extension = tsv_file(&format!(
        "{simple_header}simple-1\t20260101\t0\t900000000000207008\t991381000000107\t404684003\n"
    ));
    let association_header =
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\ttargetComponentId\n";
    let association_base = tsv_file(&format!(
        "{association_header}association-1\t20250101\t1\t900000000000207008\t900000000000526001\t9468002\t404684003\n"
    ));
    let association_extension = tsv_file(&format!(
        "{association_header}association-1\t20260101\t0\t900000000000207008\t900000000000526001\t9468002\t404684003\n"
    ));

    let files = Rf2Files {
        concept_files: vec![
            concepts_base.path().to_path_buf(),
            concepts_extension.path().to_path_buf(),
        ],
        description_files: vec![
            description_base.path().to_path_buf(),
            description_extension.path().to_path_buf(),
        ],
        relationship_files: vec![
            relationship_base.path().to_path_buf(),
            relationship_extension.path().to_path_buf(),
        ],
        lang_refset_files: vec![
            language_base.path().to_path_buf(),
            language_extension.path().to_path_buf(),
        ],
        simple_map_files: vec![
            simple_map_base.path().to_path_buf(),
            simple_map_extension.path().to_path_buf(),
        ],
        refset_files: vec![
            simple_base.path().to_path_buf(),
            simple_extension.path().to_path_buf(),
        ],
        association_files: vec![
            association_base.path().to_path_buf(),
            association_extension.path().to_path_buf(),
        ],
        ..Rf2Files::default()
    };

    let dataset = Rf2Dataset::load(&files, false).unwrap();
    assert!(dataset.concepts.contains_key("404684003"));
    assert!(!dataset.concepts.contains_key("9468002"));
    assert!(dataset.ctv3_maps.is_empty());
    assert!(dataset.refset_members.is_empty());
    assert!(dataset.history.is_empty());
    assert!(dataset.descriptions.is_empty());
    assert!(dataset.parents.is_empty());
    assert!(dataset.acceptability.is_empty());
}

#[test]
fn layered_nonprojectable_map_and_history_rows_retract_older_members() {
    let map_header =
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tmapTarget\n";
    let association_header =
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\ttargetComponentId\n";
    let map_base = tsv_file(&format!(
        "{map_header}map-1\t20250101\t1\t0\t900000000000497000\t22298006\tX200\n"
    ));
    let association_base = tsv_file(&format!(
        "{association_header}history-1\t20250101\t1\t0\t900000000000526001\t9468002\t22298006\n"
    ));
    for active in ["0", "1"] {
        let map_empty = tsv_file(&format!(
            "{map_header}map-1\t20260101\t{active}\t0\t900000000000497000\t22298006\t\n"
        ));
        let association_empty = tsv_file(&format!(
            "{association_header}history-1\t20260101\t{active}\t0\t900000000000526001\t9468002\t\n"
        ));
        let files = Rf2Files {
            simple_map_files: vec![map_base.path().into(), map_empty.path().into()],
            association_files: vec![
                association_base.path().into(),
                association_empty.path().into(),
            ],
            ..Rf2Files::default()
        };
        let dataset = Rf2Dataset::load(&files, false).unwrap();
        assert!(dataset.ctv3_maps.is_empty());
        assert!(dataset.history.is_empty());
    }
    let non_ctv3 = tsv_file(&format!(
        "{map_header}map-1\t20260101\t1\t0\t999999\t22298006\tOTHER\n"
    ));
    let files = Rf2Files {
        simple_map_files: vec![map_base.path().into(), non_ctv3.path().into()],
        ..Rf2Files::default()
    };
    assert!(Rf2Dataset::load(&files, false)
        .unwrap()
        .ctv3_maps
        .is_empty());
}

// --- Rf2Dataset::load ---

/// Build a minimal in-memory dataset:
///   root (138875005) → "Clinical finding" (404684003) → "Fever" (386661006)
#[test]
fn dataset_load_minimal() {
    let concepts_f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tdefinitionStatusId\n\
         138875005\t20020131\t1\t900000000000207008\t900000000000074008\n\
         404684003\t20020131\t1\t900000000000207008\t900000000000074008\n\
         386661006\t20020131\t1\t900000000000207008\t900000000000074008\n",
    );

    let descs_f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tconceptId\tlanguageCode\ttypeId\tterm\tcaseSignificanceId\n\
         1\t20020131\t1\t0\t138875005\ten\t900000000000003001\tSNOMED CT Concept (SNOMED RT+CTV3)\t0\n\
         2\t20020131\t1\t0\t404684003\ten\t900000000000003001\tClinical finding (finding)\t0\n\
         3\t20020131\t1\t0\t386661006\ten\t900000000000003001\tFever (finding)\t0\n\
         4\t20020131\t1\t0\t386661006\ten\t900000000000013009\tPyrexia\t0\n",
    );

    let rels_f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tsourceId\tdestinationId\trelationshipGroup\ttypeId\tcharacteristicTypeId\tmodifierId\n\
         10\t20020131\t1\t0\t404684003\t138875005\t0\t116680003\t0\t0\n\
         11\t20020131\t1\t0\t386661006\t404684003\t0\t116680003\t0\t0\n",
    );

    let lang_f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\tacceptabilityId\n\
         aa\t20020131\t1\t0\t0\t4\t900000000000548007\n",
    );

    let files = Rf2Files {
        concept_files: vec![concepts_f.path().to_path_buf()],
        description_files: vec![descs_f.path().to_path_buf()],
        relationship_files: vec![rels_f.path().to_path_buf()],
        lang_refset_files: vec![lang_f.path().to_path_buf()],
        simple_map_files: vec![],
        refset_files: vec![],
        extended_map_files: vec![],
        complex_map_files: vec![],
        attribute_value_files: vec![],
        association_files: vec![],
    };

    let ds = Rf2Dataset::load(&files, false).unwrap();
    assert_eq!(ds.concepts.len(), 3);
    assert!(ds.concepts.contains_key("138875005"));
    assert!(ds.concepts.contains_key("404684003"));
    assert!(ds.concepts.contains_key("386661006"));

    let fever_parents = ds.parents.get("386661006").unwrap();
    assert!(fever_parents.contains(&"404684003".to_string()));

    // Keyed by (refset_id, description_id); the fixture uses refsetId "0".
    assert_eq!(
        ds.acceptability.get(&("0".to_string(), "4".to_string())),
        Some(&Acceptability::Preferred)
    );

    assert!(ds.ctv3_maps.is_empty());
    assert!(ds.read2_maps.is_empty());
    assert!(ds.refset_members.is_empty());
}

/// Inactive concepts are dropped at load time by default but retained when
/// `include_inactive = true`. Regression test for `--include-inactive`, which
/// previously had no effect: inactive concepts were unconditionally filtered in
/// `Rf2Dataset::load`, so the flag (checked only later in `build_records`) never
/// had any inactive rows to act on.
#[test]
fn dataset_load_inactive_concepts_gated_by_flag() {
    let concepts_f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tdefinitionStatusId\n\
         404684003\t20020131\t1\t900000000000207008\t900000000000074008\n\
         9468002\t20020131\t0\t900000000000207008\t900000000000074008\n",
    );
    // The inactive concept keeps an ACTIVE FSN, as real RF2 does: inactivating a
    // concept does not inactivate its descriptions.
    let descs_f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\tconceptId\tlanguageCode\ttypeId\tterm\tcaseSignificanceId\n\
         1\t20020131\t1\t0\t404684003\ten\t900000000000003001\tClinical finding (finding)\t0\n\
         2\t20020131\t1\t0\t9468002\ten\t900000000000003001\tInactive example (disorder)\t0\n",
    );

    let mk_files = |c: &NamedTempFile, d: &NamedTempFile| Rf2Files {
        concept_files: vec![c.path().to_path_buf()],
        description_files: vec![d.path().to_path_buf()],
        relationship_files: vec![],
        lang_refset_files: vec![],
        simple_map_files: vec![],
        refset_files: vec![],
        extended_map_files: vec![],
        complex_map_files: vec![],
        attribute_value_files: vec![],
        association_files: vec![],
    };

    // Default: the inactive concept is dropped at load time.
    let ds = Rf2Dataset::load(&mk_files(&concepts_f, &descs_f), false).unwrap();
    assert_eq!(ds.concepts.len(), 1);
    assert!(ds.concepts.contains_key("404684003"));
    assert!(!ds.concepts.contains_key("9468002"));

    // include_inactive: the inactive concept is retained with its active flag
    // preserved, and its active FSN description is available to the builder.
    let ds = Rf2Dataset::load(&mk_files(&concepts_f, &descs_f), true).unwrap();
    assert_eq!(ds.concepts.len(), 2);
    let inactive = ds
        .concepts
        .get("9468002")
        .expect("inactive concept retained under include_inactive");
    assert!(!inactive.active);
    assert!(
        ds.descriptions.contains_key("9468002"),
        "active FSN of the inactive concept is loaded"
    );
}

// --- Simple refset parsing ---

#[test]
fn parse_simple_refset_active_and_inactive() {
    let f = tsv_file(
        "id\teffectiveTime\tactive\tmoduleId\trefsetId\treferencedComponentId\n\
         uuid1\t20250101\t1\t999000031000000106\t1129631000000105\t386661006\n\
         uuid2\t20250101\t0\t999000031000000106\t1129631000000105\t22298006\n",
    );
    let rows = parse_simple_refset(f.path()).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].active);
    assert_eq!(rows[0].refset_id, "1129631000000105");
    assert_eq!(rows[0].referenced_component_id, "386661006");
    assert!(!rows[1].active);
}
