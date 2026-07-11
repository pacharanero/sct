# FST-backed lexical index for `sct`

**Status:** ✅ Shipped (v0.4.0–0.4.1). This document is the design record; it now describes a built feature. User docs: [`docs/commands/fst.md`](../../docs/commands/fst.md). The benchmark results in §10 are real measurements.
**Scope:** Evaluate a finite-state-transducer lexical index, built from `sct`'s canonical NDJSON, as a slimmer and more capable alternative to the existing SQLite FTS5 search path. Uses BurntSushi's [`fst`](https://docs.rs/fst/) crate.
**Audience:** A coding agent (and Marcus) implementing this in the `sct` repo.

---

## 1. Why

SNOMED CT has roughly 350,000+ active concepts in the International edition and around 1.3 million active English descriptions across the standard description types. Any local-first library has to ship and load this lexicon somehow.

`sct` already serves lexical lookup today through **SQLite FTS5** (the `concepts_fts` virtual table, driving `sct lexical` and the `snomed_search` MCP tool). So the FST is not filling an empty gap - it is a *second lexical backend competing with FTS5*. The question this work answers is not "how do we implement lookup" but "what does an FST buy us that FTS5 does not, and is it worth a parallel index?"

The honest trade (size/latency now measured - see §10):

| | SQLite FTS5 (today) | FST (built) |
|---|---|---|
| Lexical-index size | ~103 MB (FTS5 shadow tables) | **72 MB** search-only (`--no-terms`); 133 MB with labels |
| Query latency | ~0.5–1.2 ms warm | ~1 µs–3 ms (1–2 orders of magnitude faster) |
| Ranked free-text | BM25, built in | manual TF/IDF only |
| Prefix / autocomplete | awkward | native |
| Fuzzy (typo-tolerant) | no | yes, via Levenshtein automaton |
| mmap, zero-deserialise start | partial | yes |
| Already wired to MCP | yes | not yet |

The two things that actually justify the FST for `sct` are therefore: **an order-of-magnitude-smaller, distributable lexical artefact**, and **fuzzy + prefix search that FTS5 cannot do natively**. SNOMED is unusually well suited to the underlying data structure:

- **Massive suffix repetition.** Every FSN ends in one of ~100 semantic tags: `(disorder)`, `(finding)`, `(procedure)`, `(body structure)`, `(substance)`, etc. Tens of thousands of terms share each tail.
- **Heavy internal phrase repetition.** "Fracture of shaft of left femur", "Fracture of shaft of right femur", "Fracture of neck of left femur" and so on. Shared phrase fragments at predictable positions.
- **Static at runtime.** SNOMED International releases twice a year, UK every six months. Rebuild offline, ship the binary, never mutate at runtime.

A minimal acyclic deterministic finite-state automaton (the structure under the `fst` crate's `Map` and `Set` types) compresses *both* prefixes *and* suffixes by merging equivalent subtrees. For natural-language data with SNOMED's regularity the compression ratio against a comparable hash map is often 100x or better, and queries are still sub-microsecond.

Andrew Quinn's [write-up](https://til.andrew-quinn.me/posts/replacing-a-3-gb-sqlite-database-with-a-7-mb-fst-finite-state-trandsucer-binary/) replacing a 3 GB SQLite database with a 10 MB FST for a Finnish dictionary is the proximal motivation. The canonical deep dive is BurntSushi's [Index 1,600,000,000 Keys with Automata and Rust](https://burntsushi.net/transducers/).

### 1.1 Decision: benchmark first

We are not committing to "supplement FTS5" or "replace FTS5" up front. The first deliverable is a standalone `snomed.fst` artefact plus a benchmark harness that produces real size and latency numbers against the existing FTS5 path. We choose the integration direction from those numbers, not from the spec. See §6 for exactly what the benchmark must measure and §9 for the slice.

---

## 2. What an FST is, briefly

A trie stores a set of strings as a tree, sharing prefixes. Two words that share their first three letters share a path through their first three nodes, then branch.

A *minimal acyclic DFA* extends this by also merging equivalent subtrees. Two states are equivalent if they have the same accept flag and the same set of outgoing `(label, target)` transitions. Merging is done bottom-up: process leaves first, replace any state whose signature is already present in a register with the canonical entry, then move up one layer. The result is a DAG, not a tree - states can have multiple incoming edges.

A *transducer* (the T in FST) extends the minimal DFA with an output value emitted along each edge. As you walk from the start state to an accept state, the output values accumulate. For a `Map<&[u8], u64>` over SNOMED terms, the accumulated value is a packed pointer to the concepts associated with the matched term.

The `fst` crate implements this with a packed on-disk format that is mmap-friendly, so you do not pay any deserialisation cost at startup.

---

## 3. Input: `sct`'s NDJSON, not RF2 directly

**Decision: the FST builder consumes NDJSON, not an RF2 snapshot.** In `sct`, NDJSON is the canonical intermediate - `sqlite`, `parquet`, `markdown`, and `embed` are all built *from NDJSON*, never from RF2. RF2 → NDJSON (the `sct ndjson` command, via `rf2.rs` + `builder.rs`) already does the hard joins: active-only filtering, FSN / preferred-term / synonym bucketing, semantic-tag extraction, and edition merging. Reading NDJSON inherits all of that for free; reading RF2 would re-implement it and drift from the pipeline.

Each NDJSON line is one `ConceptRecord` (see `schema.rs`). The fields the FST needs:

| Field | Use in the FST |
|---|---|
| `id` (concept SCTID) | the value we ultimately resolve a term to |
| `fsn` | a key; the trailing semantic tag is stripped and recorded in the packed value |
| `preferred_term` | a key |
| `synonyms[]` | keys; original case preserved |
| (semantic tag) | re-extracted from `fsn` with the same regex `builder.rs` already uses |

The first NDJSON line, when present, is provenance (`_type: "sct_provenance"`: edition, date, `sct` version). We reuse this verbatim for the artefact's embedded version stamp - no bespoke versioning scheme (see `provenance.rs`).

Two consequences worth stating, because they pre-resolve open questions from earlier drafts:

- **Inactive descriptions are already gone.** NDJSON is active-only, so the FST is active-only for free. No flag, no doubled index.
- **Edition scope is decided upstream.** Whatever RF2 was ingested (International, or International + UK Clinical + UK Drug merged) is already baked into the NDJSON. The FST indexes whatever NDJSON it is handed; merging is not its concern.

What NDJSON does *not* carry is description-level SCTIDs (it is per-concept). The earlier draft keyed an original-case `terms` table by description SCTID; we key it by concept instead, since the original-case strings are already present in the NDJSON fields. No information is lost for lexical lookup.

Out of scope for this index, as before: the IS-A hierarchy, attribute relationships, and reference sets. Those have their own representations in `sct` (`concept_isa`, the TCT builder, `refset_members`). The FST replaces lexical lookup only.

---

## 4. Design

### 4.1 What we index

One primary FST:

- **Key:** normalised term, encoded as UTF-8 bytes
- **Value:** `u64` packed as `(semantic_tag_id << 56) | posting_offset`, where:
  - `semantic_tag_id` is one byte: an index into a small table of the ~100 distinct semantic tags (`(disorder)`, `(finding)`, etc.), or `0` for terms with no associated tag
  - `posting_offset` is a 56-bit offset into the postings section holding the list of concept SCTIDs this term resolves to

Term normalisation is:

1. NFC-normalise the Unicode
2. Lowercase (Unicode-aware, `to_lowercase`)
3. Strip the trailing semantic tag for FSNs (the tag itself is kept in the value as `semantic_tag_id`)
4. Collapse internal whitespace to a single space, trim

The original-case term text is stored separately in the artefact's `terms` section for display. The FST is for *lookup*; display goes via the side tables.

### 4.2 Why pack the tag into the value

Filtering by semantic tag is the single most common SNOMED query refinement ("only disorders", "only procedures"). Packing the tag into the FST value lets a filtered lookup reject non-matching results without dereferencing into the posting list at all. The top byte of the `u64` is essentially free. SNOMED is comfortably under 256 active semantic tags; if that ever changes, switch to per-tag FSTs.

### 4.3 Why a posting list instead of a direct SCTID

Multiple concepts can share a synonym ("Cold" maps to at least a finding, a sensation, and a virus). A direct `term -> SCTID` mapping cannot represent this. The posting list also handles the inverse (multiple terms for one concept) naturally.

### 4.4 Secondary index: `words`

To answer "show me all concepts containing 'femur'" - and to give the benchmark a fair comparison against FTS5's token `MATCH` - we build a second FST keyed on individual tokens: `Map<word, posting_offset>`, the posting pointing to a list of concept SCTIDs whose terms contain that token. Word-level intersection (§5.4) is what makes the replace-vs-supplement question answerable, so it is in the first slice rather than deferred.

A reverse `SCTID -> preferred term` map is *not* built as an FST. `sct` already resolves SCTIDs through the concepts table; an FST shines for string keys, not numeric ones.

### 4.5 Single-file artefact: `snomed.fst`

Earlier drafts proposed a `sct-index/` directory of loose files. We instead emit **one file, `snomed.fst`**, to match `sct`'s neat single-artefact convention (`snomed.ndjson`, `snomed.db`, `snomed.parquet`, `snomed-embeddings.arrow`). One file is also nicer to distribute and mmaps in a single call.

Internally it is a simple container with a table of contents at the end (zip/parquet style: stream the sections, then write the TOC, then a fixed footer pointing at the TOC):

```
+-----------------------------------------------------------+
| magic  "SCTFST\0"  (8 bytes)                              |
| u32    container format version                           |
+-----------------------------------------------------------+
| section: descriptions.fst   (term -> packed value)        |
| section: postings           (delta-varint SCTID lists)    |
| section: words.fst          (token -> posting offset)     |
| section: word_postings      (delta-varint SCTID lists)    |
| section: terms              (concept SCTID -> orig text)   |
| section: tag_table          (tag_id byte -> tag string)   |
| section: provenance         (edition/date/sct version)    |
+-----------------------------------------------------------+
| TOC: [ (name, u64 offset, u64 length) ... ]               |
| footer: u64 toc_offset, u32 section_count, magic          |
+-----------------------------------------------------------+
```

`Index::open` mmaps the whole file once, reads the footer and TOC, and hands each section out as a zero-copy byte slice. The two `.fst` sections are wrapped in `fst::Map::new(slice)`; the `*_postings`, `terms`, and `tag_table` sections are read by computed offset. No allocation on the hot path.

The `terms` section (display labels) is optional: `sct fst build --no-terms` writes empty `terms_index`/`terms_text` sections, producing a search-only index ~64 MB smaller, for use alongside SQLite where labels resolve from the `concepts` table. Posting lists (`postings`, `word_postings`) are delta + unsigned-varint encoded (container format v2) rather than raw `u64` arrays.

---

## 5. Implementation

### 5.1 Crates

New runtime dependencies:

```toml
fst = "<latest>"
memmap2 = "<latest>"     # not currently used anywhere in sct
```

New dev dependency:

```toml
criterion = "<latest>"   # benchmarks live under benchmarks/
```

Already present and reused: `serde_json` (read NDJSON), `unicode-normalization` is *not* yet a dep - add it if NFC normalisation needs it, otherwise `to_lowercase` + whitespace collapse may suffice for v0; decide when implementing. Pin every version to whatever `cargo add` reports as current - do not write versions from memory.

Conventions: **`anyhow::Result<T>` + `.context()` throughout**, matching the rest of `sct`. No `thiserror`, no bespoke `BuildError`/`OpenError` enums.

### 5.2 Build pipeline

The build is one-shot per SNOMED release, exposed as a flat subcommand mirroring `sct sqlite` / `sct parquet`:

```
sct fst --ndjson snomed.ndjson --output snomed.fst
```

`--output` defaults to `snomed.fst` (clap `default_value`, exactly as `sqlite` defaults to `snomed.db`). `--ndjson` resolves through the existing NDJSON path discovery. A later change can register a `Kind::Fst` (env `SCT_FST`, canonical `snomed.fst`) in `paths.rs` so queries can discover the artefact the same way `--db` and `--embeddings` do; not required for the build itself.

Phases:

1. **Stream NDJSON.** Read line by line with `serde_json`. Skip the provenance line (capture it for the stamp). Each remaining line yields a concept `id` plus its `fsn`, `preferred_term`, and `synonyms`.
2. **Normalise & tag.** For each term, compute the normalised key and, for FSNs, extract the semantic tag (reuse `builder.rs`'s regex). Resolve the tag string to a `u8` id, allocating on first sight.
3. **Group.** Group by normalised term; each group's value is a deduplicated, sorted posting list of concept SCTIDs. Build the `words` groups in the same pass by tokenising each term.
4. **Write postings.** Append each posting list to the postings buffer as `[u32 length][u64 sctid]*`; record the offset.
5. **Sort keys.** `MapBuilder` requires sorted insertion. `Vec::sort` over ~1.3M entries is fine; reach for external sort only if a build machine actually runs out of memory (unlikely).
6. **Write FSTs.** Loop sorted keys, pack `(tag_id, offset)` into a `u64`, `insert(term.as_bytes(), packed)`. Repeat for `words`.
7. **Assemble the container.** Concatenate sections, write the TOC and footer, embed the provenance stamp.

A skeletal builder for one FST section:

```rust
use anyhow::{Context, Result};
use fst::MapBuilder;

pub fn build_map_section(pairs: &[(String, u64)]) -> Result<Vec<u8>> {
    // pairs already sorted by key, values pre-packed
    let mut builder = MapBuilder::memory();
    for (term, packed) in pairs {
        builder
            .insert(term.as_bytes(), *packed)
            .with_context(|| format!("inserting key {term:?}"))?;
    }
    builder.into_inner().context("finishing FST")
}
```

Sorted insertion is non-negotiable - `MapBuilder` errors on out-of-order keys.

### 5.3 Query API

```rust
pub struct Index {
    mmap: memmap2::Mmap,            // the whole snomed.fst
    descriptions: fst::Map<&[u8]>,  // section slice
    words: fst::Map<&[u8]>,         // section slice
    postings: &[u8],
    word_postings: &[u8],
    terms: &[u8],
    tag_table: Vec<String>,
    provenance: Provenance,
}

impl Index {
    pub fn open(path: &Path) -> Result<Self> { /* mmap, read TOC, slice sections */ }

    /// Exact match on normalised term. Returns all concepts with this term.
    pub fn lookup_exact(&self, term: &str) -> Vec<Hit>;

    /// Prefix search, for autocomplete.
    pub fn lookup_prefix(&self, prefix: &str, limit: usize) -> Vec<Hit>;

    /// Fuzzy match via a Levenshtein automaton.
    pub fn lookup_fuzzy(&self, term: &str, max_distance: u32, limit: usize) -> Vec<Hit>;

    /// Word-level intersection: concepts whose terms contain every word.
    pub fn lookup_words(&self, words: &[&str], limit: usize) -> Vec<Hit>;
}

pub struct Hit {
    pub concept_id: u64,
    pub matched_term: String,        // original case
    pub semantic_tag: Option<String>,
    pub score: f32,                  // exact > prefix > fuzzy, prefer FSN
}
```

The core primitive is "walk an automaton against the FST":

```rust
use fst::{IntoStreamer, Streamer};
use fst::automaton::{Automaton, Levenshtein, Str};

// Exact
if let Some(packed) = self.descriptions.get(term.as_bytes()) { /* unpack, resolve */ }

// Prefix
let mut stream = self.descriptions.search(Str::new(prefix).starts_with()).into_stream();
while let Some((term_bytes, packed)) = stream.next() { /* ... */ }

// Fuzzy
let lev = Levenshtein::new(term, max_distance)?;
let mut stream = self.descriptions.search(lev).into_stream();
while let Some((term_bytes, packed)) = stream.next() { /* ... */ }

// Tag-filtered: reject results whose top byte is not the wanted tag, before dereferencing postings
if ((packed >> 56) as u8) != tag_byte { continue; }
```

`Levenshtein::new` builds a Levenshtein automaton over the query and intersects it with the FST in a single pass - no enumeration of edits.

### 5.4 Word-level intersection

For `lookup_words(&["fracture", "femur"])`:

1. Look each word up in `words`; each returns a sorted posting list of concept SCTIDs.
2. Merge-intersect the lists (O(n + m)).
3. Dedupe, score, limit.

TF/IDF ranking is out of scope for the benchmark slice; note where it would slot in but skip it.

---

## 6. The benchmark - what it must produce

This is the actual deliverable. The harness (criterion under `benchmarks/`, plus a small table-printer) runs the FST against the **real local `snomed.ndjson` / `snomed.db`** already in the repo and emits:

1. **Size.** `snomed.fst` total, and its `descriptions.fst` + `postings` + `words` + `word_postings` sections individually, versus the **FTS5 shadow tables specifically** - i.e. the `concepts_fts_*` tables measured via `dbstat`, *not* the whole 1.8 GB `snomed.db`. This is the apples-to-apples lexical-index comparison.
2. **Build time.** NDJSON → `snomed.fst` wall-clock.
3. **Query latency**, FST vs equivalent FTS5 `MATCH`, on a fixed query set: `lookup_exact`, `lookup_prefix`, `lookup_fuzzy` at d=1 and d=2, `lookup_words` at 1/2/3 words. Report distribution, not just mean (short fuzzy queries that match a huge fraction of the corpus are the tail to watch; cap with `limit` and a wall-clock budget).
4. **Capability delta.** Demonstrate, with concrete hits, the things FTS5 cannot do: fuzzy ("myocaridal" → myocardial infarction) and prefix/autocomplete. The point is to show these *work and are useful*, not only that they are fast.
5. **Cold start.** mmap `Index::open` vs `rusqlite` open + first query.

Educated guesses to be confirmed: `snomed.fst` in the 50–80 MB range for International English; exact lookup sub-microsecond; prefix scaling with result-set size; fuzzy d=2 in the millisecond range for low-frequency stems. Do not optimise before these are measured.

The output of this section - a single comparison table plus the capability demonstration - is what decides supplement-vs-replace.

---

## 7. Open design decisions

Resolved by the architecture and our choices above:

- **Input source** → NDJSON (§3).
- **Inactive descriptions** → excluded, inherited from NDJSON (§3).
- **Edition scope** → decided upstream at ingest, not the FST's concern (§3).
- **Versioning** → reuse the existing provenance stamp; `Index::open` validates it and refuses a mismatch with a clear error (§3, §5.2).
- **Output shape** → single `snomed.fst` container file (§4.5).
- **Role vs FTS5** → undecided on purpose; the §6 benchmark decides.

All resolved as of the build kick-off:

1. **Term normalisation details - resolved: no diacritic-folding, no punctuation stripping.** We accept the additional index size in exchange for not losing precision. Normalisation is exactly: NFC-normalise (lossless - composes equivalent encodings, does **not** remove accents), Unicode lowercase, strip the trailing semantic tag for FSNs, collapse internal whitespace, trim. Diacritics and all punctuation are preserved. This is the one normalisation `unicode-normalization` is pulled in for; it must stay stable across releases.
2. **Language refsets - resolved: English only for now.** Matches the current NDJSON pipeline. The schema does not preclude per-language FSTs later.
3. **Distribution - resolved: no prebuilt artefact at this stage.** Ship the builder only; users supply their own NDJSON. A future SNOMED GPS-based path (see `docs/`) could make a distributable index possible later, but that is out of scope now.

---

## 8. Testing

Following `sct` conventions (`anyhow`, `tempfile` fixtures, integration tests under `tests/`, inline `#[cfg(test)]` for pure functions):

- **Unit tests** for normalisation, tag extraction, value packing/unpacking, and TOC/section round-tripping. Pure functions, test exhaustively.
- **Property tests** for the core invariant - any term inserted at build is found at query, and packing round-trips `(tag_id, offset)`. Add `proptest` only if we actually want generative coverage; otherwise table-driven cases are acceptable for v0.
- **Integration test** from a **synthetic NDJSON fixture** - a dozen hand-crafted lines covering FSN, synonyms, one-term-many-concepts, semantic-tag variety, and a Unicode edge case. Build an index in a tempdir and round-trip queries through it. Commit the fixture; never depend on a real SNOMED release in CI.
- **Benchmarks** with `criterion` per §6. These run against local real data, so they are developer-run, not CI-gated.
- **CI** already enforces `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` (see the pre-commit hook and `.github/workflows/ci.yml`); the new code must pass both.

---

## 9. Sequencing - the first (benchmark) PR

The first PR exists to lock down the `snomed.fst` container format and the build API, and to produce the §6 comparison numbers. Concretely:

1. `src/commands/fst.rs` (the `sct fst` subcommand, wired into the `Command` enum in `main.rs`) plus a `src/fst/` module (`normalise.rs`, `build.rs`, `query.rs`, `format.rs`).
2. Normalisation + tag extraction as pure functions, reusing `builder.rs`'s tag regex. Unit + property tests.
3. Builder reading NDJSON → `descriptions.fst` + postings **and** `words.fst` + word postings, assembled into the `snomed.fst` container with the provenance stamp.
4. `Index::open` (single mmap, TOC parse) + `lookup_exact`, `lookup_prefix`, `lookup_fuzzy`, `lookup_words`.
5. Synthetic NDJSON fixture + integration test (build, round-trip).
6. `criterion` benchmark + table-printer producing the §6 size/latency/capability comparison against the local `snomed.db`.
7. Open the PR with the comparison table in the description.

Deferred to follow-ups, gated on the benchmark looking good: the `terms` display section if not done in slice 1, TF/IDF ranking, `paths.rs` `Kind::Fst` discovery, MCP wiring (back `snomed_search` with the FST, or add `snomed_fuzzy`), and any decision to retire the FTS5 path.

Resist shipping everything at once. Slice 1 is about the format, the build API, and the numbers that choose our direction.

---

## 10. Benchmark results (first run)

Measured on the local artefacts: an **831,132-concept** edition (UK Monolith-scale; far larger than International's ~350k), `snomed.ndjson` (1.1 GB) and `snomed.db` (1.8 GB). Built with `sct fst build` in **16.3 s**, producing **160.4 MB** of `snomed.fst` (1,949,665 terms → 1,252,590 distinct keys, 177,261 word tokens, 59 semantic tags). Query latency from `benchmarks/fst_bench.rs` (criterion, in-process, warm); FTS5 from SQLite's statement timer (warm).

### Size - closer than the spec guessed

The `snomed.fst` sections:

| Section | Size | Note |
|---|---:|---|
| Section | v1 (raw) | **v2 (delta-varint)** | Note |
|---|---:|---:|---|
| `descriptions` (FST) | 20.8 MB | 21.0 MB | the term automaton - small |
| `postings` | 15.1 MB | **9.2 MB** | concept SCTID lists for descriptions |
| `words` (FST) | 1.4 MB | 1.4 MB | token automaton - tiny |
| `word_postings` | 66.6 MB | **44.2 MB** | the dominant section; the big compression target |
| `terms_index` + `terms_text` | 64.3 MB | 64.3 MB | display side-tables (preferred terms) |
| **Total** | 160.4 MB | **133.5 MB** | |

Posting lists were initially raw `u64` arrays. Switching to **delta + unsigned-varint** encoding (container format v2) shrank `postings` by 39% and `word_postings` by 34%, taking the full artefact from 160 MB to **133.5 MB**. The saving is bounded by SNOMED's large SCTIDs: even a delta between adjacent concepts sharing a token often needs 3–5 varint bytes, so the win is real but not dramatic. Build time is unchanged (~16 s) and decode cost is noise-level (see latency below).

Two levers control size, both now implemented:

- **Posting compression** (always on, v2): full index 160 → 133.5 MB.
- **`--no-terms`** drops the 64.3 MB display side-tables, for use alongside SQLite where labels resolve from the `concepts` table. Combined with compression, the **search-only index is 72.2 MB** - now roughly **30% smaller** than the ~103 MB FTS5 inverted index (`concepts_fts_*` via `dbstat`), reversing the v1 "wash". We deliberately keep full precision (no diacritic-folding, no punctuation stripping), which is the main thing still inflating the key space.

So the size picture by configuration: **72 MB** search-only (beats FTS5), **133 MB** with labels, versus the whole **1.8 GB** `snomed.db`. The remaining bloat is `word_postings` (44 MB) and the uncompressed label strings (`terms_text`, 51 MB) - both compressible further if it matters.

### Latency - a decisive FST win

| Query | FST (warm) | FTS5 (warm) | Speedup |
|---|---:|---:|---:|
| exact term | **~0.95 µs** | n/a (MATCH) | - |
| word intersection (`fracture femur`) | **~11 µs** | ~570 µs | ~50× |
| prefix (`myocard`) | **~87 µs** | ~1.2 ms | ~14× |
| fuzzy d=1 | **~364 µs** | not supported | - |
| fuzzy d=2 | **~3.4 ms** | not supported | - |

Exact lookup is sub-microsecond as predicted; word and prefix search beat warm FTS5 by 1–2 orders of magnitude; cold start is a single constant-time mmap.

### Capability - fuzzy and prefix work, with a caveat

Prefix and word search behave well on real data. Fuzzy d=1 recovers single-character typos cleanly (`diabetes mellitis`→*Diabetes mellitus*, `asthsma`→*Asthma*, `paracetomol`→*Paracetamol* (substance)). The caveat: Levenshtein runs over the **whole key**, so edits accumulate across a long phrase - a two-typo query over a 20-character FSN can exceed distance 2 and miss. Fuzzy is therefore most useful on shorter terms / single words; per-word fuzzy matching would be a future refinement.

### Verdict

Lean **supplement, not replace** (for now). The FST's standout wins remain **query latency** (1–2 orders of magnitude) and **fuzzy/prefix capability**. After the v2 work the **size story is now also favourable**: a `--no-terms` search index is ~30% smaller than the FTS5 inverted index, and even with labels it is an order of magnitude below the full `snomed.db`. What still keeps this in "supplement" territory is the lack of **BM25 ranking** and the fact that label resolution needs either the +64 MB display tables or a companion SQLite. Remaining optimisations, in priority order: (1) richer ranking (TF/IDF or BM25) so it could stand alone for search; (2) compress the label strings; (3) per-word fuzzy matching; then (4) MCP wiring once a direction is chosen. Re-measuring against International-only (~350k concepts) would roughly halve every absolute number here.

---

## 11. References

- BurntSushi, [Index 1,600,000,000 Keys with Automata and Rust](https://burntsushi.net/transducers/) - the canonical deep dive; read this first.
- [`fst` crate documentation](https://docs.rs/fst/) - API reference.
- Andrew Quinn, [Replacing a 3 GB SQLite database with a 10 MB FST](https://til.andrew-quinn.me/posts/replacing-a-3-gb-sqlite-database-with-a-7-mb-fst-finite-state-trandsucer-binary/) - the proximal motivation.
- Daciuk, Mihov, Watson, Watson, [Incremental construction of minimal acyclic finite-state automata](https://aclanthology.org/J00-1002/) - the algorithm `fst` is built on.
- SNOMED International, [Release File Specifications - RF2](https://confluence.ihtsdotools.org/display/DOCRELFMT) - canonical reference for the upstream file layout (consumed by `sct ndjson`, not by the FST builder).
