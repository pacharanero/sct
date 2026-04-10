//! Integration tests for the RF2 parsing layer.
//!
//! These tests exercise the public `sct_rs::rf2` API: the row-level parsers
//! and the aggregated `Rf2Dataset::load`. They write tiny, hand-crafted TSV
//! fixtures to temp files to keep the tests hermetic.

use std::io::Write;
use tempfile::NamedTempFile;

use sct_rs::rf2::{
    parse_concepts, parse_descriptions, parse_lang_refset, parse_relationships, parse_simple_map,
    parse_simple_refset, Acceptability, Rf2Dataset, Rf2Files, IS_A, PREFERRED, TYPE_FSN,
};

fn tsv_file(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
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
    };

    let ds = Rf2Dataset::load(&files).unwrap();
    assert_eq!(ds.concepts.len(), 3);
    assert!(ds.concepts.contains_key("138875005"));
    assert!(ds.concepts.contains_key("404684003"));
    assert!(ds.concepts.contains_key("386661006"));

    let fever_parents = ds.parents.get("386661006").unwrap();
    assert!(fever_parents.contains(&"404684003".to_string()));

    assert_eq!(ds.acceptability.get("4"), Some(&Acceptability::Preferred));

    assert!(ds.ctv3_maps.is_empty());
    assert!(ds.read2_maps.is_empty());
    assert!(ds.refset_members.is_empty());
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
