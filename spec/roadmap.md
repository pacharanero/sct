# SNOMED Local-First Tooling - Roadmap

Active work only, ordered by intended delivery. Completed work is removed; use the [changelog](../CHANGELOG.md), git history, and the [July 2026 audit record](audit-2026-07.md) for shipped work and evidence.

Legend: `[ ]` not started, `[~]` in progress. The `R##` sequence was deliberately reset in July 2026 to match this delivery order; these identifiers are stable from this point onward. Historical changelog entries, audit records, commit messages, and existing issue titles retain their pre-reset identifiers. Prose is soft-wrapped - one line per item, no hard wrapping.

## Principles and issue tracking

- The canonical NDJSON artefact remains the source for every derived format; SDK and browser artefacts must not bypass it for primary data.
- Public benchmark reporting is `sct`-solo plus the fully owned local Snowstorm Lite comparison. Commercial-server figures remain private; benchmark scripts never contain hosts or credentials.
- Bugs, bounded enhancements, and community discussion belong on the [GitHub issue tracker](https://github.com/pacharanero/sct/issues). Longer-horizon proposals live under the [`idea` label](https://github.com/pacharanero/sct/issues?q=is%3Aissue+is%3Aopen+label%3Aidea). RF2 codelist import/export remains decision-gated in [issue #60](https://github.com/pacharanero/sct/issues/60).

## Next - SDK and bindings

The next programme makes the existing `sct_rs` library a deliberate application API, then exposes the same Rust engine to Python and browsers. Full architecture, acceptance criteria, data constraints, docs information architecture, and licensing rationale: [`sdk.md`](sdk.md).

- [~] `R1` **Ship the Rust SDK facade and make existing surfaces consume it.** Implementation is complete locally: the typed `sct_rs::sdk::Snomed` facade covers read-only open, provenance/schema compatibility, concept lookup/search, ECL, hierarchy/subsumption, refsets, mappings/history, release-validated optional FST search, and offline typed codelist composition. CLI lookup/lexical/refset/map, MCP concept/search/hierarchy/refset/map, and FHIR lookup/subsumption consume shared query primitives. The default `cli` feature now isolates command/build/export dependencies, while `default-features = false` retains only the native query stack; `tests/downstream-sdk/` proves the downstream API shape. Mark complete after the next crates.io release and a version-dependency smoke test.

- [ ] `R2` **Publish Python bindings via PyO3/maturin.** Follow `clincalc`'s separate `python/` `cdylib` pattern with ABI3 CPython 3.9+ wheels: a typed/context-managed `Snomed` class exposing lookup, batch lookup, search, ECL, hierarchy/subsumption, refsets, mappings, and provenance over a user-supplied `snomed.db`; release the GIL for long queries, provide type hints and specific exceptions, test wheels against the synthetic fixture, and integrate PyPI publication into the release cascade. Check PyPI/import naming immediately before implementation, ship no terminology content, retain AGPL-3.0-or-later, and add the Python page under **SDK**.

- [ ] `R3` **Compile the Rust engine to WebAssembly and ship a local-only browser demo.** Make storage-independent logic `wasm32-unknown-unknown` clean, then select a browser query backend through a measured spike: preferably a compact `.sct-web` artefact derived locally from canonical NDJSON, compared against official SQLite WASM/OPFS. The docs-hosted static demo ships only the synthetic fixture; users generate/select their own licensed artefact, all content and queries remain in-browser, and no SNOMED data is hosted or uploaded. Initial useful scope: lookup, autocomplete, hierarchy/subsumption, and a documented ECL subset, validated against shared Rust/WASM fixtures with browser memory/load benchmarks.

## Foundation and interface consistency

Do these alongside or immediately after the SDK extraction: they define contracts every adapter and binding should inherit rather than reimplement.

- [ ] `R4` **Make stdout and exit status reliable.** Single-lookups that miss write the hint to stderr and exit 1; empty searches may exit 0 but keep stdout machine-clean; usage remains exit 2. Apply consistently to lookup, refset, lexical, FST, and codelist commands, document the convention, and cover it with CLI contract tests. This is a behaviour change and must be called out in the changelog.

- [ ] `R5` **Unify structured output formats.** Give `sct info` the shared `--format text|json|yaml`; align `map` and `diff` with the common vocabulary while retaining domain formats such as TSV/CSV/Markdown as additive variants; publish a removal schedule for hidden `--json` aliases.

- [ ] `R7` **Make stdin (`-`) composable across read commands.** Add batch stdin paths to the natural single-value readers (`lookup`, `lexical`, `semantic`, and relevant refset operations), with deterministic line-oriented or structured output suitable for pipelines.

- [ ] `R8` **Unify missing-TCT guidance.** Route recursive-CTE fallbacks through one helper so CLI callers receive the same stderr instruction and MCP callers receive an equivalent log/diagnostic notification; retain `sct size`'s explicit interactive build flow.

- [ ] `R9` **Bring MCP mapping/history to parity.** Extend `snomed_map` beyond CTV3/Read v2 to ICD-10 and OPCS-4 crossmaps, add history-forwarding/resolve results, and ensure the MCP adapter consumes the same typed SDK methods as CLI and FHIR.

## Terminology capability

- [ ] `R10` **Parse the remaining RF2 refset families.** Add Complex refsets and AttributeValue refsets without overloading the concept-only `refset_members` table; preserve payload fields and provenance. AttributeValue ingestion is the prerequisite for inactivation reasons in `R11`. Additional ExtendedMap systems should be classified only when a future release provides a known target.

- [ ] `R11` **Tell the complete inactive-concept story.** Using Snapshot concepts, Association history, and the inactivation-indicator AttributeValue refset, make lookup, MCP, and FHIR show inactive status, inactivation date/reason, and replacement targets with preferred terms; provide one coherent `history` view. True birth dates and years-in-service remain part of Full-RF2 temporal work (`R25`). See [`cross-terminology-mapping.md`](cross-terminology-mapping.md).

- [ ] `R12` **Finish set-to-ECL compression.** Build on the shipped exact greedy compressor with straddling-exclusion push-down, `^refset` cover clauses, and `sct codelist export --format ecl`; preserve re-expansion exactness tests and explicit residuals where no compact exact expression exists.

- [ ] `R13` **Design multi-terminology codelists (format v2).** Allow first-class non-SNOMED source codes where SNOMED is not an honest canonical pivot, while preserving the current format and `--include-maps` workflow for SNOMED-canonical lists. Treat migration, validation, and FHIR export semantics as design gates rather than merely adding a `system` column.

- [ ] `R14` **Improve SAYT ranking and query refinement.** Add clinically useful/frequency-aware ranking, word-prefix matching for partially typed multi-word queries, and hierarchy/semantic-tag filters; evaluate an explicit backend selector across FST, FTS5, and semantic search against a fixed query set.

- [ ] `R15` **Improve semantic-search result quality.** Benchmark per-synonym embeddings with max pooling, hybrid lexical/vector ranking, and clinically tuned models against the documented failure set (synonym dilution, hierarchy drift, and colloquial language) before selecting an implementation. See [`docs/commands/semantic.md`](../docs/commands/semantic.md#known-limitations) and [`spec/commands/embed.md`](commands/embed.md).

- [ ] `R16` **Complete the practical FHIR terminology surface.** Add `$expand` parameters (`activeOnly`, `displayLanguage`, designation/property filters, system/value-set versions), CodeSystem resource read, optional stored-ValueSet canonical URL override/draft filtering, then the useful FHIR R5 additions. Keep multi-version routing and national syndication explicitly out of scope until there is a concrete consumer.

## Assurance, documentation, and evidence

- [ ] `R17` **Add externally verified FHIR conformance.** Run the HL7 FHIR Validator against real resources/Implementation Guides using `sct serve` as terminology backend, gate a synthetic-fixture subset in CI, and keep Touchstone/TestScript as a later complement. Continue to describe the home-grown suite as HL7-aligned, not certified.

- [ ] `R18` **Write a concise SNOMED CT primer.** Explain concepts, descriptions, relationships, refsets, ECL, editions, and releases in plain language, with separate routes for technical and clinical readers and runnable examples using `sct`.

- [ ] `R19` **Finish the architecture diagrams.** Add an FST/search-internals diagram and worked diagrams over real SNOMED examples; retain literal text where it represents terminal/file layouts better than Mermaid.

- [~] `R20` **Complete and publish the benchmark suite.** Broaden the committed FHIR conformance fixtures, add comparator compose profiles, compare `lexical`/FST/index configurations, and publish reproducible `sct`-solo reports under the reporting policy above.

- [ ] `R21` **Publish separated `sct serve` scaling curves.** Run `benchmarks/load.sh` from a separate client machine, sweep `--pool-size`, and publish throughput/latency versus concurrency so client compute no longer distorts server capacity.

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
