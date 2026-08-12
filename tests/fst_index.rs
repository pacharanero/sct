// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integration tests for the FST index: build a `snomed.fst` from a synthetic
//! NDJSON fixture and round-trip exact / prefix / fuzzy / word queries through
//! it. No real SNOMED release is touched (see `spec/commands/fst.md` §8).

use sct_rs::index::{self, format, Index};
use std::io::Cursor;

/// A hand-crafted NDJSON fixture covering: a provenance header, FSNs with
/// semantic tags, a term shared by two concepts ("Cold"), shared phrase
/// structure for prefix/word search, and a Unicode term with diacritics.
const FIXTURE: &str = r#"{"_type":"sct_provenance","edition_label":"International","release_date":"2026-03-01","release_id":"SnomedCT_InternationalRF2_PRODUCTION_20260301T120000Z","source_paths":["/tmp/release"],"sct_version":"0.0.0-test","created_at":"2026-03-02T00:00:00Z"}
{"id":"22298006","fsn":"Myocardial infarction (disorder)","preferred_term":"Myocardial infarction","synonyms":["Heart attack","MI"],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":3}
{"id":"82272006","fsn":"Common cold (disorder)","preferred_term":"Common cold","synonyms":["Cold"],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":3}
{"id":"84162001","fsn":"Cold sensation (finding)","preferred_term":"Cold sensation","synonyms":["Cold"],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":3}
{"id":"71620000","fsn":"Fracture of left femur (disorder)","preferred_term":"Fracture of left femur","synonyms":[],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":3}
{"id":"71620001","fsn":"Fracture of right femur (disorder)","preferred_term":"Fracture of right femur","synonyms":[],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":3}
{"id":"1234567","fsn":"Ménière's disease (disorder)","preferred_term":"Ménière's disease","synonyms":[],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":3}"#;

/// Build the fixture into a tempfile and open it. Returns the index plus the
/// tempdir guard (kept alive by the caller).
fn build_fixture() -> (Index, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.fst");
    {
        let mut out = std::fs::File::create(&path).unwrap();
        let stats = index::build(Cursor::new(FIXTURE), &mut out).unwrap();
        assert_eq!(stats.concepts, 6);
    }
    let idx = Index::open(&path).unwrap();
    (idx, dir)
}

#[test]
fn exact_lookup_is_case_insensitive() {
    let (idx, _dir) = build_fixture();
    let hits = idx.lookup_exact("Myocardial Infarction");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].concept_id, 22298006);
    assert_eq!(hits[0].term, "Myocardial infarction");
    assert_eq!(hits[0].semantic_tag.as_deref(), Some("disorder"));
}

#[test]
fn exact_lookup_via_synonym() {
    let (idx, _dir) = build_fixture();
    // "Heart attack" is a synonym of myocardial infarction.
    let hits = idx.lookup_exact("heart attack");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].concept_id, 22298006);
}

#[test]
fn shared_term_resolves_to_multiple_concepts() {
    let (idx, _dir) = build_fixture();
    let mut ids: Vec<u64> = idx
        .lookup_exact("cold")
        .iter()
        .map(|h| h.concept_id)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![82272006, 84162001]);
}

#[test]
fn prefix_search_finds_shared_phrase() {
    let (idx, _dir) = build_fixture();
    let mut ids: Vec<u64> = idx
        .lookup_prefix("fracture of", 10)
        .unwrap()
        .iter()
        .map(|h| h.concept_id)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![71620000, 71620001]);
}

#[test]
fn fuzzy_search_tolerates_a_typo() {
    let (idx, _dir) = build_fixture();
    // One transposition/substitution from "myocardial infarction".
    let hits = idx.lookup_fuzzy("myocardial infarcton", 1, 10).unwrap();
    assert!(
        hits.iter().any(|h| h.concept_id == 22298006),
        "fuzzy d=1 should recover the misspelled term, got {hits:?}"
    );
}

#[test]
fn word_intersection_requires_all_words() {
    let (idx, _dir) = build_fixture();

    let both: Vec<u64> = idx
        .lookup_words(&["fracture", "femur"], 10)
        .iter()
        .map(|h| h.concept_id)
        .collect();
    assert_eq!(both.len(), 2, "both femur fractures contain both words");

    let left: Vec<u64> = idx
        .lookup_words(&["fracture", "left"], 10)
        .iter()
        .map(|h| h.concept_id)
        .collect();
    assert_eq!(left, vec![71620000]);

    // A word present in no term yields no results.
    assert!(idx.lookup_words(&["fracture", "zzzznope"], 10).is_empty());
}

#[test]
fn diacritics_are_preserved_not_folded() {
    let (idx, _dir) = build_fixture();
    // Exact accented form matches.
    assert_eq!(idx.lookup_exact("ménière's disease").len(), 1);
    // The de-accented form must NOT match - we deliberately keep precision.
    assert!(idx.lookup_exact("meniere's disease").is_empty());
}

