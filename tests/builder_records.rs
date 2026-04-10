//! Integration tests for the NDJSON record builder.
//!
//! These tests hand-build a tiny `Rf2Dataset` in memory and assert on the
//! `ConceptRecord`s produced by `sct_rs::builder::build_records`.

use std::collections::HashMap;

use sct_rs::builder::build_records;
use sct_rs::rf2::{Acceptability, ConceptRow, DescriptionRow, Rf2Dataset, TYPE_FSN, TYPE_SYNONYM};
use sct_rs::schema::SCHEMA_VERSION;

/// Minimal dataset: root → clinical-finding → fever
fn minimal_dataset() -> Rf2Dataset {
    let mut concepts = HashMap::new();
    for id in &["138875005", "404684003", "386661006"] {
        concepts.insert(
            id.to_string(),
            ConceptRow {
                id: id.to_string(),
                effective_time: "20020131".into(),
                active: true,
                module_id: "900000000000207008".into(),
                definition_status_id: "900000000000074008".into(),
            },
        );
    }

    let mut descriptions: HashMap<String, Vec<DescriptionRow>> = HashMap::new();
    let desc_data: &[(&str, &str, &str)] = &[
        // (concept_id, type_id, term)
        ("138875005", TYPE_FSN, "SNOMED CT Concept (SNOMED RT+CTV3)"),
        ("404684003", TYPE_FSN, "Clinical finding (finding)"),
        ("386661006", TYPE_FSN, "Fever (finding)"),
        ("386661006", TYPE_SYNONYM, "Pyrexia"),
    ];
    for (i, (cid, type_id, term)) in desc_data.iter().enumerate() {
        descriptions
            .entry(cid.to_string())
            .or_default()
            .push(DescriptionRow {
                id: (i + 1).to_string(),
                effective_time: "20020131".into(),
                active: true,
                concept_id: cid.to_string(),
                language_code: "en".into(),
                type_id: type_id.to_string(),
                term: term.to_string(),
                case_significance_id: "0".into(),
            });
    }

    let mut parents: HashMap<String, Vec<String>> = HashMap::new();
    parents.insert("404684003".into(), vec!["138875005".into()]);
    parents.insert("386661006".into(), vec!["404684003".into()]);

    // Mark description "4" (Pyrexia) as Preferred
    let mut acceptability = HashMap::new();
    acceptability.insert("4".into(), Acceptability::Preferred);

    Rf2Dataset {
        concepts,
        descriptions,
        parents,
        attributes: HashMap::new(),
        acceptability,
        ctv3_maps: HashMap::new(),
        read2_maps: HashMap::new(),
    }
}

#[test]
fn hierarchy_path_fever() {
    let ds = minimal_dataset();
    let records = build_records(&ds, "en", false).unwrap();
    let fever = records.iter().find(|r| r.id == "386661006").unwrap();

    assert_eq!(fever.hierarchy, "Clinical finding");

    assert_eq!(
        fever.hierarchy_path,
        vec!["SNOMED CT Concept", "Clinical finding", "Fever"]
    );
}

#[test]
fn preferred_term_locale_match() {
    let ds = minimal_dataset();
    // Description id "4" is the Pyrexia synonym, marked Preferred in acceptability
    let records = build_records(&ds, "en", false).unwrap();
    let fever = records.iter().find(|r| r.id == "386661006").unwrap();
    assert_eq!(fever.preferred_term, "Pyrexia");
}

#[test]
fn synonyms_exclude_preferred_term() {
    let ds = minimal_dataset();
    let records = build_records(&ds, "en", false).unwrap();
    let fever = records.iter().find(|r| r.id == "386661006").unwrap();
    // Pyrexia is the preferred term, so synonyms should not repeat it
    assert!(!fever.synonyms.contains(&"Pyrexia".to_string()));
}

#[test]
fn children_count() {
    let ds = minimal_dataset();
    let records = build_records(&ds, "en", false).unwrap();
    let cf = records.iter().find(|r| r.id == "404684003").unwrap();
    // "Clinical finding" has one child: Fever
    assert_eq!(cf.children_count, 1);
}

#[test]
fn schema_version_is_current() {
    let ds = minimal_dataset();
    let records = build_records(&ds, "en", false).unwrap();
    for r in &records {
        assert_eq!(r.schema_version, SCHEMA_VERSION);
    }
}
