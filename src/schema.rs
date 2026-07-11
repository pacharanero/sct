// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Canonical per-concept record type.
//!
//! This is the stable public interface between the NDJSON producer (`sct ndjson`)
//! and all downstream consumers (`sct sqlite`, `sct parquet`, `sct markdown`, `sct mcp`).
//!
//! The format is versioned with `schema_version` so consumers can detect
//! incompatible format changes at parse time.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Current NDJSON schema version. Increment when the record structure changes
/// in a backward-incompatible way.
///
/// v4: adds `relationships` (typed attribute triples, SCTID-keyed) to support
/// ECL attribute refinement. Additive - older records parse with an empty list.
/// v5: adds `crossmaps` (SNOMED CT → ICD-10 / OPCS-4 ExtendedMap targets).
/// Additive - older records parse with an empty list.
/// v6: adds `gtin_codes` (Global Trade Item Numbers).
/// Additive - older records parse with an empty list.
#[cfg(feature = "gtin")]
pub const SCHEMA_VERSION: u32 = 6;
#[cfg(not(feature = "gtin"))]
pub const SCHEMA_VERSION: u32 = 5;

/// A lightweight reference to another concept (used in parents and attributes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptRef {
    pub id: String,
    pub fsn: String,
}

/// A typed attribute relationship, with all parts kept as SCTIDs (plus the
/// relationship group number). Unlike the display-oriented `attributes` map,
/// this preserves the attribute *type* SCTID and group, which ECL refinement
/// needs. See `spec/ecl.md` §4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    /// Attribute type SCTID, e.g. `363698007` (Finding site).
    pub type_id: String,
    /// Destination (value) concept SCTID.
    pub destination_id: String,
    /// RF2 relationship group number (0 = ungrouped).
    pub group: u32,
}

/// A SNOMED CT → external classification map target (ICD-10, OPCS-4), parsed
/// from the RF2 ExtendedMap reference sets. Preserves the map group / priority /
/// rule / advice so the full map context survives into the SQLite `crossmaps`
/// table. See `spec/cross-terminology-mapping.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossMapEntry {
    /// Target classification system: `icd10` | `opcs4`.
    pub system: String,
    /// Target code (e.g. ICD-10 `I219`, OPCS-4 `Q288`).
    pub code: String,
    /// Source SNOMED map refset SCTID (provenance; distinguishes multiple maps
    /// targeting the same system).
    pub refset: String,
    /// RF2 map group (alternatives within a group).
    pub group: u32,
    /// Priority within the group.
    pub priority: u32,
    /// Map rule (e.g. age/sex condition), often empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rule: String,
    /// Human-readable map advice.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub advice: String,
    /// Correlation SCTID (exact / broad / narrow / inexact).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub correlation: String,
}

/// One historical association (concept history), written to the sidecar
/// `<stem>.history.ndjson` artefact and loaded into the SQLite `concept_history`
/// table. Kept separate from `ConceptRecord` because the `source` is usually an
/// *inactive* concept, absent from the active concept stream. See
/// `spec/cross-terminology-mapping.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    /// The (usually inactive) source concept SCTID.
    pub source: String,
    /// Association type: `replaced_by` | `same_as` | `possibly_equivalent_to` |
    /// `moved_to` | `was_a` | … (see `rf2::association_name`).
    pub association: String,
    /// The related / replacement concept SCTID.
    pub target: String,
}

