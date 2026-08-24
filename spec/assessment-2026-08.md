# Where `sct` stands - August 2026 assessment

A point-in-time stock-take: what the project set out to do, where it has gone well beyond that, where the real gaps now are, and what that implies for priorities. Companion to the [July 2026 audit record](audit-2026-07.md) (which evidences shipped work) and the [roadmap](roadmap.md) (which this assessment reprioritises).

## Where we are

The original aim - turn an RF2 release into fast, local, queryable data with no Java and no Elasticsearch - is not just done, it is hardened: NDJSON is the canonical artefact, with SQLite/Parquet/Markdown/FST/embeddings derived from it, all offline, all reproducible from a committed synthetic fixture. If that were the whole project, you would call it finished. Almost everything else is beyond the original scope.

## Where we have overdelivered

- **Surface breadth.** The same engine is reachable through the CLI, a FHIR R4 server, an MCP server, a Rust + Python SDK, a TUI and a GUI, and Parquet/DuckDB - 7+ front doors on a tool that set out to be a converter. Most SNOMED tools have one.
- **FHIR conformance rigour.** The `R17` invariant - unrecognised or unsupported input must never silently degrade to a broader default - with an asserted disposition for every input parameter of every operation, is more honest than a lot of production terminology servers. This is self-auditing at a level the original aim never implied.
- **LLM-native access.** The MCP server is ahead of the curve and was on no one's original list.
- **Semantic search** with model-aware embeddings (Nomic / Qwen3 / EmbeddingGemma) is research-grade for a conversion tool.
- **Distribution.** crates.io, Homebrew, Scoop, AUR, deb/rpm, dmg, exe, Docker, and checksummed installers - packaging maturity far beyond a project of this size.
- **The assurance machine itself** - the synthetic fixture, spec-derived conformance, and the autonomous nightly-bot QA loop - is meta-overdelivery.

## The real gaps

These cluster around one theme: `sct` has gone **wide on surfaces**, but the frontier now is **depth in SNOMED's own semantics and in search**.

1. **The description-logic layer is missing** - SCG post-coordination, OWL axioms, MRCM (`R28`/`R33`). `sct` shows relationship rows but not the actual logical *definition* of a concept, and cannot represent post-coordinated expressions. For authoring, QA, and genuine understanding, this is the biggest terminology-depth gap.
2. **Search quality is the core function that is least finished** (`R14`/`R15`). Ranking is not yet clinically or frequency-aware; semantic quality is still being tuned. Ironic, because finding the right code fast is the thing users do most - and everything else (MCP, FHIR, GUI) rides on it.
3. **Terminology breadth beyond SNOMED.** No LOINC at all (the most glaring absence for a clinical toolkit), and no ICD-10/11 yet (`R22`). Real interoperability and reimbursement work needs these.
4. **dm+d medicines depth** (`R23`). dm+d ships inside the UK Monolith but `sct` does not yet build the barcode -> AMPP -> AMP -> VMP -> VTM -> BNF/ATC graph - exactly what practical UK users reach for.
5. **Through-time reasoning** (`R25`). There is a two-release diff and association forwarding, but not full point-in-time reconstruction - what the terminology looked like at date X, how ancestry and membership evolved. Longitudinal data and migration need this.
6. **Multi-terminology codelists** (`R13`). Codelists are SNOMED-canonical; real-world codelists mix systems. This limits the "codelist as a durable clinical artefact" story.
7. **Evidence and reach.** The performance claims are not yet backed by a published, reproducible benchmark suite (`R20`/`R48`-`R51`), and there is no browser/WASM way to try it without installing (`R3`). Both are adoption gaps more than capability gaps.

## Deliberately out of scope (boundaries, not gaps)

Single-edition serving, no auth/SMART, and no patient-data handling are conscious boundaries. They do cap how far `sct serve` can be a shared production server - which is fine as long as that remains a stated non-goal.

## Synthesis

`sct` is a **terminology-access** tool of unusual breadth and unusual honesty. The next frontier is becoming a **terminology-reasoning** tool: the description-logic layer, search quality, and a second code system (LOINC or dm+d) would each add more real depth than another output surface. The single highest-leverage gap is **search quality**, because it is the core promise everything else depends on.

## Prioritisation (what this changes on the roadmap)

Human-led delivery order is reset accordingly (the roadmap keeps its stable `R##` grouping; this is the delivery order across those groups):

1. **Search quality** - `R14`, `R15`.
2. **Description-logic layer** - `R28` (SCG/OWL), then `R33` (MRCM).
3. **A second code system** - `R23` (dm+d, closest to real demand) and/or `R22` (ICD-10/11); LOINC is the most glaring absent system.
4. **Multi-terminology codelists** - `R13`, unblocked by the above.
5. **Evidence and reach** - `R20`/`R48`-`R51` (published benchmarks) and `R3` (browser/WASM).

The autonomous nightly queue continues in parallel on self-contained assurance work, which does not compete for the design attention the above require.
