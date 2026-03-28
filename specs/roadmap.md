# SNOMED Local-First Tooling — Roadmap

Outstanding work and next steps. Completed work is removed; see git log for history.

---

## In progress / near-term

### Distribution

- [ ] Add Windows x86_64 (`x86_64-pc-windows-msvc`) to the release CI matrix
- [ ] Homebrew formula for macOS one-liner install (`brew install sct`)
- [ ] SHA-256 checksums for NDJSON artefacts published alongside GitHub Releases

### Quality

- [ ] End-to-end integration test: RF2 → NDJSON → SQLite → MCP query (CI-runnable with the
      sample data already in the repo)
- [ ] Smoke test for `sct embed`: embed a handful of concepts, query for "heart attack", assert
      myocardial infarction concepts appear in top results

---

## Features

### `bench/` — benchmarking suite

Bash script suite for automated, fair comparison of `sct` against a FHIR R4 terminology server.
Full spec in [`specs/bench.md`](bench.md).

- [ ] `bench/bench.sh` entry point with `--server`, `--db`, `--runs`, `--format` flags
- [ ] `lib/timing.sh` — hyperfine wrapper with manual timing fallback
- [ ] `lib/fhir.sh` / `lib/local.sh` — query wrappers for each side
- [ ] `lib/report.sh` — table/JSON/CSV output
- [ ] Operations: `lookup`, `search`, `children`, `ancestors`, `subsumption`, `bulk`
- [ ] Fixtures: `concepts.txt` (15 well-known SCTIDs), `search_terms.txt` (9 terms)
- [ ] `bench/README.md` — usage docs

### `sct mcp` — semantic search tool

Add a `snomed_semantic_search` MCP tool that loads the Arrow IPC file produced by `sct embed`
and returns nearest-neighbour concepts for a natural-language query, embedding the query via
Ollama at call time.

- [ ] Accept an optional `--embeddings` flag pointing to a `.arrow` file
- [ ] Implement `snomed_semantic_search` tool (query text → top-N concepts by cosine similarity)
- [ ] Graceful degradation: if no `--embeddings` file provided, the tool is not registered

### `sct codelist` — clinical code list management

Full spec in [`specs/commands/codelist.md`](commands/codelist.md).

- [ ] `sct codelist new <filename>` — scaffold a `.codelist` file from template
- [ ] `sct codelist add <file> <sctid>` — add concept(s) to a codelist
- [ ] `sct codelist remove <file> <sctid>` — move concept to excluded record
- [ ] `sct codelist search <file> <query>` — interactive FTS5 search → include/exclude
- [ ] `sct codelist validate <file>` — CI-ready validation (exit 0 = warn, 1 = error)
- [ ] `sct codelist stats <file>` — concept counts, hierarchy breakdown, staleness
- [ ] `sct codelist diff <file-a> <file-b>` — compare two `.codelist` files
- [ ] `sct codelist export <file> --format <fmt>` — CSV, FHIR, RF2, Markdown
- [ ] `sct codelist import --from <source>` — OCL, CSV, RF2, FHIR import
- [ ] `sct codelist publish --to opencodelists` — publish to OpenCodelists

---

## Future / larger scope

- [ ] **TRUD integration** — `sct trud` subcommand that authenticates with the NHS TRUD API
      and downloads the latest UK Monolith RF2 release automatically
- [ ] **History files** — parse RF2 history substitution tables to map inactivated concept IDs
      forward to their replacements; expose via `snomed_resolve` MCP tool
- [ ] **`sct serve`** — HTTP FHIR terminology server implementing standard FHIR R4/R5
      `CodeSystem` and `ValueSet` operations (`$lookup`, `$validate-code`, `$expand`,
      `$subsumes`) backed by SQLite. Drop-in replacement for cloud terminology servers in EHR
      integration and FHIR workflow testing.
- [ ] **Concept maps** — cross-map support: load SNOMED→ICD-10/OPCS-4 map files from RF2 and
      expose via `snomed_map` MCP tool
- [ ] **IPS Free Set bundling** — investigate bundling the pre-processed NDJSON artefact of the
      SNOMED International IPS Free Set (freely available from MLDS without affiliate membership)
      to make `sct lexical`, `sct mcp`, and `sct serve` work out-of-the-box for IPS tooling
      without any RF2 download step. *Requires licence verification before distribution.*
- [ ] **CTV3 mappings** — load the CTV3→SNOMED map from RF2 and expose via `snomed_map` MCP tool

