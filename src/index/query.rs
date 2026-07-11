// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Read side of the FST index: open a `snomed.fst` artefact and query it.
//!
//! The whole file is mmap'd once. The two FSTs borrow sub-ranges of that mmap
//! through [`ArcSlice`] (an `Arc<Mmap>` plus a byte range), which sidesteps the
//! self-referential-struct problem while keeping every lookup allocation-free
//! on the hot path apart from the result strings.

use anyhow::{Context, Result};
use fst::automaton::{Automaton, Levenshtein, Str};
use fst::{IntoStreamer, Map, Streamer};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use crate::index::format::{self, Toc};
use crate::index::normalise::normalise;
use crate::provenance::Provenance;

/// A shared, range-limited view into the mmap. Implements `AsRef<[u8]>` so it
/// can back an `fst::Map` while keeping the underlying mmap alive.
#[derive(Clone)]
struct ArcSlice {
    mmap: Arc<Mmap>,
    range: Range<usize>,
}

impl AsRef<[u8]> for ArcSlice {
    fn as_ref(&self) -> &[u8] {
        &self.mmap[self.range.clone()]
    }
}

/// One search result.
#[derive(Debug, Clone)]
pub struct Hit {
    /// Concept SCTID.
    pub concept_id: u64,
    /// The concept's preferred term, original case (for display).
    pub term: String,
    /// The normalised index key that matched the query (empty for word search).
    pub matched: String,
    /// Semantic tag of the matched key, if it carried one.
    pub semantic_tag: Option<String>,
    /// Crude relevance score: exact > prefix > fuzzy, used only to order results.
    pub score: f32,
}

impl Hit {
    /// Compact JSON for the autocomplete surfaces (`sct sayt --stdio`, the
    /// interactive TUI, and `sct serve`'s `/autocomplete`). `id` is a **string**
    /// because SCTIDs exceed JavaScript's safe-integer range (2^53), so a JSON
    /// number would silently lose precision in a browser client.
    pub fn to_json(&self) -> serde_json::Value {
        // Round in f64 space (not f32-then-widen) so the value serialises as a
        // clean "0.788" rather than the f32's widened noise "0.7879999876".
        let score = (f64::from(self.score) * 1000.0).round() / 1000.0;
        serde_json::json!({
            "id": self.concept_id.to_string(),
            "display": self.term,
            "score": score,
            "tag": self.semantic_tag,
        })
    }
}

/// An opened, queryable FST index.
pub struct Index {
    mmap: Arc<Mmap>,
    descriptions: Map<ArcSlice>,
    words: Map<ArcSlice>,
    postings: Range<usize>,
    word_postings: Range<usize>,
    terms_index: Range<usize>,
    terms_text: Range<usize>,
    tag_table: Vec<String>,
    provenance: Option<Provenance>,
}

/// Upper bound on FST keys visited by a single prefix/fuzzy stream, so a very
/// broad query (e.g. a one-character prefix) cannot run unbounded. See
/// `spec/commands/fst.md` §6.
const STREAM_VISIT_CAP: usize = 50_000;

impl Index {
    /// Open and validate a `snomed.fst` artefact, mmapping it in full.
    pub fn open(path: &Path) -> Result<Index> {
        let file = File::open(path).with_context(|| format!("opening index {}", path.display()))?;
        // SAFETY: we treat the mapping as immutable for the lifetime of `Index`
        // and never write through it. The file is a static, build-time artefact.
        let mmap = unsafe { Mmap::map(&file) }
            .with_context(|| format!("mmapping index {}", path.display()))?;
        let mmap = Arc::new(mmap);

        let toc = Toc::parse(&mmap).context("parsing index container")?;
        let descriptions = open_map(&mmap, &toc, format::SEC_DESCRIPTIONS)?;
        let words = open_map(&mmap, &toc, format::SEC_WORDS)?;
        let postings = toc.require(format::SEC_POSTINGS)?;
        let word_postings = toc.require(format::SEC_WORD_POSTINGS)?;
        let terms_index = toc.require(format::SEC_TERMS_INDEX)?;
        let terms_text = toc.require(format::SEC_TERMS_TEXT)?;

        let tag_table: Vec<String> = {
            let r = toc.require(format::SEC_TAG_TABLE)?;
            serde_json::from_slice(&mmap[r]).context("decoding tag table")?
        };
        let provenance: Option<Provenance> = {
            let r = toc.require(format::SEC_PROVENANCE)?;
            if r.is_empty() {
                None
            } else {
                Some(serde_json::from_slice(&mmap[r]).context("decoding provenance")?)
            }
        };

        Ok(Index {
            mmap,
            descriptions,
            words,
            postings,
            word_postings,
            terms_index,
            terms_text,
            tag_table,
            provenance,
        })
    }

