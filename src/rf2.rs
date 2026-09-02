// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

// Row fields kept for future Layer 2 consumers.
#![allow(dead_code)]

/// RF2 file discovery and parsing.
///
/// RF2 Snapshot files are TSV files with a header row.
/// We locate them by filename pattern within the release directory tree.
use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressBarIter};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::schema::{AttributeValueRefsetMember, ComplexMapRefsetMember, ExtendedMapRefsetMember};

// ---------------------------------------------------------------------------
// Row types (borrowed slices to avoid allocations during scan)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ConceptRow {
    pub id: String,
    pub effective_time: String,
    pub active: bool,
    pub module_id: String,
    pub definition_status_id: String,
}

#[derive(Debug)]
pub struct DescriptionRow {
    pub id: String,
    pub effective_time: String,
    pub active: bool,
    pub concept_id: String,
    pub language_code: String,
    pub type_id: String, // 900000000000003001 = FSN, 900000000000013009 = synonym
    pub term: String,
    pub case_significance_id: String,
}

#[derive(Debug)]
pub struct RelationshipRow {
    pub id: String,
    pub effective_time: String,
    pub active: bool,
    pub source_id: String,
    pub destination_id: String,
    pub relationship_group: String,
    pub type_id: String, // 116680003 = Is a
    pub characteristic_type_id: String,
    pub modifier_id: String,
}

/// A row from a language refset file (der2_cRefset_Language_Snapshot_*.txt)
#[derive(Debug)]
pub struct LangRefsetRow {
    pub active: bool,
    /// Language reference set SCTID - identifies the dialect (e.g. GB vs US
    /// English, or a UK realm refset). This is what distinguishes dialects;
    /// the description's `languageCode` is "en" for both GB and US English.
    pub refset_id: String,
    pub referenced_component_id: String, // description id
    pub acceptability_id: String, // 900000000000548007 = preferred, 900000000000549004 = acceptable
}

/// A row from a simple map reference set file.
///
/// Used for CTV3 cross-maps (`der2_sRefset_SimpleMap*Snapshot*.txt`).
/// The CTV3 mappings are identified by refset ID `900000000000497000`.
///
/// Columns (TSV): id effectiveTime active moduleId refsetId referencedComponentId mapTarget
#[derive(Debug)]
pub struct SimpleMapRow {
    pub id: String,
    pub active: bool,
    pub refset_id: String, // identifies the terminology (e.g. CTV3)
    pub referenced_component_id: String, // SNOMED CT SCTID
    pub map_target: String, // CTV3 or other legacy code
}

/// A row from a generic concept-level simple reference set file.
///
/// Used for membership-only refsets like SCR exclusion
/// (`der2_Refset_Simple*Snapshot*.txt`). Each row asserts that a referenced
/// component (usually a concept) is a member of a given refset at a given
/// point in time, with no additional payload.
///
/// Columns (TSV): id effectiveTime active moduleId refsetId referencedComponentId
#[derive(Debug)]
pub struct SimpleRefsetRow {
    pub id: String,
    pub active: bool,
    pub refset_id: String,
    pub referenced_component_id: String,
}

/// Backwards-compatible parser name for a canonical Extended Map member.
pub type ExtendedMapRow = ExtendedMapRefsetMember;

/// A row from a historical Association reference set file
/// (`der2_cRefset_Association*Snapshot*.txt`). Maps an inactivated concept to a
/// related/replacement concept; `refset_id` is the association type (see
/// [`association_name`]).
///
/// Columns (TSV): id effectiveTime active moduleId refsetId referencedComponentId targetComponentId
#[derive(Debug)]
pub struct AssociationRow {
    pub id: String,
    pub active: bool,
    pub refset_id: String,               // association type
    pub referenced_component_id: String, // the (usually inactive) source concept
    pub target_component_id: String,     // the related/replacement concept
}

// ---------------------------------------------------------------------------
// SNOMED CT type_id constants
// ---------------------------------------------------------------------------
pub const TYPE_FSN: &str = "900000000000003001";
pub const TYPE_SYNONYM: &str = "900000000000013009";
pub const IS_A: &str = "116680003";
pub const PREFERRED: &str = "900000000000548007";
/// Refset ID for the SNOMED CT → CTV3 simple map reference set.
pub const REFSET_CTV3_SIMPLE_MAP: &str = "900000000000497000";

/// Classify a SNOMED CT ExtendedMap refset SCTID into its target classification
/// (`icd10` | `opcs4`). Seeded with the known UK + International maps. Rows
/// whose refset is not listed remain in the lossless refset companion stream but
/// are omitted from the classified query projection. See `spec/cross-terminology-mapping.md`.
///
/// ```
/// use sct_rs::rf2::extended_map_system;
/// assert_eq!(extended_map_system("447562003"), Some("icd10")); // International -> ICD-10
/// assert_eq!(extended_map_system("1126441000000105"), Some("opcs4")); // UK -> OPCS-4
/// assert_eq!(extended_map_system("not-a-map-refset"), None);
/// ```
pub fn extended_map_system(refset_id: &str) -> Option<&'static str> {
    match refset_id {
        "1126441000000105" => Some("opcs4"), // UK SNOMED CT → OPCS-4
        // UK SNOMED CT → ICD-10 maps (5th edition + supplements).
        "999002271000000101" | "1382401000000109" | "1891651000000103" => Some("icd10"),
        "447562003" => Some("icd10"), // International SNOMED CT → ICD-10
        _ => None,
    }
}

