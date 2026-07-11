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
    pub active: bool,
    pub refset_id: String,
    pub referenced_component_id: String,
}

/// A row from a SNOMED CT → ICD-10 / OPCS-4 ExtendedMap reference set file
/// (`der2_i*Refset_ExtendedMap*Snapshot*.txt`). The target classification is
/// identified by `refset_id` (see [`extended_map_system`]).
///
/// Columns (TSV): id effectiveTime active moduleId refsetId referencedComponentId
/// mapGroup mapPriority mapRule mapAdvice mapTarget correlationId mapBlock
#[derive(Debug)]
pub struct ExtendedMapRow {
    pub active: bool,
    pub refset_id: String,
    pub referenced_component_id: String, // SNOMED CT source SCTID
    pub map_group: u32,
    pub map_priority: u32,
    pub map_rule: String,
    pub map_advice: String,
    pub map_target: String, // ICD-10 / OPCS-4 code
    pub correlation_id: String,
}

/// A row from a historical Association reference set file
/// (`der2_cRefset_Association*Snapshot*.txt`). Maps an inactivated concept to a
/// related/replacement concept; `refset_id` is the association type (see
/// [`association_name`]).
///
/// Columns (TSV): id effectiveTime active moduleId refsetId referencedComponentId targetComponentId
#[derive(Debug)]
pub struct AssociationRow {
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
/// (`icd10` | `opcs4`). Seeded with the known UK + International maps; a row
/// whose refset is not listed here is skipped (and counted) by the loader.
/// Refinable as new map refsets appear. See `spec/cross-terminology-mapping.md`.
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
/// `association` value in `concept_history`. Unknown ids fall back to the raw id.
///
/// ```
/// use sct_rs::rf2::association_name;
/// assert_eq!(association_name("900000000000526001"), "replaced_by");
/// assert_eq!(association_name("734138000"), "partially_equivalent_to");
/// // An unrecognised refset id is returned verbatim.
/// assert_eq!(association_name("111"), "111");
/// ```
pub fn association_name(refset_id: &str) -> &str {
    match refset_id {
        "900000000000526001" => "replaced_by",
        "900000000000527005" => "same_as",
        "900000000000523009" => "possibly_equivalent_to",
        "900000000000524003" => "moved_to",
        "900000000000525002" => "moved_from",
        "900000000000528000" => "was_a",
        "900000000000530003" => "alternative",
        "900000000000531004" => "refers_to",
        "734138000" => "partially_equivalent_to",
        other => other,
    }
}

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
    let mut rdr = tsv_reader(path)?;
    let mut rows = Vec::new();

    for result in rdr.records() {
        let record = result.with_context(|| format!("reading {}", path.display()))?;
        // id effectiveTime active moduleId definitionStatusId
        let active = record.get(2).unwrap_or("0") == "1";
        rows.push(ConceptRow {
            id: record.get(0).unwrap_or("").to_string(),
            effective_time: record.get(1).unwrap_or("").to_string(),
            active,
            module_id: record.get(3).unwrap_or("").to_string(),
            definition_status_id: record.get(4).unwrap_or("").to_string(),
        });
    }
    Ok(rows)
}

pub fn parse_descriptions(path: &Path) -> Result<Vec<DescriptionRow>> {
    let mut rdr = tsv_reader(path)?;
    let mut rows = Vec::new();

    for result in rdr.records() {
        let record = result.with_context(|| format!("reading {}", path.display()))?;
        // id effectiveTime active moduleId conceptId languageCode typeId term caseSignificanceId
        let active = record.get(2).unwrap_or("0") == "1";
        rows.push(DescriptionRow {
            id: record.get(0).unwrap_or("").to_string(),
            effective_time: record.get(1).unwrap_or("").to_string(),
            active,
            concept_id: record.get(4).unwrap_or("").to_string(),
            language_code: record.get(5).unwrap_or("").to_string(),
            type_id: record.get(6).unwrap_or("").to_string(),
            term: record.get(7).unwrap_or("").to_string(),
            case_significance_id: record.get(8).unwrap_or("").to_string(),
        });
    }
    Ok(rows)
}

pub fn parse_relationships(path: &Path) -> Result<Vec<RelationshipRow>> {
    let mut rdr = tsv_reader(path)?;
    let mut rows = Vec::new();

    for result in rdr.records() {
        let record = result.with_context(|| format!("reading {}", path.display()))?;
        // id effectiveTime active moduleId sourceId destinationId relationshipGroup typeId characteristicTypeId modifierId
        let active = record.get(2).unwrap_or("0") == "1";
        rows.push(RelationshipRow {
            id: record.get(0).unwrap_or("").to_string(),
            effective_time: record.get(1).unwrap_or("").to_string(),
            active,
            source_id: record.get(4).unwrap_or("").to_string(),
            destination_id: record.get(5).unwrap_or("").to_string(),
            relationship_group: record.get(6).unwrap_or("").to_string(),
            type_id: record.get(7).unwrap_or("").to_string(),
            characteristic_type_id: record.get(8).unwrap_or("").to_string(),
            modifier_id: record.get(9).unwrap_or("").to_string(),
        });
    }
    Ok(rows)
}

