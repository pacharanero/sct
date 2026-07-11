// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

/// Builds the per-concept output records by joining RF2 data.
use anyhow::Result;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use crate::rf2::{
    Acceptability, Rf2Dataset, LANG_GB_ENGLISH, LANG_UK_CLINICAL, LANG_UK_DRUG, LANG_US_ENGLISH,
    TYPE_FSN, TYPE_SYNONYM,
};
use crate::schema::{ConceptRecord, ConceptRef, CrossMapEntry, SCHEMA_VERSION};

// ---------------------------------------------------------------------------
// Known top-level SNOMED CT hierarchy concept IDs (children of the root)
// ---------------------------------------------------------------------------
// Root: 138875005 SNOMED CT Concept
const SNOMED_ROOT: &str = "138875005";

// ---------------------------------------------------------------------------
// Attribute type label map (well-known SCTIDs → human-readable keys)
// ---------------------------------------------------------------------------
fn attribute_label(type_id: &str) -> &str {
    match type_id {
        "363698007" => "finding_site",
        "116676008" => "associated_morphology",
        "47429007" => "associated_with",
        "255234002" => "after",
        "246454002" => "occurrence",
        "246090004" => "associated_finding",
        "263502005" => "clinical_course",
        "246456000" => "episodicity",
        "363714003" => "interprets",
        "363713009" => "has_interpretation",
        "370135005" => "pathological_process",
        "363709002" => "associated_procedure",
        "405816004" => "procedure_site_direct",
        "405815000" => "procedure_site_indirect",
        "260686004" => "method",
        "405813007" => "procedure_site",
        "246093002" => "component",
        "704319004" => "inheres_in",
        "704318007" => "property_type",
        "704321009" => "characterizes",
        "370132008" => "scale_type",
        "246501002" => "technique",
        "411116001" => "has_dose_form",
        "127489000" => "has_active_ingredient",
        "762949000" => "has_precise_active_ingredient",
        _ => type_id,
    }
}

// ---------------------------------------------------------------------------
// Hierarchy builder
// ---------------------------------------------------------------------------

/// Walk up the IS-A graph from a concept to the root, returning the
/// ancestor chain from root down to (but not including) the concept itself.
///
/// We stop the upward traversal when we reach the SNOMED root, or after a
/// maximum depth to guard against cycles in malformed data.
fn ancestor_chain(
    concept_id: &str,
    parents_map: &HashMap<String, Vec<String>>,
    fsn_map: &HashMap<String, String>,
) -> Vec<String> {
    const MAX_DEPTH: usize = 20;

    // BFS upwards; we want a single representative path (take first parent).
    let mut path_ids: Vec<String> = Vec::new();
    let mut current = concept_id.to_string();

    for _ in 0..MAX_DEPTH {
        let p = match parents_map.get(&current).and_then(|v| v.first()) {
            Some(p) => p.clone(),
            None => break,
        };
        path_ids.push(p.clone());
        if p == SNOMED_ROOT {
            break;
        }
        current = p;
    }

    path_ids.reverse(); // root → concept

    path_ids
        .iter()
        .filter_map(|id| fsn_map.get(id).map(|fsn| label_for_path(fsn)))
        .collect()
}

/// Ordered language reference set ids to consult for a locale, most-preferred
/// first. Dialect lives in the refset id (GB vs US English; UK realm refsets),
/// not the description `languageCode`. Refsets absent from the loaded data
/// simply yield no match and are skipped, so the UK-first ordering for `en-GB`
/// is safe for International-only inputs too (it falls through to GB English).
///
/// ```
/// use sct_rs::builder::language_refset_priority;
/// // en-GB prefers UK Clinical, then dm+d, then International GB English.
/// assert_eq!(language_refset_priority("en-GB").len(), 3);
/// assert_eq!(language_refset_priority("en-US").len(), 1);
/// // Locale parsing is case- and separator-insensitive (`en_gb` == `en-GB`).
/// assert_eq!(language_refset_priority("en_gb"), language_refset_priority("en-GB"));
/// // A non-English locale has no known dialect refset.
/// assert!(language_refset_priority("fr").is_empty());
/// ```
pub fn language_refset_priority(locale: &str) -> Vec<&'static str> {
    match locale.to_ascii_lowercase().replace('_', "-").as_str() {
        // UK realm: UK Clinical overrides, then dm+d, then International GB English.
        "en-gb" => vec![LANG_UK_CLINICAL, LANG_UK_DRUG, LANG_GB_ENGLISH],
        "en-us" => vec![LANG_US_ENGLISH],
        // Other English (e.g. bare "en"): International GB then US English.
        l if l.starts_with("en") => vec![LANG_GB_ENGLISH, LANG_US_ENGLISH],
        // Non-English locale: no dialect refset known; fall back to any-preferred.
        _ => vec![],
    }
}