/// Human-readable name for a historical Association refset SCTID, used as the
/// `association` value in `concept_history`. Re-exported from
/// [`crate::schema::association_name`], which the ECL history-supplement
/// evaluator also uses and which is therefore not gated on the `cli` feature.
pub use crate::schema::association_name;

// Language reference set SCTIDs - the dialect selectors honoured by `--locale`.
// See `builder::language_refset_priority`.
/// Great Britain English (International edition).
pub const LANG_GB_ENGLISH: &str = "900000000000508004";
/// US English (International edition).
pub const LANG_US_ENGLISH: &str = "900000000000509007";
/// UK National (Clinical) language reference set - UK-realm preferred terms.
pub const LANG_UK_CLINICAL: &str = "999001261000000100";
/// UK dm+d (drug extension) realm description language reference set.
pub const LANG_UK_DRUG: &str = "999000691000001104";

// ---------------------------------------------------------------------------
// RF2 file discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct Rf2Files {
    pub concept_files: Vec<PathBuf>,
    pub description_files: Vec<PathBuf>,
    pub relationship_files: Vec<PathBuf>,
    pub lang_refset_files: Vec<PathBuf>,
    /// Simple map reference set files (`der2_sRefset_SimpleMap*Snapshot*.txt`).
    /// Contains CTV3 and other cross-maps, distinguished by refset ID within each file.
    pub simple_map_files: Vec<PathBuf>,
    /// Generic concept-level simple refset files (`der2_Refset_Simple*Snapshot*.txt`).
    /// Membership-only refsets (e.g. SCR exclusion, GP summary), where each row
    /// asserts that a concept belongs to the given refset with no extra payload.
    pub refset_files: Vec<PathBuf>,
    /// ExtendedMap refset files (`der2_i*Refset_ExtendedMap*Snapshot*.txt`) -
    /// SNOMED CT → ICD-10 / OPCS-4 maps. Loaded with `--refsets all`.
    pub extended_map_files: Vec<PathBuf>,
    /// ComplexMap refset files (`der2_iissscRefset_ComplexMap*Snapshot*.txt`).
    /// Loaded with `--refsets all` and preserved without guessing a target system.
    pub complex_map_files: Vec<PathBuf>,
    /// AttributeValue refset files (`der2_cRefset_AttributeValue*Snapshot*.txt`).
    /// Loaded with `--refsets all`; includes concept inactivation indicators.
    pub attribute_value_files: Vec<PathBuf>,
    /// Historical Association refset files (`der2_cRefset_Association*Snapshot*.txt`) -
    /// inactive-concept forwarding. Loaded with `--refsets all`.
    pub association_files: Vec<PathBuf>,
}

/// Walk the RF2 directory tree and collect snapshot TSV paths by type.
///
/// ```no_run
/// use std::path::Path;
/// use sct_rs::rf2::discover_rf2_files;
/// let files = discover_rf2_files(Path::new("SnomedCT_Edition/Snapshot")).unwrap();
/// println!("{} concept file(s)", files.concept_files.len());
/// ```
pub fn discover_rf2_files(rf2_dir: &Path) -> Result<Rf2Files> {
    let mut files = Rf2Files::default();

    for entry in WalkDir::new(rf2_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        if name.starts_with("sct2_Concept_") && name.contains("Snapshot") && name.ends_with(".txt")
        {
            files.concept_files.push(path.to_path_buf());
        } else if name.starts_with("sct2_Description_")
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            files.description_files.push(path.to_path_buf());
        } else if (name.starts_with("sct2_Relationship_")
            || name.starts_with("sct2_StatedRelationship_"))
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            files.relationship_files.push(path.to_path_buf());
        } else if name.starts_with("der2_cRefset_Language")
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            files.lang_refset_files.push(path.to_path_buf());
        } else if name.starts_with("der2_sRefset_SimpleMap")
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            files.simple_map_files.push(path.to_path_buf());
        } else if name.starts_with("der2_Refset_Simple")
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            files.refset_files.push(path.to_path_buf());
        } else if name.contains("Refset_ExtendedMap")
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            // der2_iisssciRefset_ExtendedMap… / der2_iisssccRefset_ExtendedMap…
            files.extended_map_files.push(path.to_path_buf());
        } else if name.contains("Refset_ComplexMap")
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            files.complex_map_files.push(path.to_path_buf());
        } else if name.starts_with("der2_cRefset_AttributeValue")
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            files.attribute_value_files.push(path.to_path_buf());
        } else if name.starts_with("der2_cRefset_Association")
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            files.association_files.push(path.to_path_buf());
        }
    }

    files.concept_files.sort();
    files.description_files.sort();
    files.relationship_files.sort();
    files.lang_refset_files.sort();
    files.simple_map_files.sort();
    files.refset_files.sort();
    files.extended_map_files.sort();
    files.complex_map_files.sort();
    files.attribute_value_files.sort();
    files.association_files.sort();

    Ok(files)
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// A `Read` wrapper that drives a byte-oriented progress bar (with ETA) as the
/// underlying RF2 file is consumed, and clears it when the reader is dropped -
/// so each of the loader's file reads shows its own progress under the
/// "Loading X from ..." breadcrumb. Auto-hides off an interactive terminal,
/// like every other `sct` progress widget.
struct ProgressReader<R: std::io::Read> {
    inner: ProgressBarIter<R>,
    pb: ProgressBar,
}

