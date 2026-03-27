/// Builds the per-concept output records by joining RF2 data.
use anyhow::Result;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use crate::rf2::{Acceptability, Rf2Dataset, TYPE_FSN, TYPE_SYNONYM};
use crate::schema::{ConceptRecord, ConceptRef, SCHEMA_VERSION};

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
        "47429007"  => "associated_with",
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
        _           => type_id,
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

/// Strip the semantic tag from an FSN for display in hierarchy paths.
/// "Myocardial infarction (disorder)" → "Myocardial infarction"
fn label_for_path(fsn: &str) -> String {
    if let Some(pos) = fsn.rfind(" (") {
        fsn[..pos].to_string()
    } else {
        fsn.to_string()
    }
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
    for (_child_id, parent_ids) in &dataset.parents {
        for pid in parent_ids {
            *children_count.entry(pid.clone()).or_insert(0) += 1;
        }
    }

    // Determine which lang refset files match the requested locale.
    // The locale string ("en-GB", "en-US", "en") is matched against the
    // language_code field on descriptions. For preferred-term selection we use
    // the acceptability map (already filtered to the right refset files by the
    // caller or by locale matching in rf2.rs).
    //
    // Strategy: for each concept, prefer descriptions whose description_id is
    // marked Preferred in the acceptability map AND whose language_code starts
    // with the locale language tag.
    let locale_lang = locale.split('-').next().unwrap_or("en");

    // Build the set of description IDs that are Preferred in the loaded refset
    let preferred_desc_ids: HashSet<&str> = dataset
        .acceptability
        .iter()
        .filter_map(|(did, acc)| {
            if *acc == Acceptability::Preferred {
                Some(did.as_str())
            } else {
                None
            }
        })
        .collect();

    let mut records: Vec<ConceptRecord> = Vec::with_capacity(dataset.concepts.len());

    let mut concept_ids: Vec<&str> = dataset.concepts.keys().map(|s| s.as_str()).collect();
    concept_ids.sort(); // deterministic ordering

    for concept_id in concept_ids {
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
        // 1. Active synonym, locale language, preferred in acceptability map
        // 2. Fall back: any active synonym with preferred acceptability
        // 3. Fall back: FSN
        let preferred_term = {
            let candidates = descs.map(|ds| ds.as_slice()).unwrap_or(&[]);

            let by_locale_preferred = candidates.iter().find(|d| {
                d.type_id == TYPE_SYNONYM
                    && d.language_code.starts_with(locale_lang)
                    && preferred_desc_ids.contains(d.id.as_str())
            });

            let any_preferred = candidates.iter().find(|d| {
                d.type_id == TYPE_SYNONYM && preferred_desc_ids.contains(d.id.as_str())
            });

            by_locale_preferred
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
        let mut attr_map: IndexMap<String, Vec<ConceptRef>> = IndexMap::new();
        if let Some(attrs) = dataset.attributes.get(concept_id) {
            // Group by type_id, within each group sort by destination_id
            let mut by_type: HashMap<String, Vec<String>> = HashMap::new();
            for (type_id, dest_id, _group) in attrs {
                by_type.entry(type_id.clone()).or_default().push(dest_id.clone());
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
            schema_version: SCHEMA_VERSION,
        });
    }

    Ok(records)
}
