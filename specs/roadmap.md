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

### `sct codelist` — clinical code list management (in progress)

Full spec in [`specs/commands/codelist.md`](commands/codelist.md).

- [x] `sct codelist new <filename>` — scaffold a `.codelist` file from template
- [x] `sct codelist add <file> <sctid>` — add concept(s) to a codelist
- [x] `sct codelist remove <file> <sctid>` — move concept to excluded record
- [x] `sct codelist validate <file>` — CI-ready validation (exit 0 = warn, 1 = error)
- [x] `sct codelist stats <file>` — concept counts, hierarchy breakdown, staleness
- [x] `sct codelist diff <file-a> <file-b>` — compare two `.codelist` files
- [x] `sct codelist export <file> --format csv/opencodelists-csv/markdown`
- [ ] `sct codelist export <file> --format fhir-json/rf2` — remaining export formats
- [ ] `sct codelist search <file> <query>` — interactive FTS5 search → include/exclude
- [ ] `sct codelist import --from <source>` — OCL, CSV, RF2, FHIR import
- [ ] `sct codelist publish --to opencodelists` — publish to OpenCodelists

---

## Future / larger scope

- [ ] **TRUD integration** — `sct trud` subcommand that authenticates with the NHS TRUD API
      and downloads the latest UK release automatically. Full spec in [`specs/commands/trud.md`](commands/trud.md).
      Key TRUD item numbers: item **1799** (UK Monolith — Snapshot only; includes International + UK Clinical + UK Drug/dm+d + UK Pathology),
      item **101** (UK Clinical Edition — Full/Snapshot/Delta), item **105** (UK Drug Extension/dm+d — Full/Snapshot/Delta)
- [ ] **History files** — parse RF2 history substitution tables to map inactivated concept IDs
      forward to their replacements; expose via `snomed_resolve` MCP tool
- [ ] **`sct serve`** — HTTP FHIR R4 terminology server backed by SQLite. Drop-in replacement
      for Ontoserver, Snowstorm, and the NHS FHIR Terminology Server. Full spec in
      [`specs/commands/serve.md`](commands/serve.md).

  **Phase 1 — Core operations** (`$lookup`, `$validate-code`, `$subsumes`, `$expand` with
  text filter, CapabilityStatement, OperationOutcome errors, FHIR batch Bundle)

  **Phase 2 — ECL hierarchy** (`ValueSet/$expand` with `<<`, `<!`, `>>`, `>!`, boolean
  operators; pagination; `ValueSet/$validate-code`; `CodeSystem` resource read;
  `--fhir-base` path prefix for Ontoserver-compatible URLs)

  **Phase 3 — Refsets + ConceptMap** (requires refset tables in `sct sqlite`; `^` ECL
  member-of operator; `ConceptMap/$translate` for CTV3, Read v2, ICD-10, OPCS-4)

  **Phase 4 — R5 + hardening** (FHIR R5 CapabilityStatement; named ValueSet registry;
  Docker image / systemd unit; full ECL attribute filter support — stretch goal)
- [ ] **Concept maps** — cross-map support: load SNOMED→ICD-10/OPCS-4 map files from RF2 and
      expose via `snomed_map` MCP tool
- [ ] **IPS Free Set bundling** — investigate bundling the pre-processed NDJSON artefact of the
      SNOMED International IPS Free Set (freely available from MLDS without affiliate membership)
      to make `sct lexical`, `sct mcp`, and `sct serve` work out-of-the-box for IPS tooling
      without any RF2 download step. *Requires licence verification before distribution.*