impl<R: std::io::Read> ProgressReader<R> {
    fn new(reader: R, total_bytes: u64) -> Self {
        let pb = crate::progress::byte_bar(total_bytes);
        let inner = pb.wrap_read(reader);
        Self { inner, pb }
    }
}

impl<R: std::io::Read> std::io::Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: std::io::Read> Drop for ProgressReader<R> {
    fn drop(&mut self) {
        self.pb.finish_and_clear();
    }
}

fn tsv_reader(path: &Path) -> Result<csv::Reader<ProgressReader<std::fs::File>>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let rdr = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(true)
        .flexible(false)
        .from_reader(ProgressReader::new(file, total));
    Ok(rdr)
}

/// Stream a TSV file record-by-record into `f`, reusing one `StringRecord`
/// allocation. This is the low-memory path used by [`Rf2Dataset::load`]:
/// unlike the `parse_*` functions it never materialises a whole file as a
/// `Vec`, which matters on national editions where a single relationship or
/// description file holds millions of rows.
fn stream_tsv(path: &Path, mut f: impl FnMut(&csv::StringRecord)) -> Result<()> {
    let mut rdr = tsv_reader(path)?;
    let mut record = csv::StringRecord::new();
    while rdr
        .read_record(&mut record)
        .with_context(|| format!("reading {}", path.display()))?
    {
        f(&record);
    }
    Ok(())
}

fn stream_checked_tsv(
    path: &Path,
    expected_headers: &[&str],
    mut f: impl FnMut(&csv::StringRecord) -> Result<()>,
) -> Result<()> {
    let mut rdr = tsv_reader(path)?;
    let headers = rdr
        .headers()
        .with_context(|| format!("reading headers from {}", path.display()))?;
    anyhow::ensure!(
        headers.iter().eq(expected_headers.iter().copied()),
        "unexpected RF2 header in {}: expected {}, found {}",
        path.display(),
        expected_headers.join("\t"),
        headers.iter().collect::<Vec<_>>().join("\t")
    );

    let mut record = csv::StringRecord::new();
    while rdr
        .read_record(&mut record)
        .with_context(|| format!("reading {}", path.display()))?
    {
        f(&record).with_context(|| format!("parsing {}", path.display()))?;
    }
    Ok(())
}

fn required_field<'a>(record: &'a csv::StringRecord, index: usize, name: &str) -> Result<&'a str> {
    record
        .get(index)
        .with_context(|| format!("missing required RF2 field {name}"))
}

fn parse_active(record: &csv::StringRecord) -> Result<bool> {
    match required_field(record, 2, "active")? {
        "0" => Ok(false),
        "1" => Ok(true),
        value => anyhow::bail!("invalid RF2 active value {value:?}; expected 0 or 1"),
    }
}

fn parse_u32_field(record: &csv::StringRecord, index: usize, name: &str) -> Result<u32> {
    let value = required_field(record, index, name)?;
    value
        .parse()
        .with_context(|| format!("invalid RF2 {name} value {value:?}"))
}

/// The `active` column is column 2 in every RF2 component and refset file.
fn is_active(record: &csv::StringRecord) -> bool {
    record.get(2).unwrap_or("0") == "1"
}

// Per-file-type row mappers: the single source of truth for RF2 column layout,
// shared by the Vec-returning `parse_*` API and the streaming loader.

fn concept_row(record: &csv::StringRecord) -> ConceptRow {
    // id effectiveTime active moduleId definitionStatusId
    ConceptRow {
        id: record.get(0).unwrap_or("").to_string(),
        effective_time: record.get(1).unwrap_or("").to_string(),
        active: is_active(record),
        module_id: record.get(3).unwrap_or("").to_string(),
        definition_status_id: record.get(4).unwrap_or("").to_string(),
    }
}

fn description_row(record: &csv::StringRecord) -> DescriptionRow {
    // id effectiveTime active moduleId conceptId languageCode typeId term caseSignificanceId
    DescriptionRow {
        id: record.get(0).unwrap_or("").to_string(),
        effective_time: record.get(1).unwrap_or("").to_string(),
        active: is_active(record),
        concept_id: record.get(4).unwrap_or("").to_string(),
        language_code: record.get(5).unwrap_or("").to_string(),
        type_id: record.get(6).unwrap_or("").to_string(),
        term: record.get(7).unwrap_or("").to_string(),
        case_significance_id: record.get(8).unwrap_or("").to_string(),
    }
}

fn relationship_row(record: &csv::StringRecord) -> RelationshipRow {
    // id effectiveTime active moduleId sourceId destinationId relationshipGroup typeId characteristicTypeId modifierId
    RelationshipRow {
        id: record.get(0).unwrap_or("").to_string(),
        effective_time: record.get(1).unwrap_or("").to_string(),
        active: is_active(record),
        source_id: record.get(4).unwrap_or("").to_string(),
        destination_id: record.get(5).unwrap_or("").to_string(),
        relationship_group: record.get(6).unwrap_or("").to_string(),
        type_id: record.get(7).unwrap_or("").to_string(),
        characteristic_type_id: record.get(8).unwrap_or("").to_string(),
        modifier_id: record.get(9).unwrap_or("").to_string(),
    }
}

