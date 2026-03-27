// Row fields kept for future Layer 2 consumers.
#![allow(dead_code)]

/// RF2 file discovery and parsing.
///
/// RF2 Snapshot files are TSV files with a header row.
/// We locate them by filename pattern within the release directory tree.
use anyhow::{Context, Result};
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
    pub type_id: String,   // 900000000000003001 = FSN, 900000000000013009 = synonym
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
    pub type_id: String,   // 116680003 = Is a
    pub characteristic_type_id: String,
    pub modifier_id: String,
}

/// A row from a language refset file (der2_cRefset_Language_Snapshot_*.txt)
#[derive(Debug)]
pub struct LangRefsetRow {
    pub active: bool,
    pub referenced_component_id: String, // description id
    pub acceptability_id: String, // 900000000000548007 = preferred, 900000000000549004 = acceptable
}

// ---------------------------------------------------------------------------
// SNOMED CT type_id constants
// ---------------------------------------------------------------------------
pub const TYPE_FSN: &str = "900000000000003001";
pub const TYPE_SYNONYM: &str = "900000000000013009";
pub const IS_A: &str = "116680003";
pub const PREFERRED: &str = "900000000000548007";

// ---------------------------------------------------------------------------
// RF2 file discovery
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct Rf2Files {
    pub concept_files: Vec<PathBuf>,
    pub description_files: Vec<PathBuf>,
    pub relationship_files: Vec<PathBuf>,
    pub lang_refset_files: Vec<PathBuf>,
}

/// Walk the RF2 directory tree and collect snapshot TSV paths by type.
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

        if name.starts_with("sct2_Concept_") && name.contains("Snapshot") && name.ends_with(".txt") {
            files.concept_files.push(path.to_path_buf());
        } else if name.starts_with("sct2_Description_") && name.contains("Snapshot") && name.ends_with(".txt") {
            files.description_files.push(path.to_path_buf());
        } else if (name.starts_with("sct2_Relationship_") || name.starts_with("sct2_StatedRelationship_"))
            && name.contains("Snapshot")
            && name.ends_with(".txt")
        {
            files.relationship_files.push(path.to_path_buf());
        } else if name.starts_with("der2_cRefset_Language") && name.contains("Snapshot") && name.ends_with(".txt") {
            files.lang_refset_files.push(path.to_path_buf());
        }
    }

    files.concept_files.sort();
    files.description_files.sort();
    files.relationship_files.sort();
    files.lang_refset_files.sort();

    Ok(files)
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

fn tsv_reader(path: &Path) -> Result<csv::Reader<std::fs::File>> {
    let rdr = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .has_headers(true)
        .flexible(false)
        .from_path(path)
        .with_context(|| format!("opening {}", path.display()))?;
    Ok(rdr)
}

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
            referenced_component_id: record.get(5).unwrap_or("").to_string(),
            acceptability_id: record.get(6).unwrap_or("").to_string(),
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
    /// concept_id -> ConceptRow (active only)
    pub concepts: HashMap<String, ConceptRow>,
    /// concept_id -> Vec<DescriptionRow> (active only)
    pub descriptions: HashMap<String, Vec<DescriptionRow>>,
    /// concept_id -> Vec<parent_id> (active IS-A relationships only)
    pub parents: HashMap<String, Vec<String>>,
    /// concept_id -> Vec<(type_id, destination_id, group)> for non-IS-A active attributes
    pub attributes: HashMap<String, Vec<(String, String, String)>>,
    /// description_id -> Acceptability (from lang refset)
    pub acceptability: HashMap<String, Acceptability>,
}

impl Rf2Dataset {
    pub fn load(files: &Rf2Files) -> Result<Self> {
        let mut concepts: HashMap<String, ConceptRow> = HashMap::new();
        let mut descriptions: HashMap<String, Vec<DescriptionRow>> = HashMap::new();
        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        let mut attributes: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
        let mut acceptability: HashMap<String, Acceptability> = HashMap::new();

        // --- Concepts ---
        for path in &files.concept_files {
            eprintln!("  Loading concepts from {}", path.display());
            for row in parse_concepts(path)? {
                if row.active {
                    concepts.insert(row.id.clone(), row);
                }
            }
        }
        eprintln!("  {} active concepts", concepts.len());

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
            // Skip StatedRelationship files — use inferred only
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
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
                    attributes
                        .entry(row.source_id.clone())
                        .or_default()
                        .push((row.type_id, row.destination_id, row.relationship_group));
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
                    // Last write wins (later rows in file take precedence)
                    acceptability.insert(row.referenced_component_id, acc);
                }
            }
        }
        eprintln!("  {} acceptability entries", acceptability.len());

        Ok(Rf2Dataset {
            concepts,
            descriptions,
            parents,
            attributes,
            acceptability,
        })
    }
}
