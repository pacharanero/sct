# SNOMED Local-First Tooling — Roadmap

Progress tracker for the full implementation described in `spec.md`.

---

## Milestone 0 — Housekeeping

Small fixes to bring the existing codebase to a clean baseline.

- [x] Initial project structure and git repository
- [x] `spec.md` written and complete
- [x] `README.md` with quick start
- [x] Fix `Cargo.toml` edition (`"2024"` is not valid; use `"2021"`)
- [x] Add schema version field to NDJSON output (`"schema_version": 1`)
- [x] Write unit tests for `rf2.rs` parsing (ConceptRow, DescriptionRow, RelationshipRow)
- [x] Write unit tests for `builder.rs` (hierarchy path, attribute labelling, preferred term selection)
- [x] CI workflow (GitHub Actions) — `cargo test`, `cargo clippy`, `cargo fmt --check`

---

## Milestone 1 — Layer 1: RF2-to-NDJSON Converter ✓

Core build tool. **Complete and production-ready.**

`sct ndjson --rf2 <DIR>`

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

## Milestone 2 — Layer 2a: SQLite + FTS5 Consumer ✓

`sct sqlite --input <NDJSON> --output snomed.db`

- [x] Decided: `sct sqlite` subcommand
- [x] Stream NDJSON input line-by-line (no full load into memory)
- [x] Create `concepts` table per spec schema
- [x] Populate all columns including JSON-encoded `synonyms`, `hierarchy_path`, `parents`, `attributes`
- [x] Create `concepts_fts` FTS5 virtual table (content table mode)
- [x] Populate FTS index (`id`, `preferred_term`, `synonyms`, `fsn`)
- [x] Add `children_count` and `module` columns not in spec but present in artefact
- [x] Also creates `concept_isa(child_id, parent_id)` table for fast hierarchy traversal
- [x] CLI: `--input`, `--output`, `--help`, progress reporting
- [x] Verify FTS queries work: `MATCH 'heart attack'`
- [x] Verify exact concept lookup works: `WHERE id = '22298006'`
- [x] Verify hierarchy filter works: `WHERE hierarchy = 'Procedure'`
- [x] Document example queries in `docs/sqlite.md`

---

## Milestone 3 — Layer 2c: Flat Markdown Consumer ✓

`sct markdown --input <NDJSON> --output snomed-concepts/ [--mode concept|hierarchy]`

- [x] Create output directory structure by top-level hierarchy (slugified: `clinical-finding/`, `procedure/`, etc.)
- [x] Stream NDJSON and write one file per concept (`{sctid}.md`)
- [x] `--mode hierarchy`: one file per top-level hierarchy (`clinical-finding.md`, `procedure.md`, etc.)
- [x] Markdown template per spec: title = preferred term, sections for FSN, hierarchy breadcrumb, synonyms, relationships, hierarchy tree
- [x] Slug hierarchy names consistently (lowercase, hyphens)
- [x] CLI: `--input`, `--output`, `--mode`, `--help`
- [x] Verify output is readable by `cat`, renderable by standard Markdown tooling
- [x] Verify concept files are findable with `grep`, `ripgrep`, `fzf`
- [x] Document in `docs/markdown.md`; suitable for RAG indexing and filesystem MCP

---

## Milestone 4 — Layer 2b: DuckDB / Parquet Consumer ✓

`sct parquet --input <NDJSON> --output snomed.parquet`

- [x] Using Apache `parquet` crate (v53) via `arrow`
- [x] Arrow schema: scalar columns + JSON-string columns for arrays/objects
- [x] Stream NDJSON and write in batches of 50,000 rows
- [x] CLI: `--input`, `--output`, `--help`
- [x] Verify DuckDB can query without import: `SELECT ... FROM 'snomed.parquet'`
- [x] Verify `GROUP BY hierarchy` analytics query from spec works
- [x] Document example DuckDB queries in `docs/parquet.md`

---

## Milestone 5 — Layer 4: MCP Server ✓

`sct mcp --db snomed.db`

- [x] JSON-RPC 2.0 over stdio with Content-Length framing (MCP protocol 2024-11-05)
- [x] Implement `snomed_search` tool — FTS5 query, returns id + preferred_term + fsn + hierarchy
- [x] Implement `snomed_concept` tool — full concept detail by SCTID
- [x] Implement `snomed_children` tool — immediate children of a concept
- [x] Implement `snomed_ancestors` tool — full ancestor chain to root (recursive CTE)
- [x] Implement `snomed_hierarchy` tool — all concepts in a named top-level hierarchy
- [x] CLI: `--db <path>`, `--help`
- [x] Read-only SQLite connection (PRAGMA query_only)
- [x] Graceful handling of unknown SCTID (returns structured message, does not panic)
- [x] schema_version validation: warn if DB is newer, refuse if too new (gap > 5 versions)
- [x] Document Claude Desktop config snippet in `docs/mcp.md`

---

## Milestone 6 — Layer 3: Vector Embeddings ✓

`sct embed --input <NDJSON> --output snomed-embeddings.arrow`

Embeds each concept via Ollama and writes an Apache Arrow IPC file for vector search.

- [x] Ollama HTTP client (`POST /api/embed`) — clear error if Ollama not running
- [x] Default model: `nomic-embed-text` (768 dim); configurable via `--model`
- [x] Configurable `--ollama-url` (default `http://localhost:11434`)
- [x] Embedding text: `"{preferred_term}. {fsn}. Synonyms: {…}. Hierarchy: {…}"`
- [x] Stream NDJSON; embed in configurable batches (`--batch-size 64`)
- [x] Write Apache Arrow IPC file with columns: `id`, `preferred_term`, `hierarchy`, `embedding` (FixedSizeList<Float32>)
- [x] Progress bar with embed count
- [x] Document Arrow IPC querying in `docs/embed.md` (DuckDB + Python examples)
- [ ] Semantic search smoke test: query "heart attack" → myocardial infarction concepts
- [ ] Expose as `snomed_semantic_search` MCP tool (future milestone)

---

## Milestone 7 — Distribution & Polish

Making the toolchain easy to install and use.

- [ ] Single workspace `Cargo.toml` with all binaries as members
- [x] Unified `sct` binary with subcommands (`sct ndjson`, `sct sqlite`, `sct parquet`, `sct markdown`, `sct mcp`, `sct embed`)
- [x] GitHub Releases with pre-built binaries for Linux x86_64, macOS arm64, macOS x86_64
- [x] GitHub Actions release workflow (triggered on `v*` tag)
- [ ] `cargo install` instructions for each tool
- [ ] End-to-end integration test: RF2 → NDJSON → SQLite → MCP query
- [ ] Checksums for NDJSON artefact in release notes
- [ ] TRUD API key support for automated RF2 download (future)

---

## Summary

| Layer | Component | Status |
|-------|-----------|--------|
| 0 | Housekeeping & CI | **Complete** |
| 1 | `sct ndjson` RF2→NDJSON | **Complete** |
| 2a | `sct sqlite` NDJSON→SQLite+FTS5 | **Complete** |
| 2b | `sct parquet` NDJSON→Parquet | **Complete** |
| 2c | `sct markdown` NDJSON→Markdown | **Complete** |
| 3 | `sct embed` NDJSON→Arrow IPC (Ollama) | **Complete** |
| 4 | `sct mcp` MCP server | **Complete** |
| 7 | Distribution & CI/CD | Partial (release CI done; `cargo install` docs pending) |