fn lang_refset_row(record: &csv::StringRecord) -> LangRefsetRow {
    // id effectiveTime active moduleId refsetId referencedComponentId acceptabilityId
    LangRefsetRow {
        active: is_active(record),
        refset_id: record.get(4).unwrap_or("").to_string(),
        referenced_component_id: record.get(5).unwrap_or("").to_string(),
        acceptability_id: record.get(6).unwrap_or("").to_string(),
    }
}

fn simple_refset_row(record: &csv::StringRecord) -> SimpleRefsetRow {
    // id effectiveTime active moduleId refsetId referencedComponentId
    SimpleRefsetRow {
        id: record.get(0).unwrap_or("").to_string(),
        active: is_active(record),
        refset_id: record.get(4).unwrap_or("").to_string(),
        referenced_component_id: record.get(5).unwrap_or("").to_string(),
    }
}

/// Returns `None` when `mapTarget` is empty (row carries no mapping).
fn simple_map_row(record: &csv::StringRecord) -> Option<SimpleMapRow> {
    // id effectiveTime active moduleId refsetId referencedComponentId mapTarget
    let map_target = record.get(6).unwrap_or("").trim().to_string();
    if map_target.is_empty() {
        return None;
    }
    Some(SimpleMapRow {
        id: record.get(0).unwrap_or("").to_string(),
        active: is_active(record),
        refset_id: record.get(4).unwrap_or("").to_string(),
        referenced_component_id: record.get(5).unwrap_or("").to_string(),
        map_target,
    })
}

const COMPLEX_MAP_HEADERS: &[&str] = &[
    "id",
    "effectiveTime",
    "active",
    "moduleId",
    "refsetId",
    "referencedComponentId",
    "mapGroup",
    "mapPriority",
    "mapRule",
    "mapAdvice",
    "mapTarget",
    "correlationId",
];

const ATTRIBUTE_VALUE_HEADERS: &[&str] = &[
    "id",
    "effectiveTime",
    "active",
    "moduleId",
    "refsetId",
    "referencedComponentId",
    "valueId",
];

fn complex_map_row(record: &csv::StringRecord) -> Result<ComplexMapRefsetMember> {
    Ok(ComplexMapRefsetMember {
        id: required_field(record, 0, "id")?.to_string(),
        effective_time: required_field(record, 1, "effectiveTime")?.to_string(),
        active: parse_active(record)?,
        module_id: required_field(record, 3, "moduleId")?.to_string(),
        refset_id: required_field(record, 4, "refsetId")?.to_string(),
        referenced_component_id: required_field(record, 5, "referencedComponentId")?.to_string(),
        map_group: parse_u32_field(record, 6, "mapGroup")?,
        map_priority: parse_u32_field(record, 7, "mapPriority")?,
        map_rule: required_field(record, 8, "mapRule")?.to_string(),
        map_advice: required_field(record, 9, "mapAdvice")?.to_string(),
        map_target: required_field(record, 10, "mapTarget")?.to_string(),
        correlation_id: required_field(record, 11, "correlationId")?.to_string(),
    })
}

fn attribute_value_row(record: &csv::StringRecord) -> Result<AttributeValueRefsetMember> {
    Ok(AttributeValueRefsetMember {
        id: required_field(record, 0, "id")?.to_string(),
        effective_time: required_field(record, 1, "effectiveTime")?.to_string(),
        active: parse_active(record)?,
        module_id: required_field(record, 3, "moduleId")?.to_string(),
        refset_id: required_field(record, 4, "refsetId")?.to_string(),
        referenced_component_id: required_field(record, 5, "referencedComponentId")?.to_string(),
        value_id: required_field(record, 6, "valueId")?.to_string(),
    })
}

#[derive(Clone, Copy)]
enum ExtendedMapTail {
    MapCategoryId,
    MapBlock,
}

fn extended_map_row(
    record: &csv::StringRecord,
    tail: ExtendedMapTail,
) -> Result<ExtendedMapRefsetMember> {
    let (map_category_id, map_block) = match tail {
        ExtendedMapTail::MapCategoryId => (
            Some(required_field(record, 12, "mapCategoryId")?.to_string()),
            None,
        ),
        ExtendedMapTail::MapBlock => (None, Some(parse_u32_field(record, 12, "mapBlock")?)),
    };

    Ok(ExtendedMapRefsetMember {
        id: required_field(record, 0, "id")?.to_string(),
        effective_time: required_field(record, 1, "effectiveTime")?.to_string(),
        active: parse_active(record)?,
        module_id: required_field(record, 3, "moduleId")?.to_string(),
        refset_id: required_field(record, 4, "refsetId")?.to_string(),
        referenced_component_id: required_field(record, 5, "referencedComponentId")?.to_string(),
        map_group: parse_u32_field(record, 6, "mapGroup")?,
        map_priority: parse_u32_field(record, 7, "mapPriority")?,
        map_rule: required_field(record, 8, "mapRule")?.to_string(),
        map_advice: required_field(record, 9, "mapAdvice")?.to_string(),
        map_target: required_field(record, 10, "mapTarget")?.to_string(),
        correlation_id: required_field(record, 11, "correlationId")?.to_string(),
        map_category_id,
        map_block,
    })
}

