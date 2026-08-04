// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Build a `snomed.fst` artefact from `sct`'s canonical NDJSON.
//!
//! See `spec/commands/fst.md` §5.2. The builder is one-shot per SNOMED release. It
//! reads the per-concept NDJSON (the same input `sct sqlite` / `sct parquet`
//! consume), groups terms by their normalised form, and emits the single-file
//! container defined in [`crate::index::format`].

use anyhow::{Context, Result};
use fst::MapBuilder;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};

use crate::index::format::{self, Section};
use crate::index::normalise::{normalise, split_semantic_tag, tokenise};
use crate::provenance::{self, Provenance};
use crate::schema::ConceptRecord;

/// Summary statistics returned by [`build`], surfaced by the CLI.
#[derive(Debug, Default, Clone)]
pub struct BuildStats {
    pub concepts: usize,
    pub terms: usize,
    pub distinct_keys: usize,
    pub distinct_words: usize,
    pub semantic_tags: usize,
    pub bytes_written: u64,
    pub terms_included: bool,
}

/// Knobs for [`build_with_options`].
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Write the display side-tables (concept SCTID → preferred term). On by
    /// default so a standalone index can render labels. Turn off to shave the
    /// largest non-search sections when labels will be resolved elsewhere (e.g.
    /// from the SQLite `concepts` table). See `spec/commands/fst.md` §4.5.
    pub include_terms: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            include_terms: true,
        }
    }
}

/// Accumulates the in-memory grouping before the FSTs are serialised.
struct Accumulator {
    /// normalised term -> (semantic tag id, set of concept SCTIDs)
    keys: BTreeMap<String, KeyEntry>,
    /// normalised word token -> set of concept SCTIDs
    words: BTreeMap<String, BTreeSet<u64>>,
    /// concept SCTID -> original-case preferred term (for display)
    preferred: BTreeMap<u64, String>,
    /// semantic tag string -> tag id (1-based; 0 means "no tag")
    tag_ids: BTreeMap<String, u8>,
    /// tag id order, parallel to allocation, position i = tag string for id i+1
    tag_order: Vec<String>,
    concepts: usize,
    terms: usize,
}

#[derive(Default)]
struct KeyEntry {
    tag: u8,
    concepts: BTreeSet<u64>,
}

impl Accumulator {
    fn new() -> Self {
        Accumulator {
            keys: BTreeMap::new(),
            words: BTreeMap::new(),
            preferred: BTreeMap::new(),
            tag_ids: BTreeMap::new(),
            tag_order: Vec::new(),
            concepts: 0,
            terms: 0,
        }
    }

    /// Resolve (allocating on first sight) a tag id for a tag string. Caps at
    /// 255 distinct tags - SNOMED has well under 256 (see `spec/commands/fst.md` §4.2);
    /// anything beyond is folded to "no tag" rather than corrupting the byte.
    fn tag_id(&mut self, tag: &str) -> u8 {
        if let Some(&id) = self.tag_ids.get(tag) {
            return id;
        }
        if self.tag_order.len() >= 255 {
            return 0;
        }
        let id = (self.tag_order.len() + 1) as u8;
        self.tag_order.push(tag.to_string());
        self.tag_ids.insert(tag.to_string(), id);
        id
    }

    /// Index one term against a concept. `tag` is the FSN's semantic tag, or
    /// `None` for preferred terms / synonyms.
    fn add_term(&mut self, concept_id: u64, raw_term: &str, tag: Option<&str>) {
        let key = normalise(raw_term);
        if key.is_empty() {
            return;
        }
        self.terms += 1;
        let tag_id = tag.map(|t| self.tag_id(t)).unwrap_or(0);

        let entry = self.keys.entry(key.clone()).or_default();
        entry.concepts.insert(concept_id);
        // Prefer a real (FSN-derived) tag over the "no tag" default. First
        // non-zero tag to claim the key wins; collisions across differently
        // tagged concepts are rare and tag filtering is best-effort in v0.
        if entry.tag == 0 && tag_id != 0 {
            entry.tag = tag_id;
        }

        for tok in tokenise(&key) {
            self.words
                .entry(tok.to_string())
                .or_default()
                .insert(concept_id);
        }
    }

