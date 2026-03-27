# SNOMED Local-First Tooling — Roadmap

Progress tracker for the full implementation described in `spec.md`.

---

## Milestone 0 — Housekeeping

Small fixes to bring the existing codebase to a clean baseline.

- [x] Initial project structure and git repository
- [x] `spec.md` written and complete
- [x] `README.md` with quick start
- [ ] Fix `Cargo.toml` edition (`"2024"` is not valid; use `"2021"`)
- [ ] Add schema version field to NDJSON output (`"schema_version": 1`)
- [ ] Write unit tests for `rf2.rs` parsing (ConceptRow, DescriptionRow, RelationshipRow)
- [ ] Write unit tests for `builder.rs` (hierarchy path, attribute labelling, preferred term selection)
- [ ] CI workflow (GitHub Actions) — `cargo test`, `cargo clippy`, `cargo fmt --check`

---

## Milestone 1 — Layer 1: RF2-to-NDJSON Converter ✓

Core build tool. **Complete and production-ready.**

- [x] RF2 file discovery (walkdir, pattern matching for Snapshot TSV files)
- [x] Concept parsing (`sct2_Concept_Snapshot_*.txt`)
- [x] Description parsing (`sct2_Description_Snapshot_*.txt`)
- [x] Relationship parsing (`sct2_Relationship_Snapshot_*.txt`) — IS-A + attribute types
- [x] Language refset parsing (`der2_cRefset_Language_*.txt`) — locale-aware preferred terms
- [x] Attribute type ID → human-readable label mapping
- [x] Hierarchy path traversal (ancestor chain, cycle-guarded)
- [x] Children count computation
- [x] Deterministic output (stable sort by concept ID)
- [x] `--include-inactive` flag
- [x] Multiple RF2 directory layering (base + extension)
- [x] `--locale` flag (BCP-47, defaults to `en-GB`)
- [x] Smart output filename derivation from RF2 directory name
- [x] Validated against UK SNOMED CT Monolith (831,132 concepts, ~10s runtime)

---

## Milestone 2 — Layer 2a: SQLite + FTS5 Consumer

New binary `snomed-sqlite` (or subcommand `sct sqlite`). Reads the NDJSON artefact and produces a single `snomed.db`.

- [ ] Decide on delivery: separate binary or `sct sqlite` subcommand
- [ ] Stream NDJSON input line-by-line (no full load into memory)
- [ ] Create `concepts` table per spec schema
- [ ] Populate all columns including JSON-encoded `synonyms`, `hierarchy_path`, `parents`, `attributes`
- [ ] Create `concepts_fts` FTS5 virtual table (content table mode)
- [ ] Populate FTS index (`id`, `preferred_term`, `synonyms`, `fsn`)
- [ ] Add `children_count` and `module` columns not in spec but present in artefact
- [ ] CLI: `--input`, `--output`, `--help`, progress reporting
- [ ] Verify FTS queries work: `MATCH 'heart attack'`
- [ ] Verify exact concept lookup works: `WHERE id = '22298006'`
- [ ] Verify hierarchy filter works: `WHERE hierarchy = 'Procedure'`
- [ ] Document example queries in README

---

## Milestone 3 — Layer 2c: Flat Markdown Consumer

New binary `snomed-markdown` (or subcommand). Produces one `.md` file per concept, organised by hierarchy.

- [ ] Create output directory structure by top-level hierarchy (slugified: `clinical-finding/`, `procedure/`, etc.)
- [ ] Stream NDJSON and write one file per concept (`{sctid}.md`)
- [ ] Markdown template per spec: title = preferred term, sections for FSN, hierarchy breadcrumb, synonyms, relationships, hierarchy tree
- [ ] Slug hierarchy names consistently (lowercase, hyphens)
- [ ] CLI: `--input`, `--output`, `--help`
- [ ] Verify output is readable by `cat`, renderable by standard Markdown tooling
- [ ] Verify concept files are findable with `grep`, `ripgrep`, `fzf`
- [ ] Add note in README: suitable for RAG indexing and filesystem MCP