    /// Release provenance recorded at build time, if any.
    pub fn provenance(&self) -> Option<&Provenance> {
        self.provenance.as_ref()
    }

    /// Whether the index carries display side-tables (preferred-term labels).
    /// `false` if it was built with `--no-terms`, in which case [`Hit::term`] is
    /// always empty and callers must resolve labels themselves.
    pub fn has_terms(&self) -> bool {
        let idx = &self.mmap[self.terms_index.clone()];
        idx.len() >= 4 && u32::from_le_bytes(idx[0..4].try_into().unwrap()) > 0
    }

    /// All known semantic tags (excludes the empty "no tag" slot at index 0).
    pub fn semantic_tags(&self) -> impl Iterator<Item = &str> {
        self.tag_table.iter().skip(1).map(|s| s.as_str())
    }

    /// Exact match on a term. Returns every concept whose normalised term equals
    /// the (normalised) query.
    pub fn lookup_exact(&self, term: &str) -> Vec<Hit> {
        let key = normalise(term);
        let mut hits = Vec::new();
        if let Some(packed) = self.descriptions.get(key.as_bytes()) {
            let (tag, off) = format::unpack(packed);
            for cid in self.read_postings(&self.postings, off) {
                hits.push(self.make_hit(cid, &key, tag, 1.0));
            }
        }
        hits
    }

    /// Prefix search over the normalised key space (autocomplete).
    pub fn lookup_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<Hit>> {
        let key = normalise(prefix);
        let aut = Str::new(&key).starts_with();
        let stream = self.descriptions.search(aut).into_stream();
        Ok(self.collect_stream(stream, &key, 0.8, limit))
    }

    /// Fuzzy (Levenshtein) search up to `max_distance` edits.
    pub fn lookup_fuzzy(&self, term: &str, max_distance: u32, limit: usize) -> Result<Vec<Hit>> {
        let key = normalise(term);
        let lev = Levenshtein::new(&key, max_distance)
            .with_context(|| format!("building Levenshtein automaton for {key:?}"))?;
        let stream = self.descriptions.search(lev).into_stream();
        Ok(self.collect_stream(stream, &key, 0.6, limit))
    }

    /// Word-level intersection: concepts whose terms contain *every* given word.
    pub fn lookup_words(&self, words: &[&str], limit: usize) -> Vec<Hit> {
        let keys: Vec<String> = words
            .iter()
            .map(|w| normalise(w))
            .filter(|w| !w.is_empty())
            .collect();
        if keys.is_empty() {
            return Vec::new();
        }

        let mut lists: Vec<Vec<u64>> = Vec::with_capacity(keys.len());
        for k in &keys {
            match self.words.get(k.as_bytes()) {
                Some(packed) => {
                    let (_tag, off) = format::unpack(packed);
                    lists.push(self.read_postings(&self.word_postings, off));
                }
                // A word that matches nothing makes the intersection empty.
                None => return Vec::new(),
            }
        }

        // Intersect smallest-first to keep the working set small.
        lists.sort_by_key(|l| l.len());
        let mut acc = lists[0].clone();
        for next in &lists[1..] {
            acc = intersect_sorted(&acc, next);
            if acc.is_empty() {
                break;
            }
        }

        let matched = keys.join(" ");
        acc.into_iter()
            .take(limit)
            .map(|cid| self.make_hit(cid, &matched, 0, 0.7))
            .collect()
    }

