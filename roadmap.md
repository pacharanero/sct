# SNOMED Local-First Tooling — Roadmap

Outstanding work and next steps. Completed milestones have been removed.

---

## In progress / near-term

### Distribution

- [x] Publish to crates.io and document `cargo install sct` (CI workflow adds `cargo publish` step on tag push)
- [ ] Add Windows x86_64 (`x86_64-pc-windows-msvc`) to the release CI matrix
- [ ] Homebrew formula for macOS one-liner install (`brew install sct`)
- [ ] SHA-256 checksums for NDJSON artefacts published alongside GitHub Releases

### Quality

- [ ] End-to-end integration test: RF2 → NDJSON → SQLite → MCP query (CI-runnable with the sample data already in the repo)
- [ ] Build and run `bench/bench.sh` against `https://terminology.openehr.org/fhir` and populate real timings in `BENCHMARKS.md`
- [ ] Smoke test for `sct embed`: embed a handful of concepts, query for "heart attack", assert myocardial infarction concepts appear in top results

---

## Features

### `bench/` — benchmarking suite

Bash script suite for automated, fair comparison of `sct` against a FHIR R4 terminology server. Specified in `spec.md` under "Benchmarking Tooling".

- [ ] `bench/bench.sh` entry point with `--server`, `--db`, `--runs`, `--format` flags
- [ ] `lib/timing.sh` — hyperfine wrapper with manual timing fallback
- [ ] `lib/fhir.sh` / `lib/local.sh` — query wrappers for each side
- [ ] `lib/report.sh` — table/JSON/CSV output
- [ ] Operations: `lookup`, `search`, `children`, `ancestors`, `subsumption`, `bulk`
- [ ] Fixtures: `concepts.txt` (15 well-known SCTIDs), `search_terms.txt` (9 terms)
- [ ] `bench/README.md` — usage docs

### `sct mcp` — semantic search tool

Add a `snomed_semantic_search` MCP tool that loads the Arrow IPC file produced by `sct embed` and returns the nearest-neighbour concepts for a natural-language query. The tool would embed the query via Ollama at call time and perform cosine similarity against the pre-built index.

- [ ] Accept an optional `--embeddings` flag pointing to a `.arrow` file
- [ ] Implement `snomed_semantic_search` tool (query text → top-N concepts by embedding similarity)
- [ ] Graceful degradation: if no `--embeddings` file is provided, the tool is simply not registered

### `sct lexical` and `sct semantic` — search commands

- [x] `sct lexical <query>` — FTS5 keyword search against the SQLite database; phrase/prefix/boolean query syntax; `--hierarchy` filter; `--limit`
- [x] `sct semantic <query>` — cosine similarity search against the Arrow IPC embeddings file; embeds query via Ollama; `--limit`; clear error if Ollama not running

### `sct diff` — compare two NDJSON artefacts

Compare two releases of the canonical artefact (e.g. 2025-01 vs 2026-01) and report:

- [x] Concepts added since the previous release
- [x] Concepts inactivated since the previous release
- [x] Concepts whose preferred term changed
- [x] Concepts whose hierarchy changed
- [x] `--format summary` (default, human-readable) and `--format ndjson` (one diff record per change)

### `sct info` — inspect an artefact

A quick introspection command for any `sct`-produced file:

- [x] For `.ndjson`: concept count, `schema_version`, release date (from filename), hierarchy breakdown
- [x] For `.db`: concept count, schema version, FTS row count, IS-A edge count, file size, hierarchy breakdown
- [x] For `.arrow`: embedding count, dimension, schema, file size

---

## Future / larger scope

- [ ] **TRUD integration** — `sct trud` subcommand that authenticates with the NHS TRUD API and downloads the latest UK Monolith RF2 release automatically
- [ ] **History files** — parse RF2 history substitution tables to map inactivated concept IDs forward to their replacements; expose via `snomed_resolve` MCP tool
- [ ] **`sct serve`** — HTTP FHIR terminology server implementing the standard FHIR R4/R5 `CodeSystem` and `ValueSet` operations (`$lookup`, `$validate-code`, `$expand`, `$subsumes`) backed by the SQLite database. Drop-in replacement for cloud terminology servers in EHR integration and FHIR workflow testing. No Elasticsearch or Java required.
- [ ] **Concept maps** — cross-map support: load SNOMED→ICD-10/OPCS-4 map files from RF2 and expose via `snomed_map` MCP tool
- [ ] **IPS Free Set bundling** — SNOMED International publishes a curated SNOMED CT IPS Free Set specifically for International Patient Summary use, available without a member licence. Investigate bundling the pre-processed NDJSON artefact of this subset directly in `sct` (the RF2 source is freely available from MLDS without affiliate membership). This would make `sct lexical`, `sct mcp`, and `sct serve` work out-of-the-box for IPS tooling without any RF2 download step. *Requires licence verification before distribution.*
- [ ] CTV3 mappings — load the CTV3→SNOMED map from RF2 and expose via `snomed_map` MCP tool