    fn add_record(&mut self, rec: &ConceptRecord) {
        let Ok(concept_id) = rec.id.parse::<u64>() else {
            // Non-numeric id should never occur in real RF2-derived NDJSON;
            // skip defensively rather than abort the whole build.
            return;
        };
        self.concepts += 1;

        // FSN: strip and record the semantic tag.
        let (fsn_term, tag) = split_semantic_tag(&rec.fsn);
        self.add_term(concept_id, fsn_term, tag);

        // Preferred term: a key, and the display string for this concept.
        self.add_term(concept_id, &rec.preferred_term, None);
        self.preferred
            .entry(concept_id)
            .or_insert_with(|| rec.preferred_term.clone());

        for syn in &rec.synonyms {
            self.add_term(concept_id, syn, None);
        }
    }
}

/// Read NDJSON from `reader`, build the index with default options, and write
/// the container to `out`.
pub fn build<R: BufRead, W: Write>(reader: R, out: &mut W) -> Result<BuildStats> {
    build_with_options(reader, out, &BuildOptions::default())
}

/// As [`build`], with explicit [`BuildOptions`].
pub fn build_with_options<R: BufRead, W: Write>(
    reader: R,
    out: &mut W,
    opts: &BuildOptions,
) -> Result<BuildStats> {
    let mut acc = Accumulator::new();
    let mut prov: Option<Provenance> = None;
    let mut fingerprint = provenance::ContentFingerprint::new();

    for line in reader.lines() {
        let line = line.context("reading NDJSON input")?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(p) = provenance::try_parse_ndjson_line(&line) {
            prov = Some(p);
            continue;
        }
        let rec: ConceptRecord =
            serde_json::from_str(&line).context("parsing NDJSON concept record")?;
        fingerprint.update(line.as_bytes());
        acc.add_record(&rec);
    }

    provenance::verify_or_set_content_fingerprint(&mut prov, fingerprint.finish())?;

    serialise(acc, prov, out, opts)
}

/// Serialise the accumulated maps into the container.
fn serialise<W: Write>(
    acc: Accumulator,
    prov: Option<Provenance>,
    out: &mut W,
    opts: &BuildOptions,
) -> Result<BuildStats> {
    // --- descriptions.fst + postings ---
    let mut postings: Vec<u8> = Vec::new();
    let mut desc_builder = MapBuilder::memory();
    for (key, entry) in &acc.keys {
        let offset = postings.len() as u64;
        write_posting(&mut postings, &entry.concepts);
        let value = format::pack(entry.tag, offset);
        desc_builder
            .insert(key.as_bytes(), value)
            .with_context(|| format!("inserting description key {key:?}"))?;
    }
    let descriptions = desc_builder
        .into_inner()
        .context("finalising descriptions FST")?;

    // --- words.fst + word_postings ---
    let mut word_postings: Vec<u8> = Vec::new();
    let mut word_builder = MapBuilder::memory();
    for (word, concepts) in &acc.words {
        let offset = word_postings.len() as u64;
        write_posting(&mut word_postings, concepts);
        let value = format::pack(0, offset);
        word_builder
            .insert(word.as_bytes(), value)
            .with_context(|| format!("inserting word key {word:?}"))?;
    }
    let words = word_builder.into_inner().context("finalising words FST")?;

    // --- terms_index + terms_text (concept SCTID -> preferred term) ---
    // Optional: when omitted, both sections are an empty (count = 0) table, and
    // the reader resolves no labels - callers supply display text elsewhere.
    let mut terms_index: Vec<u8> = Vec::new();
    let mut terms_text: Vec<u8> = Vec::new();
    let term_count = if opts.include_terms {
        acc.preferred.len()
    } else {
        0
    };
    terms_index.extend_from_slice(&(term_count as u32).to_le_bytes());
    if opts.include_terms {
        for (&sctid, term) in &acc.preferred {
            let off = terms_text.len() as u32;
            let bytes = term.as_bytes();
            terms_text.extend_from_slice(bytes);
            terms_index.extend_from_slice(&sctid.to_le_bytes());
            terms_index.extend_from_slice(&off.to_le_bytes());
            terms_index.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        }
    }

    // --- tag_table (JSON; index 0 = "no tag") ---
    let mut tag_table: Vec<String> = Vec::with_capacity(acc.tag_order.len() + 1);
    tag_table.push(String::new());
    tag_table.extend(acc.tag_order.iter().cloned());
    let tag_table_json = serde_json::to_vec(&tag_table).context("encoding tag table")?;

    // --- provenance (JSON, or empty if absent) ---
    let prov_json = match &prov {
        Some(p) => serde_json::to_vec(p).context("encoding provenance")?,
        None => Vec::new(),
    };

    let sections = [
        Section {
            name: format::SEC_DESCRIPTIONS,
            bytes: &descriptions,
        },
        Section {
            name: format::SEC_POSTINGS,
            bytes: &postings,
        },
        Section {
            name: format::SEC_WORDS,
            bytes: &words,
        },
        Section {
            name: format::SEC_WORD_POSTINGS,
            bytes: &word_postings,
        },
        Section {
            name: format::SEC_TERMS_INDEX,
            bytes: &terms_index,
        },
        Section {
            name: format::SEC_TERMS_TEXT,
            bytes: &terms_text,
        },
        Section {
            name: format::SEC_TAG_TABLE,
            bytes: &tag_table_json,
        },
        Section {
            name: format::SEC_PROVENANCE,
            bytes: &prov_json,
        },
    ];

    let bytes_written: u64 = section_total(&sections);
    format::write_container(out, &sections).context("writing container")?;

    Ok(BuildStats {
        concepts: acc.concepts,
        terms: acc.terms,
        distinct_keys: acc.keys.len(),
        distinct_words: acc.words.len(),
        semantic_tags: acc.tag_order.len(),
        bytes_written,
        terms_included: opts.include_terms,
    })
}