    /// Search-as-you-type: given a partial query, return the best ranked hits.
    /// This is the shared core behind `sct sayt` (interactive TUI + stdio
    /// protocol) and `sct serve`'s `/autocomplete` endpoint.
    ///
    /// It blends whole-term prefix - the primary autocomplete signal, e.g.
    /// `myoc` → *Myocardial infarction* - with whole-word intersection for
    /// multi-word queries, and an optional fuzzy pass for typo tolerance. Every
    /// branch is a sub-millisecond FST stream, so it is cheap to call on every
    /// keystroke.
    pub fn search_typeahead(&self, query: &str, limit: usize, fuzzy: bool) -> Vec<Hit> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }

        let mut pool: Vec<Hit> = Vec::new();

        // Primary signal: terms that start with the whole query.
        if let Ok(mut h) = self.lookup_prefix(q, limit.saturating_mul(4)) {
            pool.append(&mut h);
        }

        // Multi-word queries: terms containing every (whole) word, in any
        // order/position. Only fires with >1 word so single-word typing stays
        // prefix-driven and instant.
        let words: Vec<&str> = q.split_whitespace().collect();
        if words.len() > 1 {
            pool.append(&mut self.lookup_words(&words, limit.saturating_mul(2)));
        }

        // Typo tolerance, opt-in and only when the cheaper passes found little,
        // since fuzzy is the broadest stream.
        if fuzzy && pool.len() < limit {
            let dist = if q.chars().count() > 6 { 2 } else { 1 };
            if let Ok(mut h) = self.lookup_fuzzy(q, dist, limit.saturating_mul(2)) {
                pool.append(&mut h);
            }
        }

        dedupe_rank(pool, limit)
    }

    // --- internals ---

    fn collect_stream<S>(&self, mut stream: S, query_key: &str, base: f32, limit: usize) -> Vec<Hit>
    where
        S: for<'a> Streamer<'a, Item = (&'a [u8], u64)>,
    {
        // Dedupe by concept, keeping the best score (and the key that earned it).
        let mut best: HashMap<u64, Hit> = HashMap::new();
        let mut visited = 0usize;
        while let Some((kb, packed)) = stream.next() {
            visited += 1;
            if visited > STREAM_VISIT_CAP {
                break;
            }
            let matched = String::from_utf8_lossy(kb).into_owned();
            // Reward keys closer in length to the query: a small, cheap proxy
            // for "how much of the key the query explains".
            let score = base - (matched.len() as f32 - query_key.len() as f32).abs() / 256.0;
            let (tag, off) = format::unpack(packed);
            for cid in self.read_postings(&self.postings, off) {
                let hit = self.make_hit(cid, &matched, tag, score);
                best.entry(cid)
                    .and_modify(|h| {
                        if hit.score > h.score {
                            *h = hit.clone();
                        }
                    })
                    .or_insert(hit);
            }
        }
        let mut hits: Vec<Hit> = best.into_values().collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.concept_id.cmp(&b.concept_id))
        });
        hits.truncate(limit);
        hits
    }

    fn make_hit(&self, concept_id: u64, matched: &str, tag: u8, score: f32) -> Hit {
        let semantic_tag = self
            .tag_table
            .get(tag as usize)
            .filter(|s| !s.is_empty())
            .cloned();
        Hit {
            concept_id,
            term: self.preferred_term(concept_id).unwrap_or_default(),
            matched: matched.to_string(),
            semantic_tag,
            score,
        }
    }

    /// Read a delta-varint posting list at `offset` within a postings section.
    /// Decoding is bounded by the section slice, so a corrupt offset/length can
    /// at worst return a short or empty list - never read out of bounds.
    fn read_postings(&self, section: &Range<usize>, offset: u64) -> Vec<u64> {
        let data = &self.mmap[section.clone()];
        let mut p = offset as usize;
        let Some(len) = format::read_uvarint(data, &mut p) else {
            return Vec::new();
        };
        // Cap the pre-allocation so a corrupt length cannot request a huge Vec.
        let mut out = Vec::with_capacity((len as usize).min(1 << 20));
        let mut acc = 0u64;
        for _ in 0..len {
            let Some(delta) = format::read_uvarint(data, &mut p) else {
                break;
            };
            acc = acc.wrapping_add(delta);
            out.push(acc);
        }
        out
    }

    /// Binary-search the terms index for a concept's preferred term.
    fn preferred_term(&self, sctid: u64) -> Option<String> {
        let idx = &self.mmap[self.terms_index.clone()];
        if idx.len() < 4 {
            return None;
        }
        let count = u32::from_le_bytes(idx[0..4].try_into().unwrap()) as usize;
        let entries = &idx[4..];
        const ENTRY: usize = 16; // u64 sctid + u32 off + u32 len
        let (mut lo, mut hi) = (0usize, count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let e = &entries[mid * ENTRY..mid * ENTRY + ENTRY];
            let id = u64::from_le_bytes(e[0..8].try_into().unwrap());
            match id.cmp(&sctid) {
                std::cmp::Ordering::Equal => {
                    let off = u32::from_le_bytes(e[8..12].try_into().unwrap()) as usize;
                    let len = u32::from_le_bytes(e[12..16].try_into().unwrap()) as usize;
                    let text =
                        &self.mmap[self.terms_text.start + off..self.terms_text.start + off + len];
                    return Some(String::from_utf8_lossy(text).into_owned());
                }
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }
}

fn open_map(mmap: &Arc<Mmap>, toc: &Toc, name: &str) -> Result<Map<ArcSlice>> {
    let range = toc.require(name)?;
    Map::new(ArcSlice {
        mmap: mmap.clone(),
        range,
    })
    .with_context(|| format!("loading FST section '{name}'"))
}

/// Merge hits pooled from several typeahead passes: dedupe by concept (keeping
/// each concept's best score), sort by score descending then SCTID for a
/// stable order, and truncate to `limit`.
fn dedupe_rank(pool: Vec<Hit>, limit: usize) -> Vec<Hit> {
    use std::collections::hash_map::Entry;
    let mut best: HashMap<u64, Hit> = HashMap::new();
    for hit in pool {
        match best.entry(hit.concept_id) {
            Entry::Occupied(mut e) => {
                if hit.score > e.get().score {
                    e.insert(hit);
                }
            }
            Entry::Vacant(e) => {
                e.insert(hit);
            }
        }
    }
    let mut hits: Vec<Hit> = best.into_values().collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.concept_id.cmp(&b.concept_id))
    });
    hits.truncate(limit);
    hits
}