#[test]
fn provenance_round_trips() {
    let (idx, _dir) = build_fixture();
    let prov = idx.provenance().expect("provenance recorded");
    assert_eq!(prov.edition_label, "International");
    assert_eq!(prov.release_date, "2026-03-01");
}

#[test]
fn no_terms_build_omits_labels_but_still_resolves_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noterms.fst");
    {
        let mut out = std::fs::File::create(&path).unwrap();
        index::build_with_options(
            Cursor::new(FIXTURE),
            &mut out,
            &index::BuildOptions {
                include_terms: false,
            },
        )
        .unwrap();
    }
    let idx = Index::open(&path).unwrap();
    assert!(!idx.has_terms(), "index built with include_terms=false");

    // Lookup still resolves the concept id; only the display label is absent.
    let hits = idx.lookup_exact("myocardial infarction");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].concept_id, 22298006);
    assert!(
        hits[0].term.is_empty(),
        "no label without the terms section"
    );
    // Semantic tag still comes from the packed value, not the terms table.
    assert_eq!(hits[0].semantic_tag.as_deref(), Some("disorder"));
}

#[test]
fn default_build_includes_labels() {
    let (idx, _dir) = build_fixture();
    assert!(idx.has_terms());
}

#[test]
fn semantic_tags_are_collected() {
    let (idx, _dir) = build_fixture();
    let tags: Vec<&str> = idx.semantic_tags().collect();
    assert!(tags.contains(&"disorder"));
    assert!(tags.contains(&"finding"));
}

/// A second fixture, otherwise identical in shape to [`FIXTURE`], with one
/// concept marked `"active":false` - the shape `sct ndjson --include-inactive`
/// produces for a retired concept.
const FIXTURE_WITH_INACTIVE: &str = r#"{"_type":"sct_provenance","edition_label":"International","release_date":"2026-03-01","release_id":"SnomedCT_InternationalRF2_PRODUCTION_20260301T120000Z","source_paths":["/tmp/release"],"sct_version":"0.0.0-test","created_at":"2026-03-02T00:00:00Z"}
{"id":"22298006","fsn":"Myocardial infarction (disorder)","preferred_term":"Myocardial infarction","synonyms":["Heart attack"],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":3}
{"id":"9468002","fsn":"Inactive example disorder (disorder)","preferred_term":"Inactive example disorder","synonyms":[],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":false,"module":"x","effective_time":"","attributes":{},"schema_version":3}"#;

/// R11: a concept's active status flows through the FST container end to end -
/// build, open, and every query surface (`lookup_exact` and the JSON hit
/// shape), so `sct fst search` and `sct sayt` can flag a retired concept the
/// same way `sct lexical` already does.
#[test]
fn active_status_round_trips_through_the_container() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("with_inactive.fst");
    {
        let mut out = std::fs::File::create(&path).unwrap();
        let stats = index::build(Cursor::new(FIXTURE_WITH_INACTIVE), &mut out).unwrap();
        assert_eq!(stats.concepts, 2);
        assert_eq!(
            stats.inactive_concepts, 1,
            "the build must count the one inactive concept"
        );
    }
    let idx = Index::open(&path).unwrap();

    let inactive = idx.lookup_exact("Inactive example disorder");
    assert_eq!(inactive.len(), 1);
    assert!(
        !inactive[0].active,
        "the retired concept must read inactive"
    );
    assert_eq!(inactive[0].to_json()["active"], false);

    let active = idx.lookup_exact("Myocardial infarction");
    assert_eq!(active.len(), 1);
    assert!(active[0].active, "the live concept must read active");
    assert_eq!(active[0].to_json()["active"], true);

    assert!(idx.is_active(22298006));
    assert!(!idx.is_active(9468002));
    // A concept the index has never seen reads as active - "unknown" is not
    // "known inactive".
    assert!(idx.is_active(999999999));
}

/// A default build (no `--include-inactive`) never records any inactive
/// concepts, so every hit reads active - the pre-R11 behaviour, unchanged.
#[test]
fn every_concept_is_active_on_a_default_build() {
    let (idx, _dir) = build_fixture();
    for hit in idx.lookup_exact("Myocardial infarction") {
        assert!(hit.active);
    }
}

#[test]
fn rejects_truncated_terms_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupt.fst");
    index::build(
        Cursor::new(FIXTURE),
        &mut std::fs::File::create(&path).unwrap(),
    )
    .unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let terms = format::Toc::parse(&bytes)
        .unwrap()
        .require(format::SEC_TERMS_INDEX)
        .unwrap();
    bytes[terms.start..terms.start + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();

    assert!(Index::open(&path).is_err());
}

#[test]
fn rejects_out_of_bounds_term_text_range() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupt.fst");
    index::build(
        Cursor::new(FIXTURE),
        &mut std::fs::File::create(&path).unwrap(),
    )
    .unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let terms = format::Toc::parse(&bytes)
        .unwrap()
        .require(format::SEC_TERMS_INDEX)
        .unwrap();
    bytes[terms.start + 12..terms.start + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();

    assert!(Index::open(&path).is_err());
}