fn stream_extended_map(
    path: &Path,
    mut f: impl FnMut(ExtendedMapRefsetMember) -> Result<()>,
) -> Result<()> {
    let mut rdr = tsv_reader(path)?;
    let headers = rdr
        .headers()
        .with_context(|| format!("reading headers from {}", path.display()))?;
    anyhow::ensure!(
        headers.len() == COMPLEX_MAP_HEADERS.len() + 1
            && headers
                .iter()
                .take(COMPLEX_MAP_HEADERS.len())
                .eq(COMPLEX_MAP_HEADERS.iter().copied()),
        "unexpected RF2 ExtendedMap header in {}: {}",
        path.display(),
        headers.iter().collect::<Vec<_>>().join("\t")
    );
    let tail = match headers.get(12) {
        Some("mapCategoryId") => ExtendedMapTail::MapCategoryId,
        Some("mapBlock") => ExtendedMapTail::MapBlock,
        Some(name) => anyhow::bail!(
            "unexpected RF2 ExtendedMap payload field {name:?} in {}; expected mapCategoryId or mapBlock",
            path.display()
        ),
        None => unreachable!("header length checked above"),
    };

    let mut record = csv::StringRecord::new();
    while rdr
        .read_record(&mut record)
        .with_context(|| format!("reading {}", path.display()))?
    {
        f(extended_map_row(&record, tail)?)
            .with_context(|| format!("parsing {}", path.display()))?;
    }
    Ok(())
}

/// Returns `None` when `targetComponentId` is empty (row carries no association).
fn association_row(record: &csv::StringRecord) -> Option<AssociationRow> {
    // id effectiveTime active moduleId refsetId referencedComponentId targetComponentId
    let target = record.get(6).unwrap_or("").trim().to_string();
    if target.is_empty() {
        return None;
    }
    Some(AssociationRow {
        id: record.get(0).unwrap_or("").to_string(),
        active: is_active(record),
        refset_id: record.get(4).unwrap_or("").to_string(),
        referenced_component_id: record.get(5).unwrap_or("").to_string(),
        target_component_id: target,
    })
}

/// Parse an RF2 Concept snapshot file into rows. Representative of the sibling
/// `parse_*` parsers (descriptions, relationships, refsets, maps, associations),
/// which share the same TSV shape and error handling.
///
/// ```no_run
/// use std::path::Path;
/// use sct_rs::rf2::parse_concepts;
/// let rows = parse_concepts(Path::new(
///     "Snapshot/Terminology/sct2_Concept_Snapshot_INT_20240101.txt",
/// ))
/// .unwrap();
/// println!("{} concept rows", rows.len());
/// ```
pub fn parse_concepts(path: &Path) -> Result<Vec<ConceptRow>> {
    let mut rows = Vec::new();
    stream_tsv(path, |record| rows.push(concept_row(record)))?;
    Ok(rows)
}

pub fn parse_descriptions(path: &Path) -> Result<Vec<DescriptionRow>> {
    let mut rows = Vec::new();
    stream_tsv(path, |record| rows.push(description_row(record)))?;
    Ok(rows)
}

pub fn parse_relationships(path: &Path) -> Result<Vec<RelationshipRow>> {
    let mut rows = Vec::new();
    stream_tsv(path, |record| rows.push(relationship_row(record)))?;
    Ok(rows)
}

pub fn parse_lang_refset(path: &Path) -> Result<Vec<LangRefsetRow>> {
    let mut rows = Vec::new();
    stream_tsv(path, |record| rows.push(lang_refset_row(record)))?;
    Ok(rows)
}

/// Parse a generic concept-level simple refset file.
///
/// Columns: id effectiveTime active moduleId refsetId referencedComponentId
pub fn parse_simple_refset(path: &Path) -> Result<Vec<SimpleRefsetRow>> {
    let mut rows = Vec::new();
    stream_tsv(path, |record| rows.push(simple_refset_row(record)))?;
    Ok(rows)
}

/// Parse a simple map reference set file.
///
/// Columns: id effectiveTime active moduleId refsetId referencedComponentId mapTarget
pub fn parse_simple_map(path: &Path) -> Result<Vec<SimpleMapRow>> {
    let mut rows = Vec::new();
    stream_tsv(path, |record| rows.extend(simple_map_row(record)))?;
    Ok(rows)
}

/// Parse a SNOMED CT Extended Map reference set file.
///
/// Columns: id effectiveTime active moduleId refsetId referencedComponentId
/// mapGroup mapPriority mapRule mapAdvice mapTarget correlationId, followed by
/// either the International `mapCategoryId` or UK `mapBlock` field.
pub fn parse_extended_map(path: &Path) -> Result<Vec<ExtendedMapRow>> {
    let mut rows = Vec::new();
    stream_extended_map(path, |row| {
        rows.push(row);
        Ok(())
    })?;
    Ok(rows)
}

