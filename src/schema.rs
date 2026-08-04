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

use crate::provenance::Provenance;

/// Current NDJSON schema version. Increment when the record structure changes
/// in a backward-incompatible way.
///
/// v4: adds `relationships` (typed attribute triples, SCTID-keyed) to support
/// ECL attribute refinement. Additive - older records parse with an empty list.
/// v5: adds `crossmaps` (SNOMED CT → ICD-10 / OPCS-4 ExtendedMap targets).
/// Additive - older records parse with an empty list.
/// v6: adds `definition_status` (primitive / fully-defined) for definition
/// diagrams. Additive - older records parse with an empty string.
pub const SCHEMA_VERSION: u32 = 6;

/// Schema version for the canonical payload-refset companion stream.
pub const REFSET_SIDECAR_SCHEMA_VERSION: u32 = 1;
pub const HISTORY_SIDECAR_SCHEMA_VERSION: u32 = 1;

/// Discriminator for the first line of a payload-refset companion stream.
pub const REFSET_SIDECAR_TYPE_TAG: &str = "sct_refset_provenance";

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

/// Provenance header for `<stem>.refsets.ndjson`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefsetSidecarProvenance {
    #[serde(rename = "_type")]
    pub type_tag: String,
    pub schema_version: u32,
    /// Fingerprint of the payload-refset records that follow this header.
    pub refset_fingerprint: String,
    /// Provenance of the concept NDJSON this companion stream belongs to.
    pub source: Provenance,
}

impl RefsetSidecarProvenance {
    pub fn new(source: Provenance, refset_fingerprint: String) -> Self {
        Self {
            type_tag: REFSET_SIDECAR_TYPE_TAG.to_string(),
            schema_version: REFSET_SIDECAR_SCHEMA_VERSION,
            refset_fingerprint,
            source,
        }
    }
}

/// One RF2 Complex Map reference set member, including the complete member envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComplexMapRefsetMember {
    pub id: String,
    pub effective_time: String,
    pub active: bool,
    pub module_id: String,
    pub refset_id: String,
    pub referenced_component_id: String,
    pub map_group: u32,
    pub map_priority: u32,
    pub map_rule: String,
    pub map_advice: String,
    pub map_target: String,
    pub correlation_id: String,
}

/// One RF2 Extended Map reference set member, including International and UK tail fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtendedMapRefsetMember {
    pub id: String,
    pub effective_time: String,
    pub active: bool,
    pub module_id: String,
    pub refset_id: String,
    pub referenced_component_id: String,
    pub map_group: u32,
    pub map_priority: u32,
    pub map_rule: String,
    pub map_advice: String,
    pub map_target: String,
    pub correlation_id: String,
    /// International RF2 Extended Map payload field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_category_id: Option<String>,
    /// UK RF2 Extended Map payload field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_block: Option<u32>,
}

/// One RF2 Attribute Value reference set member, including the complete member envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeValueRefsetMember {
    pub id: String,
    pub effective_time: String,
    pub active: bool,
    pub module_id: String,
    pub refset_id: String,
    pub referenced_component_id: String,
    pub value_id: String,
}

/// Typed record in `<stem>.refsets.ndjson`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "_type", content = "member")]
pub enum RefsetMemberRecord {
    #[serde(rename = "sct_complex_map_refset_member")]
    ComplexMap(ComplexMapRefsetMember),
    #[serde(rename = "sct_extended_map_refset_member")]
    ExtendedMap(ExtendedMapRefsetMember),
    #[serde(rename = "sct_attribute_value_refset_member")]
    AttributeValue(AttributeValueRefsetMember),
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
#[derive(Debug, Serialize, Deserialize)]
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
    /// RF2 definition status SCTID: primitive (`900000000000074008`) or fully
    /// defined (`900000000000073002`). Empty on records from schema v5 and earlier.
    #[serde(default)]
    pub definition_status: String,
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
    pub schema_version: u32,
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
            definition_status: "900000000000073002".into(),
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
            schema_version: SCHEMA_VERSION,
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
        assert_eq!(back.definition_status, "900000000000073002");
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
        assert!(rec.definition_status.is_empty());
        assert_eq!(rec.schema_version, 3);
    }
}