/// Strip the semantic tag from an FSN and return a borrowed slice.
/// "Myocardial infarction (disorder)" → "Myocardial infarction"
///
/// ```
/// use sct_rs::builder::strip_semantic_tag;
/// assert_eq!(
///     strip_semantic_tag("Myocardial infarction (disorder)"),
///     "Myocardial infarction"
/// );
/// // An FSN with no tag is returned unchanged.
/// assert_eq!(strip_semantic_tag("No tag here"), "No tag here");
/// ```
pub fn strip_semantic_tag(fsn: &str) -> &str {
    match fsn.rfind(" (") {
        Some(pos) => &fsn[..pos],
        None => fsn,
    }
}

/// Owned version of [`strip_semantic_tag`] for sites that need a `String`.
pub(crate) fn label_for_path(fsn: &str) -> String {
    strip_semantic_tag(fsn).to_string()
}

/// Return the top-level hierarchy name for a concept (e.g. "Clinical finding").
/// This is the FSN label of the child of the SNOMED root that is an ancestor
/// of this concept.
fn top_level_hierarchy(
    concept_id: &str,
    parents_map: &HashMap<String, Vec<String>>,
    fsn_map: &HashMap<String, String>,
) -> String {
    const MAX_DEPTH: usize = 20;
    let mut current = concept_id.to_string();

    for _ in 0..MAX_DEPTH {
        let p = match parents_map.get(&current).and_then(|v| v.first()) {
            Some(p) => p.clone(),
            None => break,
        };
        if p == SNOMED_ROOT {
            // `current` is the direct child of root → that's the top-level hierarchy
            return fsn_map
                .get(&current)
                .map(|fsn| label_for_path(fsn))
                .unwrap_or_default();
        }
        current = p;
    }

    String::new()
}

// ---------------------------------------------------------------------------
// Main builder
// ---------------------------------------------------------------------------