/// Parse a SNOMED CT Complex Map reference set file.
pub fn parse_complex_map(path: &Path) -> Result<Vec<ComplexMapRefsetMember>> {
    let mut rows = Vec::new();
    stream_checked_tsv(path, COMPLEX_MAP_HEADERS, |record| {
        rows.push(complex_map_row(record)?);
        Ok(())
    })?;
    Ok(rows)
}

/// Parse a SNOMED CT Attribute Value reference set file.
pub fn parse_attribute_value(path: &Path) -> Result<Vec<AttributeValueRefsetMember>> {
    let mut rows = Vec::new();
    stream_checked_tsv(path, ATTRIBUTE_VALUE_HEADERS, |record| {
        rows.push(attribute_value_row(record)?);
        Ok(())
    })?;
    Ok(rows)
}

/// Parse a historical Association reference set file (concept history).
///
/// Columns: id effectiveTime active moduleId refsetId referencedComponentId targetComponentId
pub fn parse_association(path: &Path) -> Result<Vec<AssociationRow>> {
    let mut rows = Vec::new();
    stream_tsv(path, |record| rows.extend(association_row(record)))?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Aggregated in-memory datastore
// ---------------------------------------------------------------------------

/// The preferred term selection for a description.
#[derive(Debug, Clone, PartialEq)]
pub enum Acceptability {
    Preferred,
    Acceptable,
}

/// All data loaded from a (possibly multi-directory) RF2 snapshot.
pub struct Rf2Dataset {
    /// concept_id -> ConceptRow. Active concepts are always present; inactive
    /// concepts (`active = false`) are included only when the dataset was loaded
    /// with `include_inactive` (see [`Rf2Dataset::load`]).
    pub concepts: HashMap<String, ConceptRow>,
    /// concept_id -> Vec<DescriptionRow> (active only)
    pub descriptions: HashMap<String, Vec<DescriptionRow>>,
    /// concept_id -> Vec<parent_id> (active IS-A relationships only)
    pub parents: HashMap<String, Vec<String>>,
    /// concept_id -> Vec<(type_id, destination_id, group)> for non-IS-A active attributes
    pub attributes: HashMap<String, Vec<(String, String, String)>>,
    /// (language_refset_id, description_id) -> Acceptability (from lang refsets).
    /// Keyed by refset id as well as description id so dialects (GB vs US
    /// English, UK realm refsets) stay distinct - see `builder`.
    pub acceptability: HashMap<(String, String), Acceptability>,
    /// concept_id (SCTID) -> Vec<CTV3 code> (active mappings from UK CTV3 simple map refset)
    pub ctv3_maps: HashMap<String, Vec<String>>,
    /// concept_id (SCTID) -> Vec<Read v2 code> (active mappings from UK Read Code simple map refset)
    pub read2_maps: HashMap<String, Vec<String>>,
    /// concept_id (SCTID) -> Vec<refset_id> - generic simple refset memberships.
    /// Only concept-level memberships are retained; rows whose referencedComponentId
    /// is not a known active concept are dropped.
    pub refset_members: HashMap<String, Vec<String>>,
    /// concept_id (SCTID) -> SNOMED CT → ICD-10/OPCS-4 ExtendedMap rows.
    /// Query projection containing only active, non-null rows from recognised map refsets.
    pub extended_maps: HashMap<String, Vec<ExtendedMapRefsetMember>>,
    /// Complete latest-version Extended Map members, including inactive, null-map,
    /// and unclassified rows. Written to the canonical refset companion stream.
    pub extended_map_members: Vec<ExtendedMapRefsetMember>,
    /// Complete latest-version Complex Map members.
    pub complex_map_members: Vec<ComplexMapRefsetMember>,
    /// Complete latest-version Attribute Value members.
    pub attribute_value_members: Vec<AttributeValueRefsetMember>,
    /// Historical associations (inactive-concept forwarding) from Association
    /// refsets. Only populated under `--refsets all`. Keyed by source SCTID is
    /// not possible (sources may be inactive and absent from `concepts`), so this
    /// is a flat list of `(source_id, association, target_id)`.
    pub history: Vec<(String, String, String)>,
}

impl Rf2Dataset {
    /// Load and aggregate every discovered RF2 file into the in-memory dataset.
    ///
    /// `include_inactive` controls whether inactive *concepts* are retained.
    /// When `false` (the default), inactive concept rows are dropped here at
    /// load time, so they never reach [`crate::builder::build_records`] and the
    /// common active-only path stays lean. When `true`, inactive concepts are
    /// kept (with `active = false`); their active descriptions, refset
    /// memberships and cross-maps attach as usual. Inactivating a concept does
    /// not inactivate its descriptions, so an inactive concept still carries a
    /// fully-populated FSN, preferred term and synonyms.
    ///
    /// The output gate in `build_records` also honours `include_inactive`, so
    /// callers must pass the same value to both: `build_records` may be stricter
    /// (drop inactive) but cannot resurrect concepts already dropped here.
    ///
    /// ```no_run
    /// use std::path::Path;
    /// use sct_rs::rf2::{discover_rf2_files, Rf2Dataset};
    /// let files = discover_rf2_files(Path::new("SnomedCT_Edition/Snapshot")).unwrap();
    /// let dataset = Rf2Dataset::load(&files, /* include_inactive = */ false).unwrap();
    /// // `dataset` now holds the parsed concepts, descriptions, relationships,
    /// // language refset, maps and history, ready for `build_records`.
    /// ```
    pub fn load(files: &Rf2Files, include_inactive: bool) -> Result<Self> {
        let mut concepts: HashMap<String, ConceptRow> = HashMap::new();
        let mut descriptions: HashMap<String, Vec<DescriptionRow>> = HashMap::new();
        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        let mut attributes: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
        let mut acceptability: HashMap<(String, String), Acceptability> = HashMap::new();
        let mut ctv3_maps: HashMap<String, Vec<String>> = HashMap::new();
        let mut ctv3_map_members_by_id: HashMap<String, SimpleMapRow> = HashMap::new();
        let read2_maps: HashMap<String, Vec<String>> = HashMap::new();
        let mut refset_members: HashMap<String, Vec<String>> = HashMap::new();
        let mut simple_refset_members_by_id: HashMap<String, SimpleRefsetRow> = HashMap::new();
        let mut extended_maps: HashMap<String, Vec<ExtendedMapRefsetMember>> = HashMap::new();
        let mut extended_map_members_by_id: HashMap<String, ExtendedMapRefsetMember> =
            HashMap::new();
        let mut complex_map_members_by_id: HashMap<String, ComplexMapRefsetMember> = HashMap::new();
        let mut attribute_value_members_by_id: HashMap<String, AttributeValueRefsetMember> =
            HashMap::new();
        let mut association_members_by_id: HashMap<String, AssociationRow> = HashMap::new();
        let mut history: Vec<(String, String, String)> = Vec::new();

        // --- Concepts ---
        // Active concepts are always retained; inactive concepts only under
        // `include_inactive`. Dropping them here (rather than only at the
        // builder's output gate) keeps the default path's memory and downstream
        // joins limited to the active substrate.
        for path in &files.concept_files {
            eprintln!("  Loading concepts from {}", path.display());
            stream_tsv(path, |record| {
                let row = concept_row(record);
                concepts.insert(row.id.clone(), row);
            })?;
        }
        if !include_inactive {
            concepts.retain(|_, concept| concept.active);
        }
        if include_inactive {
            // Count from the final map so layered editions (last-write-wins on a
            // repeated id) and any active/inactive restatement are reflected.
            let active = concepts.values().filter(|c| c.active).count();
            eprintln!(
                "  {} concepts ({} active, {} inactive)",
                concepts.len(),
                active,
                concepts.len() - active
            );
        } else {
            eprintln!("  {} active concepts", concepts.len());
        }
        crate::progress::debug_mem("concepts loaded");

        // --- Descriptions ---
        for path in &files.description_files {
            eprintln!("  Loading descriptions from {}", path.display());
            stream_tsv(path, |record| {
                // Filter on the raw record before allocating a row: inactive
                // descriptions and unknown concepts are the majority of rows
                // in a national edition and would otherwise be allocated only
                // to be dropped.
                if !is_active(record) {
                    return;
                }
                let concept_id = record.get(4).unwrap_or("");
                if !concepts.contains_key(concept_id) {
                    return;
                }
                let row = description_row(record);
                descriptions
                    .entry(row.concept_id.clone())
                    .or_default()
                    .push(row);
            })?;
        }

        crate::progress::debug_mem("descriptions loaded");

        // --- Relationships ---
        for path in &files.relationship_files {
            // Skip StatedRelationship files - use inferred only
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("sct2_StatedRelationship") {
                continue;
            }
            eprintln!("  Loading relationships from {}", path.display());
            stream_tsv(path, |record| {
                if !is_active(record) {
                    return;
                }
                let row = relationship_row(record);
                if row.type_id == IS_A {
                    parents
                        .entry(row.source_id)
                        .or_default()
                        .push(row.destination_id);
                } else {
                    attributes.entry(row.source_id).or_default().push((
                        row.type_id,
                        row.destination_id,
                        row.relationship_group,
                    ));
                }
            })?;
        }

        crate::progress::debug_mem("relationships loaded");

        // --- Language refsets ---
        for path in &files.lang_refset_files {
            eprintln!("  Loading language refset from {}", path.display());
            stream_tsv(path, |record| {
                if !is_active(record) {
                    return;
                }
                let row = lang_refset_row(record);
                let acc = if row.acceptability_id == PREFERRED {
                    Acceptability::Preferred
                } else {
                    Acceptability::Acceptable
                };
                // Keyed by (refset, description); last write wins per pair.
                acceptability.insert((row.refset_id, row.referenced_component_id), acc);
            })?;
        }
        eprintln!("  {} acceptability entries", acceptability.len());
        crate::progress::debug_mem("language refsets loaded");

        // --- CTV3 maps (refset 900000000000497000 within SimpleMap files) ---
        for path in &files.simple_map_files {
            eprintln!("  Loading simple maps from {}", path.display());
            stream_tsv(path, |record| {
                // Filter on the raw record: most SimpleMap rows are not CTV3.
                if record.get(4).unwrap_or("") != REFSET_CTV3_SIMPLE_MAP {
                    return;
                }
                if let Some(row) = simple_map_row(record) {
                    if files.simple_map_files.len() == 1 {
                        if row.active {
                            ctv3_maps
                                .entry(row.referenced_component_id)
                                .or_default()
                                .push(row.map_target);
                        }
                    } else {
                        ctv3_map_members_by_id.insert(row.id.clone(), row);
                    }
                }
            })?;
        }
        if files.simple_map_files.len() > 1 {
            for row in ctv3_map_members_by_id.into_values() {
                if row.active {
                    ctv3_maps
                        .entry(row.referenced_component_id)
                        .or_default()
                        .push(row.map_target);
                }
            }
        }
        eprintln!("  {} concepts with CTV3 mappings", ctv3_maps.len());
        eprintln!("  {} concepts with Read v2 mappings", read2_maps.len());

        // --- Generic simple refsets (concept-level membership) ---
        for path in &files.refset_files {
            eprintln!("  Loading simple refset from {}", path.display());
            stream_tsv(path, |record| {
                let row = simple_refset_row(record);
                if files.refset_files.len() == 1 {
                    if row.active && concepts.contains_key(&row.referenced_component_id) {
                        refset_members
                            .entry(row.referenced_component_id)
                            .or_default()
                            .push(row.refset_id);
                    }
                } else {
                    simple_refset_members_by_id.insert(row.id.clone(), row);
                }
            })?;
        }
        if files.refset_files.len() > 1 {
            for row in simple_refset_members_by_id.into_values() {
                // Drop rows whose referenced component isn't a retained concept -
                // simple refsets can also reference descriptions or relationships.
                if row.active && concepts.contains_key(&row.referenced_component_id) {
                    refset_members
                        .entry(row.referenced_component_id)
                        .or_default()
                        .push(row.refset_id);
                }
            }
        }
        eprintln!(
            "  {} concepts with simple refset memberships",
            refset_members.len()
        );

        // --- Extended Map members; `--refsets all` only ---
        for path in &files.extended_map_files {
            eprintln!("  Loading extended maps from {}", path.display());
            stream_extended_map(path, |row| {
                extended_map_members_by_id.insert(row.id.clone(), row);
                Ok(())
            })?;
        }
        let mut extended_map_members: Vec<_> = extended_map_members_by_id.into_values().collect();
        extended_map_members.sort_by(|a, b| a.id.cmp(&b.id));
        for row in &extended_map_members {
            if row.active
                && !row.map_target.trim().is_empty()
                && extended_map_system(&row.refset_id).is_some()
            {
                extended_maps
                    .entry(row.referenced_component_id.clone())
                    .or_default()
                    .push(row.clone());
            }
        }
        if !files.extended_map_files.is_empty() {
            eprintln!(
                "  {} Extended Map members; {} concepts with classified active maps",
                extended_map_members.len(),
                extended_maps.len(),
            );
        }

        // --- Complex Map members; `--refsets all` only ---
        for path in &files.complex_map_files {
            eprintln!("  Loading complex maps from {}", path.display());
            stream_checked_tsv(path, COMPLEX_MAP_HEADERS, |record| {
                let row = complex_map_row(record)?;
                complex_map_members_by_id.insert(row.id.clone(), row);
                Ok(())
            })?;
        }
        let mut complex_map_members: Vec<_> = complex_map_members_by_id.into_values().collect();
        complex_map_members.sort_by(|a, b| a.id.cmp(&b.id));
        if !files.complex_map_files.is_empty() {
            eprintln!("  {} Complex Map members", complex_map_members.len());
        }

        // --- Attribute Value members; `--refsets all` only ---
        for path in &files.attribute_value_files {
            eprintln!("  Loading attribute values from {}", path.display());
            stream_checked_tsv(path, ATTRIBUTE_VALUE_HEADERS, |record| {
                let row = attribute_value_row(record)?;
                attribute_value_members_by_id.insert(row.id.clone(), row);
                Ok(())
            })?;
        }
        let mut attribute_value_members: Vec<_> =
            attribute_value_members_by_id.into_values().collect();
        attribute_value_members.sort_by(|a, b| a.id.cmp(&b.id));
        if !files.attribute_value_files.is_empty() {
            eprintln!(
                "  {} Attribute Value members",
                attribute_value_members.len()
            );
        }

        // --- Historical associations (inactive forwarding); `--refsets all` only ---
        for path in &files.association_files {
            eprintln!("  Loading associations from {}", path.display());
            stream_tsv(path, |record| {
                let Some(row) = association_row(record) else {
                    return;
                };
                if files.association_files.len() == 1 {
                    if row.active {
                        history.push((
                            row.referenced_component_id,
                            association_name(&row.refset_id).to_string(),
                            row.target_component_id,
                        ));
                    }
                } else {
                    association_members_by_id.insert(row.id.clone(), row);
                }
            })?;
        }
        if files.association_files.len() > 1 {
            for row in association_members_by_id.into_values() {
                if row.active {
                    history.push((
                        row.referenced_component_id,
                        association_name(&row.refset_id).to_string(),
                        row.target_component_id,
                    ));
                }
            }
        }
        history.sort();
        if !files.association_files.is_empty() {
            eprintln!("  {} historical associations", history.len());
        }

        crate::progress::debug_mem("rf2 dataset loaded");

        Ok(Rf2Dataset {
            concepts,
            descriptions,
            parents,
            attributes,
            acceptability,
            ctv3_maps,
            read2_maps,
            refset_members,
            extended_maps,
            extended_map_members,
            complex_map_members,
            attribute_value_members,
            history,
        })
    }
}
