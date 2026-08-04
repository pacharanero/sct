# SNOMED Local-First Tooling - Roadmap

Active work only, ordered by intended delivery. Completed work is removed; use the [changelog](../CHANGELOG.md), git history, and the [July 2026 audit record](audit-2026-07.md) for shipped work and evidence.

Legend: `[ ]` not started, `[~]` in progress. The `R##` sequence was deliberately reset in July 2026 to match the delivery order at that time; these identifiers are stable and are not renumbered when priorities change. Historical changelog entries, audit records, commit messages, and existing issue titles retain their pre-reset identifiers. Prose is soft-wrapped - one line per item, no hard wrapping.

## Principles and issue tracking

- The canonical NDJSON artefact remains the source for every derived format; SDK and browser artefacts must not bypass it for primary data.
- Public benchmark reporting is `sct`-solo plus the fully owned local Snowstorm Lite comparison. Commercial-server figures remain private; benchmark scripts never contain hosts or credentials.
- Bugs, bounded enhancements, and community discussion belong on the [GitHub issue tracker](https://github.com/pacharanero/sct/issues). Longer-horizon proposals live under the [`idea` label](https://github.com/pacharanero/sct/issues?q=is%3Aissue+is%3Aopen+label%3Aidea). RF2 codelist import/export remains decision-gated in [issue #60](https://github.com/pacharanero/sct/issues/60).

## Terminology capability

- [~] `R10` **Parse the remaining RF2 refset families.** Add ComplexMap and AttributeValue refsets without overloading the concept-only `refset_members` table; preserve the complete RF2 member envelope and family payload in canonical NDJSON and dedicated SQLite tables. AttributeValue ingestion is the prerequisite for inactivation reasons in `R11`. Preserve unknown ComplexMap and ExtendedMap rows without guessing their target system; classify additional map systems only when a future release provides a known target.

- [ ] `R11` **Tell the complete inactive-concept story.** Using Snapshot concepts, Association history, and the inactivation-indicator AttributeValue refset, make lookup, MCP, and FHIR show inactive status, inactivation date/reason, and replacement targets with preferred terms; provide one coherent `history` view. True birth dates and years-in-service remain part of Full-RF2 temporal work (`R25`). See [`cross-terminology-mapping.md`](cross-terminology-mapping.md).

- [ ] `R12` **Finish set-to-ECL compression.** Build on the shipped exact greedy compressor with straddling-exclusion push-down, `^refset` cover clauses, and `sct codelist export --format ecl`; preserve re-expansion exactness tests and explicit residuals where no compact exact expression exists.

- [ ] `R13` **Design multi-terminology codelists (format v2).** Allow first-class non-SNOMED source codes where SNOMED is not an honest canonical pivot, while preserving the current format and `--include-maps` workflow for SNOMED-canonical lists. Treat migration, validation, and FHIR export semantics as design gates rather than merely adding a `system` column.

- [ ] `R14` **Improve SAYT ranking and query refinement.** Add clinically useful/frequency-aware ranking, word-prefix matching for partially typed multi-word queries, and hierarchy/semantic-tag filters; evaluate an explicit backend selector across FST, FTS5, and semantic search against a fixed query set.

- [ ] `R15` **Improve semantic-search result quality.** Benchmark per-synonym embeddings with max pooling, hybrid lexical/vector ranking, and clinically tuned models against the documented failure set (synonym dilution, hierarchy drift, and colloquial language) before selecting an implementation. See [`docs/commands/semantic.md`](../docs/commands/semantic.md#known-limitations) and [`spec/commands/embed.md`](commands/embed.md).

- [ ] `R16` **Complete the practical FHIR terminology surface.** Add `$expand` parameters (`activeOnly`, `displayLanguage`, designation/property filters, system/value-set versions), CodeSystem resource read, optional stored-ValueSet canonical URL override/draft filtering, then the useful FHIR R5 additions. Keep multi-version routing and national syndication explicitly out of scope until there is a concrete consumer.

## Browser SDK

The SDK programme has shipped native Rust and Python APIs over the same engine. A later stage brings a measured subset to browsers. Full architecture, acceptance criteria, data constraints, docs information architecture, and licensing rationale: [`sdk.md`](sdk.md).

- [ ] `R3` **Compile the Rust engine to WebAssembly and ship a local-only browser demo.** Make storage-independent logic `wasm32-unknown-unknown` clean, then select a browser query backend through a measured spike: preferably a compact `.sct-web` artefact derived locally from canonical NDJSON, compared against official SQLite WASM/OPFS. The docs-hosted static demo ships only the synthetic fixture; users generate/select their own licensed artefact, all content and queries remain in-browser, and no SNOMED data is hosted or uploaded. Initial useful scope: lookup, autocomplete, hierarchy/subsumption, and a documented ECL subset, validated against shared Rust/WASM fixtures with browser memory/load benchmarks.

## Foundation and interface consistency

This work should reuse shared engine contracts rather than reimplementing them.

- [ ] `R38` **Refresh `sct gui` as a clinical knowledge atlas.** Replace the experimental fixed dashboard with an offline, search-first, responsive terminology explorer whose stable knowledge graph, concept workspace, mappings/history, URL navigation, and accessibility are verified through a Playwright feedback loop. Follow the stable `GUI-1` through `GUI-8` delivery stages in [`gui.md`](gui.md); ship the polished search-to-concept vertical slice before expanding graph and specialist-query scope.

## Assurance, documentation, and evidence

- [ ] `R17` **Add externally verified FHIR conformance.** Run the HL7 FHIR Validator against real resources/Implementation Guides using `sct serve` as terminology backend, gate a synthetic-fixture subset in CI, and keep Touchstone/TestScript as a later complement. Continue to describe the home-grown suite as HL7-aligned, not certified.
- [ ] `R53` **Bound and cancel server-side ECL expansion work.** Cap or stream compound ECL and combined ECL/filter expansion so a remote client cannot force unbounded result materialisation, and replace uncancellable `spawn_blocking` work with an execution model that stops database work when the 30-second HTTP response timeout fires. This preserves the unfinished availability work identified as `R44` in the 2026-07-25 audit.

- [ ] `R18` **Write a concise SNOMED CT primer.** Explain concepts, descriptions, relationships, refsets, ECL, editions, and releases in plain language, with separate routes for technical and clinical readers and runnable examples using `sct`.

- [ ] `R19` **Finish the architecture diagrams.** Add an FST/search-internals diagram and worked diagrams over real SNOMED examples; retain literal text where it represents terminal/file layouts better than Mermaid.

- [~] `R20` **Complete and publish the benchmark suite.** Preserve the working Bash suite while the parity-gated typed-runner programme (`R48`-`R51`) lands, then broaden the committed FHIR conformance scenarios, add comparator compose profiles, compare SDK/CLI/FST/FTS/server boundaries honestly, and publish reproducible `sct`-solo reports under the reporting policy above. Architecture and evidence contract: [`benchmark-runner.md`](benchmark-runner.md).

- [ ] `R52` **Ship `sct bench`, a user-facing self-benchmark.** Add a public subcommand that times the SDK and CLI boundaries against the user's own database with no repository clone, container runtime, or external tooling, and renders a readable terminal report plus pasteable Markdown, standalone HTML, and canonical JSON. Emits the shared result schema so a user's numbers can be ingested by the comparative runner and quoted in a bug report. Deliverable before `R48`: it is smaller, independently useful, and needs no comparator. Surface and acceptance criteria: [`commands/bench.md`](commands/bench.md); boundary against the non-shipped runner: [`benchmark-runner.md`](benchmark-runner.md#relationship-to-sct-bench).

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

- [ ] `R26` **Compute proximal primitive supertypes.** Use definition status plus the TCT to expose the classification/normal-form primitive needed for subsumption and post-coordination QA, with typed SDK and CLI surfaces.

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

## Deferred distribution gates

The existing release routes already cover crates.io, installers, Homebrew tap, Scoop bucket, Docker Hub, binaries, checksums, deb/rpm, and unsigned DMGs. These remaining items depend on external accounts, certificates, adoption, or a preceding gate, so they sit at the end of the active plan.

- [ ] `R34` **Sign and notarise macOS releases.** Requires an Apple Developer ID and annual fee.
- [ ] `R35` **Add Windows Authenticode signing.** Requires an appropriate certificate; this unblocks a smoother SmartScreen experience and `R37`.
- [ ] `R36` **Submit to homebrew-core.** Wait for the project to meet its adoption/cadence expectations (currently around 30+ stars); the existing tap remains the supported route meanwhile.
- [ ] `R37` **Submit to winget.** Do this after Windows signing is operational.

## Exploratory ideas

Data-science adapters, notebooks, graph exports, editor/desktop integrations, clinical-data bridges, LLM-assisted authoring, visualisations, and `sct mud` live as [issues labelled `idea`](https://github.com/pacharanero/sct/issues?q=is%3Aissue+is%3Aopen+label%3Aidea), where the community can comment and add evidence. Promote an idea into this roadmap only when it has a likely delivery window and an owner.