/// Append a posting list as `uvarint(len)` followed by delta-encoded SCTIDs,
/// each delta a uvarint. A `BTreeSet` iterates ascending, so deltas are positive
/// and small for dense runs - the whole point of the encoding. The ascending
/// order is also what the merge-intersection in `query` relies on.
fn write_posting(buf: &mut Vec<u8>, concepts: &BTreeSet<u64>) {
    format::write_uvarint(buf, concepts.len() as u64);
    let mut prev = 0u64;
    for &cid in concepts {
        format::write_uvarint(buf, cid - prev);
        prev = cid;
    }
}

fn section_total(sections: &[Section<'_>]) -> u64 {
    // Header + section bytes + a rough TOC/footer estimate. Exact size is the
    // file length; this is only a pre-write hint, recomputed by the caller from
    // the real file when it matters.
    sections.iter().map(|s| s.bytes.len() as u64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_container_from_records() {
        let ndjson = r#"{"id":"22298006","fsn":"Myocardial infarction (disorder)","preferred_term":"Myocardial infarction","synonyms":["Heart attack","MI"],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":3}
{"id":"73211009","fsn":"Diabetes mellitus (disorder)","preferred_term":"Diabetes mellitus","synonyms":["Diabetes"],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":3}"#;
        let mut out = Vec::new();
        let stats = build(std::io::Cursor::new(ndjson), &mut out).unwrap();
        assert_eq!(stats.concepts, 2);
        // 2 FSNs + 2 PTs + 3 synonyms = 7 terms
        assert_eq!(stats.terms, 7);
        assert_eq!(stats.semantic_tags, 1); // "disorder"
                                            // The container parses back.
        let toc = format::Toc::parse(&out).unwrap();
        assert!(toc.require(format::SEC_DESCRIPTIONS).is_ok());
    }

    #[test]
    fn verifies_fingerprint_without_reserialising_future_fields() {
        let record = r#"{"id":"22298006","fsn":"Myocardial infarction (disorder)","preferred_term":"Myocardial infarction","synonyms":[],"hierarchy":"Clinical finding","hierarchy_path":[],"parents":[],"children_count":0,"active":true,"module":"x","effective_time":"","attributes":{},"schema_version":6,"future_field":"preserve in fingerprint"}"#;
        let mut fingerprint = provenance::ContentFingerprint::new();
        fingerprint.update(record.as_bytes());
        let provenance = Provenance {
            type_tag: provenance::NDJSON_TYPE_TAG.to_string(),
            edition_label: "Synthetic".to_string(),
            release_date: "2026-01-01".to_string(),
            release_id: "synthetic-20260101".to_string(),
            content_fingerprint: Some(fingerprint.finish()),
            companions: vec![],
            source_paths: vec![],
            sct_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let ndjson = format!("{}\n{record}", serde_json::to_string(&provenance).unwrap());

        build(std::io::Cursor::new(ndjson), &mut Vec::new()).unwrap();
    }
}
