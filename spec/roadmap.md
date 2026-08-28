# SNOMED Local-First Tooling - Roadmap

Active work only, ordered by intended delivery. Completed work is removed; use the [changelog](../CHANGELOG.md), git history, and the [July 2026 audit record](audit-2026-07.md) for shipped work and evidence. The [August 2026 assessment](assessment-2026-08.md) records where the project stands, what it has overdelivered, and the real gaps that set current priorities.

Legend: `[ ]` not started, `[~]` in progress. The `R##` sequence was deliberately reset in July 2026 to match the delivery order at that time; these identifiers are stable and are not renumbered when priorities change. Historical changelog entries, audit records, commit messages, and existing issue titles retain their pre-reset identifiers. Prose is soft-wrapped - one line per item, no hard wrapping.

## Principles and issue tracking

- The canonical NDJSON artefact remains the source for every derived format; SDK and browser artefacts must not bypass it for primary data.
- Public benchmark reporting is `sct`-solo plus the fully owned local Snowstorm Lite comparison. Commercial-server figures remain private; benchmark scripts never contain hosts or credentials.
- Bugs, bounded enhancements, and community discussion belong on the [GitHub issue tracker](https://github.com/pacharanero/sct/issues). Longer-horizon proposals live under the [`idea` label](https://github.com/pacharanero/sct/issues?q=is%3Aissue+is%3Aopen+label%3Aidea). RF2 codelist import/export remains decision-gated in [issue #60](https://github.com/pacharanero/sct/issues/60).

## Current priorities

The [August 2026 assessment](assessment-2026-08.md) found the original goal - fast, local, offline RF2-to-queryable-data - done and hardened, and the project unusually broad across surfaces (CLI, FHIR, MCP, SDK, TUI/GUI, Parquet) and distribution. The next frontier is **depth in SNOMED's own semantics and in search**, not more surfaces. Human-led delivery is therefore reprioritised in this order. The thematic sections below keep their stable `R##` grouping; this list sets delivery order across them.

1. **Search quality** (`R14`, `R15`) - the core promise every other surface rides on: clinically/frequency-aware ranking, word-prefix matching for partially-typed multi-word queries, and a semantic default backed by a clinically reviewed corpus.
2. **The description-logic layer** (`R28` SCG/OWL axioms, then `R33` MRCM) - show a concept's actual logical definition and support post-coordinated expressions, not just relationship rows. The biggest terminology-depth gap.
3. **A second code system** (`R23` dm+d medicines graph, closest to real demand, and/or `R22` ICD-10/11; LOINC is the most glaring absent system) - moves `sct` from SNOMED-access to genuine cross-terminology work.
4. **Multi-terminology codelists** (`R13`) - so codelists become durable artefacts that can mix systems, unblocked by the above.
5. **Evidence and reach** (`R20`/`R48`-`R51` published benchmarks; `R3` browser/WASM demo) - back the performance claims and let people try it without installing.

Everything below remains on the roadmap; this section only sets what comes first. It does not gate the autonomous queue, which runs in parallel on self-contained assurance work.

## Autonomous (nightly) agent queue

An autonomous agent picks the **first unstarted item in this list**, rather than choosing freely from the roadmap. An item is listed here only if it is self-contained, has a written spec or unambiguous acceptance criteria, and is completable *and verifiable* in a single session.

The `R17b` spec-derived FHIR conformance vein (`$expand`, `$lookup`, `$validate-code` in both `CodeSystem` and `ValueSet` forms, `$subsumes`, `$translate`), the `R17c` CapabilityStatement-accuracy and `R17d` `OperationOutcome`-shape checks, the `R57` codelist<->ECL round-trip, and `R58` MCP output-schema conformance for the refset and codelist tools are all **complete and shipped** (see the changelog and git history); `$translate` also fixed a real silent-wrong-answer defect (`target` aliased to `targetsystem`), `R17c` caught the CapabilityStatement under-declaring `CodeSystem` read/search-type, and `R58` closed the live-path gap `#106` left open (the in-memory `build_test_db` has no reference sets, so `snomed_refsets`/`snomed_refset_members`/`snomed_refset_compare`/`snomed_refset_profile` and the codelist read/report tools - `codelist_list`, `codelist_read` (named `codelist_show` in the original item text; the registered tool is `codelist_read`), `codelist_validate`, `codelist_stats` - were verified against a real `sct sqlite` fixture build instead). The items below continue the same assurance discipline: work from an authoritative source (an HL7 R4 page, or an in-repo invariant), add a test that **fails on drift**, keep the change self-contained and one-per-pull-request, and verify with `cargo test` (add `--features serve` for server items, `--test cli` for CLI items) against the committed synthetic fixture, cross-checking a known concept (e.g. 22298006 = Myocardial infarction, 46635009 = Type 1 diabetes mellitus, 73211009 = Diabetes mellitus). The governing invariant remains `R17`'s: unrecognised or unsupported input - and inaccurate self-description - must never silently mislead a client. Server operations are implemented at `src/commands/serve/mod.rs` and `src/commands/serve/ops.rs`.

- [ ] `R59` **CLI stdout/stderr and exit-code discipline test.** The [`AGENTS.md`](../AGENTS.md) invariant - machine-readable output on **stdout**, human hints/progress/warnings/"not found" on **stderr**, exit `0` success / `1` unresolved single-item lookup / `2` usage error - is currently enforced only by convention. Transcribe it into a small table and assert it over the real `sct` CLI binary against the committed fixture (as `tests/cli.rs` already does with `assert_cmd`). Cover at minimum: `sct lookup <known SCTID>` (result on stdout, exit 0); `sct lookup <unknown SCTID>` (empty stdout, hint on stderr, exit 1); a lexical search with no matches (empty stdout, hint on stderr, exit 0, and `--format json` emits an empty array on stdout); and a usage error such as an unknown flag (exit 2, message on stderr, empty stdout). Cross-check a known concept (22298006 = Myocardial infarction). Self-contained; no new dependency. Verify with `cargo test --test cli`.

`R17a` (external HL7-Validator structural checks) stays **out** of this queue: it needs a Java runtime, the validator jar, and a FHIR package cache in CI - a human cost decision. `R12`'s two remaining compression pieces are both shipped (see the changelog and git history). `R16`'s remaining work - the useful FHIR R5 additions - is still ineligible: no specific first R5 operation/parameter is scoped yet, and a human should pick one before it returns to the autonomous queue. Note that `$expand` has no `property` parameter in R4 - that is an R5 addition, so it belongs with that remaining R5 work, not the shipped R4 surface.

Everything else on this roadmap needs a human design decision, spans several sessions, depends on licensed content or an external account, or needs before/after benchmarks against a real release. Do not begin those autonomously; comment on the item or open an issue instead. When this queue is empty, say so rather than substituting unlisted work.

**Verification contract.** Query logic must be validated against the committed synthetic RF2 fixture in `tests/fixtures/rf2/` through the real `sct sqlite` schema, and cross-checked against a known concept (see [`AGENTS.md`](../AGENTS.md)). A hand-built in-memory schema is a useful unit-test convenience but is **not** sufficient evidence of correctness: it drifts from the real DDL, most often over nullability and absent columns. Where the environment cannot run part of the gate, say so plainly in the pull request and name the specific checks that were skipped.

## Terminology capability

- [ ] `R13` **Design multi-terminology codelists (format v2).** Allow first-class non-SNOMED source codes where SNOMED is not an honest canonical pivot, while preserving the current format and `--include-maps` workflow for SNOMED-canonical lists. Treat migration, validation, and FHIR export semantics as design gates rather than merely adding a `system` column.

- [ ] `R14` **Improve SAYT ranking and query refinement.** Add clinically useful/frequency-aware ranking, word-prefix matching for partially typed multi-word queries, and hierarchy/semantic-tag filters; evaluate an explicit backend selector across FST, FTS5, and semantic search against a fixed query set.

- [x] `R56` **Make embedding model-aware before benchmarking semantic quality.** A curated shared registry now gives Nomic v1.5, Nomic v2 MoE, Qwen3 Embedding 0.6B, and EmbeddingGemma distinct versioned document/query adapters, expected dimensions and context constraints. `sct embed` records the profile in Arrow metadata; `sct semantic` and MCP fail closed on model/profile mismatches; unsupported Ollama names fail before an expensive build; existing Nomic scheme-2 artefacts remain readable. Every profile was dimension-probed and exercised end-to-end against the committed synthetic fixture with real Ollama on 18 August 2026. Model downloads remain explicit and runtime stays local. See [`spec/commands/embed.md`](commands/embed.md#model-aware-adapters-r56).

- [~] `R15` **Improve semantic-search result quality (depends on R56).** The first dense baseline runner is now available as `sct bench semantic`: it embeds a versioned five-case regression corpus, validates expected concepts against the artefact, records release/model/profile provenance, retains full ranked evidence, and reports aggregate/per-class quality with separate query-embedding and Arrow-scan timings. Next: expand to a clinically reviewed 50-100 case corpus, benchmark every R56 profile on matching full-release artefacts, then evaluate per-synonym max pooling, hybrid lexical/vector ranking, and clinically tuned models before selecting an implementation or changing the default. Report retrieval quality, build/query latency, model memory, vector dimensions, and Arrow artefact size so model-specific quality/resource trade-offs remain visible. See [`docs/commands/semantic.md`](../docs/commands/semantic.md#known-limitations) and [`spec/commands/embed.md`](commands/embed.md).

- [~] `R16` **Complete the practical FHIR terminology surface.** `$expand` parameters (`activeOnly`, `displayLanguage`, designation controls/filters, system/value-set versions - `property` is R5-only and belongs with the R5 additions below), CodeSystem resource read (`GET /CodeSystem`, `GET /CodeSystem/{id}`), and stored-ValueSet canonical URL override plus `GET /ValueSet?status=` draft filtering are done; remaining: the useful FHIR R5 additions. Keep multi-version routing and national syndication explicitly out of scope until there is a concrete consumer.

## Browser SDK

The SDK programme has shipped native Rust and Python APIs over the same engine. A later stage brings a measured subset to browsers. Full architecture, acceptance criteria, data constraints, docs information architecture, and licensing rationale: [`sdk.md`](sdk.md).

- [ ] `R3` **Compile the Rust engine to WebAssembly and ship a local-only browser demo.** Make storage-independent logic `wasm32-unknown-unknown` clean, then select a browser query backend through a measured spike: preferably a compact `.sct-web` artefact derived locally from canonical NDJSON, compared against official SQLite WASM/OPFS. The docs-hosted static demo ships only the synthetic fixture; users generate/select their own licensed artefact, all content and queries remain in-browser, and no SNOMED data is hosted or uploaded. Initial useful scope: lookup, autocomplete, hierarchy/subsumption, and a documented ECL subset, validated against shared Rust/WASM fixtures with browser memory/load benchmarks.

## Foundation and interface consistency

This work should reuse shared engine contracts rather than reimplementing them.

- [ ] `R38` **Refresh `sct gui` as a clinical knowledge atlas.** Replace the experimental fixed dashboard with an offline, search-first, responsive terminology explorer whose stable knowledge graph, concept workspace, mappings/history, URL navigation, and accessibility are verified through a Playwright feedback loop. Follow the stable `GUI-1` through `GUI-8` delivery stages in [`gui.md`](gui.md); ship the polished search-to-concept vertical slice before expanding graph and specialist-query scope.

- [ ] `R54` **Ship `sct-lens`, system-wide clinical terminology lookup.** A Tauri desktop companion - global hotkeys to look up, search, and annotate SNOMED CT codes from any application - backed locally by an in-process `sct-rs` SDK link (no remote terminology server required, unlike the prior art). Lives as a sibling Cargo project (`lens/`), following the exact pattern `python/` already establishes, not a `sct` CLI feature and not a separate repository. Ideas harvested from `aehrc/codeagogo`/`codeagogo-win` (Apache-2.0), clean-room Rust reimplementation. Follow the staged `LENS-1` through `LENS-4` delivery plan in [`lens.md`](lens.md); ship the hotkey-plus-lookup vertical slice first to de-risk the cross-platform OS-integration layer before search, ECL, and visualisation.

## Assurance, documentation, and evidence

- [~] `R17` **Add externally verified FHIR conformance.** Brought forward (August 2026) after three independent silent-wrong-answer defects in `ValueSet/$expand` were found by reading the R4 specification rather than by the test suite: designations emitted in the `$lookup` shape, `?fhir_vs=isa/` and `refset/` expanding the whole code system, and a POST body (the standard invocation, and the only way to send an inline `valueSet`) being discarded so the operation expanded everything. The committed suite passed throughout, because it tested what we believed we had implemented.

  Deliver in two distinct stages, because they catch different faults:

  - `R17a` **External structural validation.** Run the HL7 FHIR Validator over responses captured from `sct serve` on the committed synthetic fixture, and gate it in CI. This catches malformed resources - it would have caught the designation defect immediately - but note it would **not** have caught the other two, which returned perfectly well-formed resources describing the wrong value set. Needs a Java runtime, the validator jar, and a FHIR package cache in CI: a real cost to weigh before committing.
  - [x] `R17b` **Spec-derived semantic coverage.** Shipped in [`tests/fhir_conformance.rs`](../tests/fhir_conformance.rs), gated by the existing `cargo test --features serve` CI step. It transcribes all 21 R4 `$expand` input parameters and requires each to have an asserted disposition - honoured, refused, or provably unable to affect the result (checked by comparing the expansion against a baseline). A parameter that is merely ignored can satisfy none of them. It found a fourth defect on first run: `valueSetVersion` was silently ignored, so a client pinning a value set version got whatever was on disk. Extend the same treatment to `$lookup`, `$validate-code`, `$subsumes`, and `$translate` next.

  The governing invariant, which all three defects violated: **unrecognised or unsupported input must never silently degrade to a broader default.** Refuse it and say so. Keep describing the home-grown suite as HL7-aligned, not certified; keep Touchstone/TestScript as a later complement.

- [~] `R20` **Complete and publish the benchmark suite.** Preserve the working Bash suite while the parity-gated typed-runner programme (`R48`-`R51`) lands, then broaden the committed FHIR conformance scenarios, add comparator compose profiles, compare SDK/CLI/FST/FTS/server boundaries honestly, and publish reproducible `sct`-solo reports under the reporting policy above. Architecture and evidence contract: [`benchmark-runner.md`](benchmark-runner.md).

- [ ] `R48` **Build the typed benchmark contract and vertical slice.** Add a non-shipped Rust runner with versioned scenario/result types, raw samples, fail-closed target handling, and text/JSON/Markdown rendering; migrate lookup and lexical search across SDK, CLI, `sct serve`, and an arbitrary FHIR target while keeping every Bash path until parity is proven.

- [ ] `R49` **Benchmark `sct` at its real internal boundaries.** Extend Criterion and the runner across SDK, CLI, FST/FTS, hierarchy, subsumption, ECL, startup, and artefact-size profiles; use the synthetic fixture for automated smoke coverage and opt-in licensed releases for publication-quality runs, with raw SQLite retained only as a clearly labelled diagnostic.

- [ ] `R50` **Migrate comparative FHIR latency and conformance.** Use the shared scenarios for capability discovery, semantic preflight, identical HTTP requests, multi-target latency reports, and the synthetic CI conformance gate; fail rather than silently dropping an unavailable comparator, and keep official external validation under `R17` distinct.

- [ ] `R51` **Migrate load evidence and retire benchmark Bash.** Have the runner orchestrate `oha`, preserve raw load JSON, capture environment/topology/resource metadata, render scaling curves, and remove `bench.sh`, `load.sh`, `conformance.sh`, and their sourced libraries only after fixture, failure, and report parity; retain thin shell wrappers for Docker and OS profilers.

- [ ] `R21` **Publish separated `sct serve` scaling curves.** After the `R51` load profile lands, run it from a separate client machine, sweep `--pool-size`, and publish throughput/latency versus concurrency so client compute no longer distorts server capacity.

## Larger product capabilities

- [ ] `R22` **Add first-class ICD-10 and ICD-11 code systems.** Introduce generic code-system/code/relationship storage rather than forcing ICD into SNOMED concepts; import locally supplied ICD-10 first, then ICD-11 MMS; extend lookup, search, codelist validation/export, and FHIR CodeSystem operations. Preserve source URI/version/licence provenance, do not redistribute source content, and do not assume a public production SNOMED-to-ICD-11 map. Research sources: [WHO ICD API](https://icd.who.int/icdapi/docs2/SupportedClassifications/), [ICD-11 MMS](https://icd.who.int/browse/2026-01/mms/en), [ICD-11 licence](https://icd.who.int/en/docs/ICD11-license.pdf), and the [NHS Classifications Browser](https://classbrowser.nhs.uk/).

- [ ] `R23` **Build the dm+d medicines graph from barcode to class.** Ingest NHSBSA dm+d weekly XML/GTIN data through a new TRUD edition, normalise GTIN-8/12/13/14, map GTIN -> AMPP, traverse AMPP/AMP/VMP/VTM relationships already present in the Monolith, then import supplementary BNF/ATC mappings into the generic classification model from `R22`. Credit community PR [#38](https://github.com/pacharanero/sct/pull/38) for the query/display direction; local ingestion is mandatory because dm+d source content is not redistributed.

- [ ] `R24` **Investigate an IPS Free Set starter artefact.** Verify the licence, then decide whether a preprocessed IPS Free Set NDJSON/database can make lookup, MCP, SDK, and the WASM demo useful out of the box without affiliate-only RF2 content.

- [ ] `R25` **Support point-in-time and through-time reporting.** Ingest Full RF2 to reconstruct terminology at an `effectiveTime` and report changes in ancestry, descendants, refset membership, and concept lifecycle across releases; extend the shipped two-release diff and Association forwarding rather than creating a separate temporal model.

- [ ] `R27` **Add named-set algebra over ECL results.** Let users name query results and combine them with AND/OR/NOT, feeding named sets into subsequent ECL/codelist work as transient refsets.

- [ ] `R28` **Handle SCG and OWL axioms.** Parse and pretty-print Semantic Compositional Grammar and the OWL axiom refset so concept views can show the actual description-logic definition rather than only relationship rows.

## Performance experiments

These are measured candidates, not committed architecture. Each needs a before/after benchmark and an explicit complexity/risk decision.

- [ ] `R29` **Evaluate DB-wide INTEGER SCTID columns.** Measure size/build/query gains from converting every SCTID-bearing column together, including JOIN affinity, FTS5 rowid semantics, schema migration/rebuild, and keeping non-numeric crossmap codes as TEXT. The TCT's INTEGER conversion already demonstrated the potential benefit.

- [ ] `R30` **Evaluate an optional one-pass RF2-to-SQLite build.** Quantify eliminating the large NDJSON write/read cycle for users who only need SQLite, while preserving NDJSON as the default canonical artefact and making any direct path explicitly additive and opt-in.

- [ ] `R31` **Evaluate an in-memory subsumption index and a larger mmap window.** First test a bundled-SQLite `SQLITE_MAX_MMAP_SIZE` override because Monolith databases exceed the current ~2 GiB clamp; then benchmark a server-only roaring-bitmap/CSR IS-A representation against warm mmap plus TCT for large ECL expansion and subsumption workloads. Do not add a naive whole-database `:memory:` copy.

## Specialist and licence-gated extensions

- [ ] `R32` **Import further crossmap targets where licensing permits.** Evaluate HPO and NICIP first; treat MedDRA and HGNC as licence-gated. Reuse the generic crossmaps model and preserve source/version/licence provenance.

- [ ] `R33` **Render MRCM constraints.** Parse and diagram Machine-Readable Concept Model domain, attribute, and range constraints for content-authoring and post-coordination QA.

- [ ] `R55` **Make `sct` an OMOP/OHDSI terminology companion.** Preserve the RF2/NDJSON-first engine and keep patient data out of scope; start with a synthetic-fixture-backed Athena identity resolver (`omop_concept_id` to/from vocabulary/code to/from SCTID), then evaluate exact ATLAS concept-set import/export, lifecycle/mapping audits, and R/Python affordances under the design and licensing gates in [`omop.md`](omop.md).

## Deferred distribution gates

The existing release routes already cover crates.io, installers, Homebrew tap, Scoop bucket, Docker Hub, binaries, checksums, deb/rpm, and unsigned DMGs. These remaining items depend on external accounts, certificates, adoption, or a preceding gate, so they sit at the end of the active plan.

- [ ] `R34` **Sign and notarise macOS releases.** Requires an Apple Developer ID and annual fee.
- [ ] `R35` **Add Windows Authenticode signing.** Requires an appropriate certificate; this unblocks a smoother SmartScreen experience and `R37`. See [`windows-code-signing.md`](windows-code-signing.md) for the 2026 options, step-by-step Azure Artifact Signing setup, costs, time estimates, and the Organization-vs-Individual validation decision.
- [ ] `R36` **Submit to homebrew-core.** Wait for the project to meet its adoption/cadence expectations (currently around 30+ stars); the existing tap remains the supported route meanwhile.
- [ ] `R37` **Submit to winget.** Do this after Windows signing is operational (depends on `R35`; see [`windows-code-signing.md`](windows-code-signing.md)).

## Exploratory ideas

Data-science adapters, notebooks, graph exports, editor/desktop integrations, clinical-data bridges, LLM-assisted authoring, visualisations, and `sct mud` live as [issues labelled `idea`](https://github.com/pacharanero/sct/issues?q=is%3Aissue+is%3Aopen+label%3Aidea), where the community can comment and add evidence. Promote an idea into this roadmap only when it has a likely delivery window and an owner.