/// Intersect two ascending-sorted SCTID lists. O(n + m).
fn intersect_sorted(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_basic() {
        assert_eq!(
            intersect_sorted(&[1, 2, 3, 5], &[2, 3, 4, 5]),
            vec![2, 3, 5]
        );
        assert_eq!(intersect_sorted(&[1, 2], &[3, 4]), Vec::<u64>::new());
        assert_eq!(intersect_sorted(&[], &[1]), Vec::<u64>::new());
    }

    fn hit(id: u64, score: f32) -> Hit {
        Hit {
            concept_id: id,
            term: format!("term {id}"),
            matched: String::new(),
            semantic_tag: None,
            score,
        }
    }

    #[test]
    fn dedupe_rank_keeps_best_score_and_orders() {
        // Concept 3 appears twice (0.5 and 0.8) - keep 0.8. Concepts 1 and 2 tie
        // at 0.9 - order by SCTID ascending. Result order: 1, 2, then 3.
        let pool = vec![hit(3, 0.5), hit(1, 0.9), hit(3, 0.8), hit(2, 0.9)];
        let out = dedupe_rank(pool, 10);
        assert_eq!(
            out.iter().map(|h| h.concept_id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(out.iter().find(|h| h.concept_id == 3).unwrap().score, 0.8);
    }

    #[test]
    fn dedupe_rank_truncates_to_limit() {
        let pool = vec![hit(1, 0.9), hit(2, 0.8), hit(3, 0.7)];
        assert_eq!(dedupe_rank(pool, 2).len(), 2);
    }

    #[test]
    fn hit_to_json_uses_string_id_and_clean_score() {
        let h = Hit {
            concept_id: 22298006,
            term: "Myocardial infarction".to_string(),
            matched: String::new(),
            semantic_tag: Some("disorder".to_string()),
            score: 0.788,
        };
        let v = h.to_json();
        // SCTID must be a JSON string, not a number (exceeds JS 2^53).
        assert_eq!(v["id"], serde_json::json!("22298006"));
        assert_eq!(v["display"], "Myocardial infarction");
        assert_eq!(v["tag"], "disorder");
        assert_eq!(v["score"].as_f64().unwrap(), 0.788);
    }
}