pub fn parse_lang_refset(path: &Path) -> Result<Vec<LangRefsetRow>> {
    let mut rdr = tsv_reader(path)?;
    let mut rows = Vec::new();

    for result in rdr.records() {
        let record = result.with_context(|| format!("reading {}", path.display()))?;
        // id effectiveTime active moduleId refsetId referencedComponentId acceptabilityId
        let active = record.get(2).unwrap_or("0") == "1";
        rows.push(LangRefsetRow {
            active,
            refset_id: record.get(4).unwrap_or("").to_string(),
            referenced_component_id: record.get(5).unwrap_or("").to_string(),
            acceptability_id: record.get(6).unwrap_or("").to_string(),
        });
    }
    Ok(rows)
}

/// Parse a generic concept-level simple refset file.
///
/// Columns: id effectiveTime active moduleId refsetId referencedComponentId
pub fn parse_simple_refset(path: &Path) -> Result<Vec<SimpleRefsetRow>> {
    let mut rdr = tsv_reader(path)?;
    let mut rows = Vec::new();

    for result in rdr.records() {
        let record = result.with_context(|| format!("reading {}", path.display()))?;
        let active = record.get(2).unwrap_or("0") == "1";
        rows.push(SimpleRefsetRow {
            active,
            refset_id: record.get(4).unwrap_or("").to_string(),
            referenced_component_id: record.get(5).unwrap_or("").to_string(),
        });
    }
    Ok(rows)
}

/// Parse a simple map reference set file.
///
/// Columns: id effectiveTime active moduleId refsetId referencedComponentId mapTarget
pub fn parse_simple_map(path: &Path) -> Result<Vec<SimpleMapRow>> {
    let mut rdr = tsv_reader(path)?;
    let mut rows = Vec::new();

    for result in rdr.records() {
        let record = result.with_context(|| format!("reading {}", path.display()))?;
        let active = record.get(2).unwrap_or("0") == "1";
        let map_target = record.get(6).unwrap_or("").trim().to_string();
        if map_target.is_empty() {
            continue;
        }
        rows.push(SimpleMapRow {
            active,
            refset_id: record.get(4).unwrap_or("").to_string(),
            referenced_component_id: record.get(5).unwrap_or("").to_string(),
            map_target,
        });
    }
    Ok(rows)
}

/// Parse a SNOMED CT ExtendedMap reference set file (ICD-10 / OPCS-4 maps).
///
/// Columns: id effectiveTime active moduleId refsetId referencedComponentId
/// mapGroup mapPriority mapRule mapAdvice mapTarget correlationId mapBlock
pub fn parse_extended_map(path: &Path) -> Result<Vec<ExtendedMapRow>> {
    let mut rdr = tsv_reader(path)?;
    let mut rows = Vec::new();
    for result in rdr.records() {
        let record = result.with_context(|| format!("reading {}", path.display()))?;
        let map_target = record.get(10).unwrap_or("").trim().to_string();
        if map_target.is_empty() {
            continue;
        }
        rows.push(ExtendedMapRow {
            active: record.get(2).unwrap_or("0") == "1",
            refset_id: record.get(4).unwrap_or("").to_string(),
            referenced_component_id: record.get(5).unwrap_or("").to_string(),
            map_group: record.get(6).and_then(|s| s.parse().ok()).unwrap_or(0),
            map_priority: record.get(7).and_then(|s| s.parse().ok()).unwrap_or(0),
            map_rule: record.get(8).unwrap_or("").to_string(),
            map_advice: record.get(9).unwrap_or("").to_string(),
            map_target,
            correlation_id: record.get(11).unwrap_or("").to_string(),
        });
    }
    Ok(rows)
}