/// The per-concept JSON record written to the NDJSON artefact.
///
/// One record per line, sorted by `id` (ascending numeric SCTID).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptRecord {
    pub id: String,
    pub fsn: String,
    pub preferred_term: String,
    pub synonyms: Vec<String>,
    pub hierarchy: String,
    pub hierarchy_path: Vec<String>,
    pub parents: Vec<ConceptRef>,
    pub children_count: usize,
    pub active: bool,
    pub module: String,
    pub effective_time: String,
    pub attributes: IndexMap<String, Vec<ConceptRef>>,
    /// CTV3 (Read v3) codes mapped to this concept (may be empty if no UK map loaded)
    #[serde(default)]
    pub ctv3_codes: Vec<String>,
    /// Read v2 codes mapped to this concept (may be empty if no UK map loaded)
    #[serde(default)]
    pub read2_codes: Vec<String>,
    /// SCTIDs of reference sets this concept belongs to. Populated when the
    /// NDJSON was built with `--refsets simple` (or higher). Each ID itself
    /// resolves to a concept in this dataset - look it up to get the refset's
    /// preferred term, module, and other metadata.
    #[serde(default)]
    pub refsets: Vec<String>,
    /// Typed attribute relationships (SCTID-keyed, with group). Populated from
    /// the same RF2 data that feeds `attributes`, but preserving the type SCTID
    /// and group for ECL. Empty on records from schema v3 and earlier.
    #[serde(default)]
    pub relationships: Vec<Relationship>,
    /// SNOMED CT → ICD-10 / OPCS-4 map targets (from RF2 ExtendedMap refsets).
    /// Populated with `--refsets all`. Empty on records from schema v4 and earlier.
    #[serde(default)]
    pub crossmaps: Vec<CrossMapEntry>,
    /// GTIN (Global Trade Item Number) codes mapped to this concept.
    /// Additive - older records parse with an empty list.
    #[cfg(feature = "gtin")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gtin_codes: Vec<String>,
    pub schema_version: u32,
}

impl Default for ConceptRecord {
    fn default() -> Self {
        Self {
            id: String::new(),
            fsn: String::new(),
            preferred_term: String::new(),
            synonyms: vec![],
            hierarchy: String::new(),
            hierarchy_path: vec![],
            parents: vec![],
            children_count: 0,
            attributes: indexmap::IndexMap::new(),
            active: true,
            module: String::new(),
            effective_time: String::new(),
            ctv3_codes: vec![],
            read2_codes: vec![],
            refsets: vec![],
            relationships: vec![],
            crossmaps: vec![],
            #[cfg(feature = "gtin")]
            gtin_codes: vec![],
            schema_version: SCHEMA_VERSION,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ConceptRecord {
        ConceptRecord {
            id: "22298006".into(),
            fsn: "Myocardial infarction (disorder)".into(),
            preferred_term: "Myocardial infarction".into(),
            synonyms: vec!["Heart attack".into()],
            hierarchy: "Clinical finding".into(),
            hierarchy_path: vec![],
            parents: vec![],
            children_count: 0,
            active: true,
            module: "x".into(),
            effective_time: "20260101".into(),
            attributes: IndexMap::new(),
            ctv3_codes: vec![],
            read2_codes: vec![],
            refsets: vec![],
            relationships: vec![Relationship {
                type_id: "363698007".into(),
                destination_id: "74281007".into(),
                group: 1,
            }],
            crossmaps: vec![CrossMapEntry {
                system: "icd10".into(),
                code: "I219".into(),
                refset: "999002271000000101".into(),
                group: 1,
                priority: 1,
                rule: String::new(),
                advice: "ALWAYS I21.9".into(),
                correlation: String::new(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn v4_record_with_relationships_round_trips() {
        let rec = sample();
        let json = serde_json::to_string(&rec).unwrap();
        let back: ConceptRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.relationships.len(), 1);
        assert_eq!(back.relationships[0].type_id, "363698007");
        assert_eq!(back.relationships[0].destination_id, "74281007");
        assert_eq!(back.relationships[0].group, 1);
        assert_eq!(back.crossmaps.len(), 1);
        assert_eq!(back.crossmaps[0].system, "icd10");
        assert_eq!(back.crossmaps[0].code, "I219");
        assert_eq!(back.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn v3_json_without_relationships_still_parses() {
        // A pre-v4 record has no `relationships` key; serde default fills it empty.
        let v3 = r#"{"id":"73211009","fsn":"Diabetes mellitus (disorder)",
            "preferred_term":"Diabetes mellitus","synonyms":[],"hierarchy":"Clinical finding",
            "hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x",
            "effective_time":"","attributes":{},"schema_version":3}"#;
        let rec: ConceptRecord = serde_json::from_str(v3).unwrap();
        assert!(rec.relationships.is_empty());
        assert_eq!(rec.schema_version, 3);
    }
}
