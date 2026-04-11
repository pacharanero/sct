# SNOMED Local-First Tooling — Roadmap

Outstanding work and next steps. Completed work is removed; see git log for history.

---

## In progress / near-term

### Distribution

- [x] Add Windows x86_64 (`x86_64-pc-windows-msvc`) to the release CI matrix (v0.3.9)
- [x] Add Linux aarch64 (`aarch64-unknown-linux-musl`) to the release CI matrix (v0.3.9,
      native `ubuntu-24.04-arm` runner)
- [x] SHA-256 checksums published alongside GitHub Releases (v0.3.9, `SHA256SUMS` file)
- [x] `curl | sh` installer (`install.sh`) — auto-detects OS/arch, verifies checksum,
      installs to `~/.local/bin`
- [x] PowerShell installer (`install.ps1`) — Windows equivalent of `install.sh`
- [x] cargo-binstall support — `cargo binstall sct-rs` pulls prebuilt tarballs
- [x] Homebrew tap (`pacharanero/homebrew-sct`) — `brew tap pacharanero/sct && brew install sct`,
      supports macOS arm64/x86_64 and Linux arm64/x86_64, auto-bumped by release workflow
- [x] Scoop bucket (`pacharanero/scoop-sct`) — `scoop bucket add sct ... && scoop install sct`,
      auto-bumped by release workflow

**Future distribution work:**

- [ ] macOS code signing + notarization (requires Apple Developer ID, $99/yr) so users
      don't have to `chmod +x` and bypass Gatekeeper
- [ ] Windows Authenticode signing (requires cert from CA) so SmartScreen doesn't block
- [ ] `.deb` / `.rpm` via `cargo-deb` / `cargo-generate-rpm`, attached to GitHub Releases
- [ ] Submit to `homebrew-core` once project hits 30+ stars and has stable release cadence
      (would enable `brew install sct` without the tap)
- [ ] Submit to `winget` after Windows signing is in place
- [ ] Nix flake

### Quality

- [ ] End-to-end integration test: RF2 → NDJSON → SQLite → MCP query (CI-runnable with the
      sample data already in the repo)
- [ ] Smoke test for `sct embed`: embed a handful of concepts, query for "heart attack", assert
      myocardial infarction concepts appear in top results
- [ ] **End-to-end CLI tests** with `assert_cmd` — run `sct` as a binary against tiny fixtures
      under `tests/fixtures/` and assert on exit codes, output files, and stdout. Would cover
      contract-level regressions (argument parsing, file naming, `sct trud check` exit-2
      semantics, `sct codelist validate` exit codes) that inline unit tests cannot.
- [ ] **Network-layer tests for `sct trud`** using `wiremock` to stand up a fake TRUD API.
      The current 41 trud tests are all pure helpers — `fetch_releases`, `probe_edition`,
      `run_download`, and the SHA-256 mismatch / re-download paths are entirely untested.
- [ ] **De-flake trud tests' environment variables** — the `HOME` / `SCT_DATA_HOME` tests use
      `unsafe { std::env::set_var(...) }` while `cargo test` runs in parallel. Currently
      passing but fragile; `temp-env` or `serial_test` would remove the global-state race.
- [ ] **Snapshot tests for formatted output** — `sct diff`, `sct trud list`, `sct info` all
      emit human-readable tables/summaries. `insta` would freeze the current shape and catch
      accidental format regressions without hand-written `contains` assertions.
- [ ] **Doctests on library public items** — now that `src/lib.rs` exposes `build_records`,
      `Rf2Dataset::load`, the rf2 parsers, etc. as genuine library surface, `///` examples on
      these items double as living documentation and get tested by `cargo test` for free.
- [ ] **Coverage measurement** — run `cargo-tarpaulin` (or similar) in CI to surface blind
      spots. `src/commands/mcp.rs` is ~1,800 lines; worth knowing which tool handlers are
      lightly covered.

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

## Completed

- [x] **TRUD integration** — `sct trud` subcommand authenticates with the NHS TRUD API and
      downloads UK releases automatically, with SHA-256 verification, pre-flight health check,
      optional `--pipeline` / `--pipeline-full` chaining, and standardised `~/.local/share/sct/`
      directory layout. Full spec in [`specs/commands/trud.md`](commands/trud.md) and user docs
      in [`docs/commands/trud.md`](../../docs/commands/trud.md).
      Key TRUD item numbers: item **1799** (UK Monolith), item **101** (UK Clinical), item **105** (UK Drug/dm+d).

---

## Future / larger scope
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

  **Phase 3 — Refsets + ConceptMap** (`^` ECL member-of operator now unblocked — Simple
  refsets load into the `refset_members` table via `sct ndjson --refsets simple` + `sct sqlite`;
  `ConceptMap/$translate` for CTV3, Read v2, ICD-10, OPCS-4; complex/map/association refsets
  still to come via `--refsets all`)

  **Phase 4 — R5 + hardening** (FHIR R5 CapabilityStatement; named ValueSet registry;
  Docker image / systemd unit; full ECL attribute filter support — stretch goal)
- [ ] **`sct ndjson --refsets all`** — extend RF2 ingestion beyond Simple refsets to cover the
      remaining derivative-2 refset shapes. The CLI flag and `RefsetMode::All` enum variant
      already exist (added with the Simple refset work) and currently bail with "not yet
      implemented". Concretely needs:
      - **Complex refsets** (`der2_Refset_Complex*Snapshot*.txt`) — adds attribute payload columns
        beyond simple membership; needs a wider row type and a strategy for surfacing those
        attributes to downstream consumers
      - **Association refsets** (`der2_cRefset_Association*Snapshot*.txt`) — `SAME_AS`,
        `REPLACED_BY`, `MAY_BE_A`, etc. Foundation for the `History files` item below
      - **Attribute value refsets** (`der2_cRefset_AttributeValue*Snapshot*.txt`) — concept-to-value
        annotations used by some UK national refsets
      - **Extended map refsets** (`der2_iissssRefset_ExtendedMap*Snapshot*.txt`) — structured
        SNOMED→ICD-10 / OPCS-4 / LOINC map data; needs a new `concept_maps_rf2` table (designed
        in `specs/commands/serve.md`) to capture map_group, map_priority, map_rule, map_advice,
        correlation. This is the prerequisite for full `ConceptMap/$translate` in `sct serve`
        beyond the CTV3/Read v2 maps already supported.

      Each refset family gets its own table or column extension; `refset_members` (concept-only,
      already shipped) stays as-is.

- [ ] **Concept maps** — cross-map support: load SNOMED→ICD-10/OPCS-4 map files from RF2 and
      expose via `snomed_map` MCP tool
- [ ] **IPS Free Set bundling** — investigate bundling the pre-processed NDJSON artefact of the
      SNOMED International IPS Free Set (freely available from MLDS without affiliate membership)
      to make `sct lexical`, `sct mcp`, and `sct serve` work out-of-the-box for IPS tooling
      without any RF2 download step. *Requires licence verification before distribution.*