/// Parse a historical Association reference set file (concept history).
///
/// Columns: id effectiveTime active moduleId refsetId referencedComponentId targetComponentId
pub fn parse_association(path: &Path) -> Result<Vec<AssociationRow>> {
    let mut rdr = tsv_reader(path)?;
    let mut rows = Vec::new();
    for result in rdr.records() {
        let record = result.with_context(|| format!("reading {}", path.display()))?;
        let target = record.get(6).unwrap_or("").trim().to_string();
        if target.is_empty() {
            continue;
        }
        rows.push(AssociationRow {
            active: record.get(2).unwrap_or("0") == "1",
            refset_id: record.get(4).unwrap_or("").to_string(),
            referenced_component_id: record.get(5).unwrap_or("").to_string(),
            target_component_id: target,
        });
    }
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
    /// Only populated when ExtendedMap files were supplied (`--refsets all`).
    pub extended_maps: HashMap<String, Vec<ExtendedMapRow>>,
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
        let read2_maps: HashMap<String, Vec<String>> = HashMap::new();
        let mut refset_members: HashMap<String, Vec<String>> = HashMap::new();
        let mut extended_maps: HashMap<String, Vec<ExtendedMapRow>> = HashMap::new();
        let mut history: Vec<(String, String, String)> = Vec::new();

        // --- Concepts ---
        // Active concepts are always retained; inactive concepts only under
        // `include_inactive`. Dropping them here (rather than only at the
        // builder's output gate) keeps the default path's memory and downstream
        // joins limited to the active substrate.
        for path in &files.concept_files {
            eprintln!("  Loading concepts from {}", path.display());
            for row in parse_concepts(path)? {
                if row.active || include_inactive {
                    concepts.insert(row.id.clone(), row);
                }
            }
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

        // --- Descriptions ---
        for path in &files.description_files {
            eprintln!("  Loading descriptions from {}", path.display());
            for row in parse_descriptions(path)? {
                if row.active && concepts.contains_key(&row.concept_id) {
                    descriptions
                        .entry(row.concept_id.clone())
                        .or_default()
                        .push(row);
                }
            }
        }

        // --- Relationships ---
        for path in &files.relationship_files {
            // Skip StatedRelationship files - use inferred only
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("sct2_StatedRelationship") {
                continue;
            }
            eprintln!("  Loading relationships from {}", path.display());
            for row in parse_relationships(path)? {
                if !row.active {
                    continue;
                }
                if row.type_id == IS_A {
                    parents
                        .entry(row.source_id.clone())
                        .or_default()
                        .push(row.destination_id.clone());
                } else {
                    attributes.entry(row.source_id.clone()).or_default().push((
                        row.type_id,
                        row.destination_id,
                        row.relationship_group,
                    ));
                }
            }
        }

        // --- Language refsets ---
        for path in &files.lang_refset_files {
            eprintln!("  Loading language refset from {}", path.display());
            for row in parse_lang_refset(path)? {
                if row.active {
                    let acc = if row.acceptability_id == PREFERRED {
                        Acceptability::Preferred
                    } else {
                        Acceptability::Acceptable
                    };
                    // Keyed by (refset, description); last write wins per pair.
                    acceptability.insert((row.refset_id, row.referenced_component_id), acc);
                }
            }
        }
        eprintln!("  {} acceptability entries", acceptability.len());

        // --- CTV3 maps (refset 900000000000497000 within SimpleMap files) ---
        for path in &files.simple_map_files {
            eprintln!("  Loading simple maps from {}", path.display());
            for row in parse_simple_map(path)? {
                if row.active && row.refset_id == REFSET_CTV3_SIMPLE_MAP {
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
            for row in parse_simple_refset(path)? {
                if !row.active {
                    continue;
                }
                // Drop rows whose referenced component isn't a known active
                // concept - simple refsets can reference descriptions or
                // relationships, which we don't model here.
                if !concepts.contains_key(&row.referenced_component_id) {
                    continue;
                }
                refset_members
                    .entry(row.referenced_component_id)
                    .or_default()
                    .push(row.refset_id);
            }
        }
        eprintln!(
            "  {} concepts with simple refset memberships",
            refset_members.len()
        );

        // --- ExtendedMap (SNOMED CT -> ICD-10 / OPCS-4); `--refsets all` only ---
        let mut skipped_map_rows = 0usize;
        for path in &files.extended_map_files {
            eprintln!("  Loading extended maps from {}", path.display());
            for row in parse_extended_map(path)? {
                if !row.active {
                    continue;
                }
                if extended_map_system(&row.refset_id).is_none() {
                    skipped_map_rows += 1;
                    continue;
                }
                extended_maps
                    .entry(row.referenced_component_id.clone())
                    .or_default()
                    .push(row);
            }
        }
        if !files.extended_map_files.is_empty() {
            eprintln!(
                "  {} concepts with ICD-10/OPCS-4 maps ({} rows from unrecognised map refsets skipped)",
                extended_maps.len(),
                skipped_map_rows
            );
        }

        // --- Historical associations (inactive forwarding); `--refsets all` only ---
        for path in &files.association_files {
            eprintln!("  Loading associations from {}", path.display());
            for row in parse_association(path)? {
                if !row.active {
                    continue;
                }
                history.push((
                    row.referenced_component_id,
                    association_name(&row.refset_id).to_string(),
                    row.target_component_id,
                ));
            }
        }
        if !files.association_files.is_empty() {
            eprintln!("  {} historical associations", history.len());
        }

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
            history,
        })
    }
}