/// Flatten a loaded `Rf2Dataset` into the denormalised `ConceptRecord`s that
/// `sct` serialises as NDJSON. `locale` selects the preferred-term dialect (see
/// `language_refset_priority`); `include_inactive` must match the value passed to
/// `Rf2Dataset::load` (this gate may drop inactive concepts but never resurrect
/// ones already dropped at load time).
///
/// ```no_run
/// use std::path::Path;
/// use sct_rs::rf2::{discover_rf2_files, Rf2Dataset};
/// use sct_rs::builder::build_records;
/// let files = discover_rf2_files(Path::new("SnomedCT_Edition/Snapshot")).unwrap();
/// let dataset = Rf2Dataset::load(&files, false).unwrap();
/// let records = build_records(&dataset, "en-GB", false).unwrap();
/// println!("built {} concept records", records.len());
/// ```
pub fn build_records(
    dataset: &Rf2Dataset,
    locale: &str,
    include_inactive: bool,
) -> Result<Vec<ConceptRecord>> {
    // Precompute: concept_id -> FSN string (for parent labels, hierarchy paths)
    let mut fsn_map: HashMap<String, String> = HashMap::with_capacity(dataset.concepts.len());
    for (cid, descs) in &dataset.descriptions {
        if let Some(fsn_row) = descs.iter().find(|d| d.type_id == TYPE_FSN) {
            fsn_map.insert(cid.clone(), fsn_row.term.clone());
        }
    }

    // Precompute: concept_id -> children count
    let mut children_count: HashMap<String, usize> = HashMap::new();
    for parent_ids in dataset.parents.values() {
        for pid in parent_ids {
            *children_count.entry(pid.clone()).or_insert(0) += 1;
        }
    }

    // Preferred-term selection is dialect-aware. `--locale` maps to an ordered
    // list of language reference set ids (see `language_refset_priority`):
    // a concept's preferred term is the synonym marked Preferred in the
    // highest-priority refset that has an opinion for it, falling back to a
    // synonym preferred in any refset, then the FSN. This is what makes
    // `en-GB` ("Appendicectomy") differ from `en-US` ("Appendectomy") - the
    // dialect lives in the refset id, not the description's `languageCode`
    // (which is "en" for both).
    let priority = language_refset_priority(locale);

    // refset_id -> {description_id Preferred in that refset}, plus the union
    // (description_ids Preferred in *any* refset) for the fallback.
    let mut preferred_in: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut preferred_any: HashSet<&str> = HashSet::new();
    for ((refset, did), acc) in &dataset.acceptability {
        if *acc == Acceptability::Preferred {
            preferred_in
                .entry(refset.as_str())
                .or_default()
                .insert(did.as_str());
            preferred_any.insert(did.as_str());
        }
    }

    let mut records: Vec<ConceptRecord> = Vec::with_capacity(dataset.concepts.len());

    let mut concept_ids: Vec<&str> = dataset.concepts.keys().map(|s| s.as_str()).collect();
    concept_ids.sort(); // deterministic ordering

    // Count bar with ETA over the known concept total (the `ndjson` command
    // prints the "Building concept records..." breadcrumb above it).
    let bar = crate::progress::count_bar(concept_ids.len() as u64);

    for concept_id in concept_ids {
        bar.inc(1);
        let concept = &dataset.concepts[concept_id];

        if !include_inactive && !concept.active {
            continue;
        }

        let descs = dataset.descriptions.get(concept_id);

        // --- FSN ---
        let fsn = descs
            .and_then(|ds| ds.iter().find(|d| d.type_id == TYPE_FSN))
            .map(|d| d.term.clone())
            .unwrap_or_default();

        // --- Preferred term ---
        // 1. Synonym Preferred in the highest-priority locale refset
        // 2. Fall back: any synonym Preferred in any refset
        // 3. Fall back: FSN (semantic tag stripped)
        let preferred_term = {
            let candidates = descs.map(|ds| ds.as_slice()).unwrap_or(&[]);

            let by_priority = priority.iter().find_map(|refset| {
                let prefset = preferred_in.get(refset)?;
                candidates
                    .iter()
                    .find(|d| d.type_id == TYPE_SYNONYM && prefset.contains(d.id.as_str()))
            });

            let any_preferred = candidates
                .iter()
                .find(|d| d.type_id == TYPE_SYNONYM && preferred_any.contains(d.id.as_str()));

            by_priority
                .or(any_preferred)
                .map(|d| d.term.clone())
                .unwrap_or_else(|| label_for_path(&fsn))
        };

        // --- Synonyms (all active synonyms except the preferred term) ---
        let synonyms: Vec<String> = descs
            .map(|ds| {
                ds.iter()
                    .filter(|d| d.type_id == TYPE_SYNONYM && d.term != preferred_term)
                    .map(|d| d.term.clone())
                    .collect()
            })
            .unwrap_or_default();

        // --- Hierarchy ---
        let hierarchy = top_level_hierarchy(concept_id, &dataset.parents, &fsn_map);
        let mut path_labels = ancestor_chain(concept_id, &dataset.parents, &fsn_map);
        // Append this concept's own label
        path_labels.push(label_for_path(&fsn));

        // --- Parents ---
        let parents: Vec<ConceptRef> = dataset
            .parents
            .get(concept_id)
            .map(|ids| {
                let mut v: Vec<ConceptRef> = ids
                    .iter()
                    .map(|pid| ConceptRef {
                        id: pid.clone(),
                        fsn: fsn_map.get(pid).cloned().unwrap_or_default(),
                    })
                    .collect();
                v.sort_by(|a, b| a.id.cmp(&b.id));
                v
            })
            .unwrap_or_default();

        // --- Attributes (non-IS-A relationships) ---
        // Two representations from the same triples: `attr_map` is the
        // display-oriented, label-keyed view; `relationships` preserves the
        // type SCTID and group for ECL refinement (see spec/ecl.md §4).
        let mut relationships: Vec<crate::schema::Relationship> = Vec::new();
        let mut attr_map: IndexMap<String, Vec<ConceptRef>> = IndexMap::new();
        if let Some(attrs) = dataset.attributes.get(concept_id) {
            for (type_id, dest_id, group) in attrs {
                relationships.push(crate::schema::Relationship {
                    type_id: type_id.clone(),
                    destination_id: dest_id.clone(),
                    group: group.parse().unwrap_or(0),
                });
            }
            relationships.sort_by(|a, b| {
                (a.type_id.as_str(), a.destination_id.as_str(), a.group).cmp(&(
                    b.type_id.as_str(),
                    b.destination_id.as_str(),
                    b.group,
                ))
            });

            // Group by type_id, within each group sort by destination_id
            let mut by_type: HashMap<String, Vec<String>> = HashMap::new();
            for (type_id, dest_id, _group) in attrs {
                by_type
                    .entry(type_id.clone())
                    .or_default()
                    .push(dest_id.clone());
            }
            let mut type_ids: Vec<String> = by_type.keys().cloned().collect();
            type_ids.sort();
            for type_id in type_ids {
                let mut dests = by_type.remove(&type_id).unwrap();
                dests.sort();
                let refs: Vec<ConceptRef> = dests
                    .into_iter()
                    .map(|did| ConceptRef {
                        fsn: fsn_map.get(&did).cloned().unwrap_or_default(),
                        id: did,
                    })
                    .collect();
                attr_map.insert(attribute_label(&type_id).to_string(), refs);
            }
        }

        // --- CTV3 / Read v2 cross-map codes ---
        let mut ctv3_codes = dataset
            .ctv3_maps
            .get(concept_id)
            .cloned()
            .unwrap_or_default();
        ctv3_codes.sort();
        ctv3_codes.dedup();

        let mut read2_codes = dataset
            .read2_maps
            .get(concept_id)
            .cloned()
            .unwrap_or_default();
        read2_codes.sort();
        read2_codes.dedup();

        let mut refsets = dataset
            .refset_members
            .get(concept_id)
            .cloned()
            .unwrap_or_default();
        refsets.sort();
        refsets.dedup();

        // --- SNOMED CT -> ICD-10 / OPCS-4 cross-maps (ExtendedMap) ---
        let mut crossmaps: Vec<CrossMapEntry> = dataset
            .extended_maps
            .get(concept_id)
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        crate::rf2::extended_map_system(&r.refset_id).map(|sys| CrossMapEntry {
                            system: sys.to_string(),
                            code: r.map_target.clone(),
                            refset: r.refset_id.clone(),
                            group: r.map_group,
                            priority: r.map_priority,
                            rule: r.map_rule.clone(),
                            advice: r.map_advice.clone(),
                            correlation: r.correlation_id.clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        crossmaps.sort_by(|a, b| {
            (&a.system, &a.code, a.group, a.priority)
                .cmp(&(&b.system, &b.code, b.group, b.priority))
        });

        records.push(ConceptRecord {
            id: concept_id.to_string(),
            fsn,
            preferred_term,
            synonyms,
            hierarchy,
            hierarchy_path: path_labels,
            parents,
            children_count: *children_count.get(concept_id).unwrap_or(&0),
            active: concept.active,
            module: concept.module_id.clone(),
            effective_time: concept.effective_time.clone(),
            attributes: attr_map,
            ctv3_codes,
            read2_codes,
            refsets,
            relationships,
            crossmaps,
            schema_version: SCHEMA_VERSION,
        });
    }
    bar.finish_and_clear();

    Ok(records)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_for_path_strips_tag() {
        assert_eq!(label_for_path("Fever (finding)"), "Fever");
        assert_eq!(label_for_path("No tag"), "No tag");
        assert_eq!(label_for_path("Multi (word) (tag)"), "Multi (word)");
    }
}