---

## Milestone 4 — Layer 2b: DuckDB / Parquet Consumer

New binary `snomed-parquet` (or subcommand). Produces a single `.parquet` file directly queryable by DuckDB.

- [ ] Choose Rust Parquet library (Apache `parquet` crate via `arrow`)
- [ ] Define Arrow schema (scalar columns + JSON-string columns for arrays/objects)
- [ ] Stream NDJSON and write Parquet row-by-row or in batches
- [ ] CLI: `--input`, `--output`, `--help`
- [ ] Verify DuckDB can query without import: `SELECT ... FROM 'snomed.parquet'`
- [ ] Verify `GROUP BY hierarchy` analytics query from spec works
- [ ] Document example DuckDB queries in README

---

## Milestone 5 — Layer 4: MCP Server

New binary `snomed-mcp`. Wraps the SQLite database and exposes it as a local MCP server over stdio.

- [ ] Choose MCP Rust SDK or implement stdio JSON-RPC transport from scratch
- [ ] Implement `snomed_search` tool — FTS5 query, returns id + preferred_term + fsn + hierarchy
- [ ] Implement `snomed_concept` tool — full concept detail by SCTID
- [ ] Implement `snomed_children` tool — immediate children of a concept
- [ ] Implement `snomed_ancestors` tool — full ancestor chain to root
- [ ] Implement `snomed_hierarchy` tool — all concepts in a named top-level hierarchy
- [ ] CLI: `--db <path>`, `--help`
- [ ] Startup time under 100ms
- [ ] Read-only SQLite connection
- [ ] Graceful handling of unknown SCTID (return structured error, not panic)
- [ ] Test with Claude Desktop `claude_desktop_config.json`
- [ ] Document Claude Desktop config snippet in README
- [ ] Publish as `cargo install snomed-mcp` target

---

## Milestone 6 — Layer 3: Vector Embeddings

New binary `snomed-embed`. Embeds each concept and writes a local LanceDB vector index.

- [ ] Choose embedding approach: local model via `candle` / `ort` (ONNX Runtime) or Ollama HTTP client with `nomic-embed-text`
- [ ] Define embedding text format: `"{preferred_term}. {fsn}. Synonyms: {…}. Hierarchy: {…}"`
- [ ] Stream NDJSON and embed in batches
- [ ] Write LanceDB Lance directory
- [ ] CLI: `--input`, `--model`, `--output`, `--help`
- [ ] Semantic search smoke test: query "heart attack" → returns myocardial infarction concepts
- [ ] Document how to query the Lance index from Python / Rust
- [ ] Consider: expose semantic search as an additional `snomed_semantic_search` MCP tool

---

## Milestone 7 — Distribution & Polish

Making the toolchain easy to install and use.

- [ ] Single workspace `Cargo.toml` with all binaries as members
- [ ] Unified `sct` binary with subcommands (`sct build`, `sct sqlite`, `sct parquet`, `sct markdown`, `sct embed`, `sct mcp`) — or keep separate binaries; decide and document
- [ ] GitHub Releases with pre-built binaries for Linux x86_64, macOS arm64, macOS x86_64
- [ ] GitHub Actions release workflow (triggered on tag)
- [ ] `cargo install` instructions for each tool
- [ ] End-to-end integration test: RF2 → NDJSON → SQLite → MCP query
- [ ] Checksums for NDJSON artefact in release notes
- [ ] TRUD API key support for automated RF2 download (future)

---

## Summary

| Layer | Component | Status |
|-------|-----------|--------|
| 0 | Housekeeping & CI | Partial |
| 1 | `sct` RF2→NDJSON | **Complete** |
| 2a | `snomed-sqlite` NDJSON→SQLite | Not started |
| 2b | `snomed-parquet` NDJSON→Parquet | Not started |
| 2c | `snomed-markdown` NDJSON→Markdown | Not started |
| 3 | `snomed-embed` NDJSON→LanceDB | Not started |
| 4 | `snomed-mcp` MCP server | Not started |
| 7 | Distribution & CI/CD | Not started |
