# OMOP/OHDSI terminology adapters

**Status:** Exploratory design. No implementation is committed yet.
**Scope:** Make `sct` useful to OMOP/OHDSI researchers without turning it into an OMOP Common Data Model engine, changing its RF2-first terminology model, or ingesting patient-level data.
**Audience:** Marcus, contributors, OMOP implementers, phenotype authors, and coding agents evaluating or implementing bounded adapter slices.

---

## 1. Why this fits `sct`

The Observational Medical Outcomes Partnership Common Data Model (OMOP CDM), maintained by the Observational Health Data Sciences and Informatics (OHDSI) community, standardises the structure and content of observational patient data so common analytics can run across EHR, claims, registry, and other databases. Its vocabulary layer is central to that model: source codes are mapped to standard concepts, and cohort phenotypes are constructed from reusable concept sets.

SNOMED CT is an important OMOP standard vocabulary, particularly for conditions, observations, and procedures. OMOP users therefore perform work that overlaps strongly with `sct`'s strengths:

- resolving and reviewing SNOMED CT concepts;
- walking hierarchy and subsumption relationships;
- constructing and maintaining concept sets;
- identifying inactive concepts and their replacements;
- comparing terminology content across releases;
- preserving provenance for reproducible research;
- mapping local or source vocabulary codes to standard concepts.

The systems address different layers. OMOP models observational patient data and standard analytics. `sct` manages and queries locally supplied terminology releases. The useful positioning is:

> OMOP handles observational patient data. `sct` handles the SNOMED CT terminology work around it.

Implementing a small, honest adapter is also a practical way to learn OMOP's real conventions and workflows. The first implementation should deliberately expose the points where OMOP and SNOMED semantics differ rather than papering over them.

---

## 2. Non-goals and invariants

OMOP support must preserve the existing architecture:

- **Canonical NDJSON remains canonical for SNOMED CT primary data.** Athena input is an adapter/index for OMOP identities and relationships, not an alternative SNOMED ingestion path.
- **Do not become an OMOP CDM database engine.** `sct` does not create or query patient-level tables such as `PERSON`, `CONDITION_OCCURRENCE`, `DRUG_EXPOSURE`, or `MEASUREMENT`.
- **Do not ingest patient-level data.** Initial and foreseeable OMOP features operate only on vocabulary distributions, concept-set definitions, and terminology identifiers.
- **Do not replace RF2 semantics with OMOP vocabulary semantics.** RF2 remains authoritative for SNOMED descriptions, relationships, refsets, modules, lifecycle, and historical associations.
- **Do not invent OMOP identifiers.** OMOP `concept_id` values come only from a named Athena vocabulary distribution. They are not SCTIDs and cannot be derived from them.
- **Do not treat OHDSI `Maps to` relationships as RF2 map refsets.** Preserve their distinct source, provenance, validity, and semantics.
- **Do not redistribute licensed vocabulary content.** Users provide their own RF2 and Athena downloads; committed tests use a synthetic, licence-free fixture.
- **Read operations remain read-only.** Any local adapter store is derived and regenerable. It must not cause existing terminology reads to open the main SNOMED database read-write.
- **Unsupported conversion fails explicitly.** A construct that cannot be represented without changing its meaning must be refused or materialised only after an explicit user choice and warning.

---

## 3. Ubiquitous language and identity boundaries

The following distinctions must appear consistently in code, help, docs, output schemas, and tests.

| Term | Meaning |
|---|---|
| **OMOP CDM** | The common schema and conventions for observational patient data and its vocabulary tables. |
| **OHDSI standardised vocabularies** | The multi-vocabulary distribution used by OMOP, normally obtained from Athena. |
| **Athena distribution** | A user-downloaded bundle containing OMOP vocabulary tables such as `CONCEPT`, `CONCEPT_RELATIONSHIP`, `CONCEPT_ANCESTOR`, and `VOCABULARY`. |
| **OMOP concept ID** | The integer `concept_id` assigned by OHDSI to one row in `CONCEPT`. It is internal to the OMOP vocabulary ecosystem and is not an SCTID. |
| **Vocabulary concept code** | `CONCEPT.concept_code`, interpreted in the namespace identified by `vocabulary_id`. For a SNOMED concept this is normally the SCTID string. |
| **Standard concept** | An OMOP concept whose `standard_concept` flag marks it for standard representation. This is an OMOP designation, not a statement that SNOMED regards the concept as clinically preferred or active. |
| **Source concept** | The vocabulary concept found in source data before mapping to an OMOP standard concept. It may itself use SNOMED CT or another vocabulary. |
| **OMOP relationship** | A row in `CONCEPT_RELATIONSHIP`, including relationships such as `Maps to`. It is an OHDSI vocabulary assertion, not an RF2 relationship or map member. |
| **OMOP concept set** | A set definition used by OHDSI tools such as ATLAS, potentially carrying include/exclude and descendant intent. It is not automatically equivalent to ECL or `.codelist`. |
| **`sct` codelist** | A reviewable, version-controlled local artefact whose effective members may be composed from other codelists. |
| **Vocabulary release identity** | The version/provenance of the Athena distribution, recorded separately from the SNOMED edition and release used by `sct`. |

Never expose an unqualified field named only `id` when both OMOP and SNOMED identities appear. Prefer `omop_concept_id`, `sctid`, `concept_code`, and `vocabulary_id`.

---

## 4. Target users and workflows

### 4.1 Phenotype author

An R or ATLAS user imports an existing OMOP concept set, inspects its SNOMED hierarchy and lifecycle against a specific local release, records review decisions in a `.codelist`, and exports an ATLAS-compatible definition for use by the research team.

### 4.2 OMOP ETL implementer

An implementer resolves a batch of OMOP `concept_id` values to vocabulary/system codes, distinguishes source from standard concepts, and audits SNOMED mappings without loading patient records into `sct`.

### 4.3 Cohort governance reviewer

A reviewer compares a phenotype's SNOMED content with a newer release, identifies retired codes and historical-association targets, checks hierarchy coverage, and produces a deterministic report carrying both Athena and SNOMED provenance.

### 4.4 Data scientist

An R or Python user reads tabular adapter output into `dplyr`, DuckDB, Polars, or pandas while continuing to use the existing CLI/SDK for terminology operations.

---

## 5. Adapter architecture

### 5.1 Inputs

The adapter may consume a user-supplied Athena vocabulary directory or archive containing a declared subset of standard OMOP vocabulary tables:

- `CONCEPT` - OMOP identity, vocabulary, code, name, domain, class, standard status, validity dates, invalid reason;
- `VOCABULARY` - vocabulary identity and version metadata;
- `CONCEPT_RELATIONSHIP` - mappings and other inter-concept relationships;
- `RELATIONSHIP` - relationship metadata and reverse relationships;
- `CONCEPT_ANCESTOR` - optional OMOP-wide ancestry for non-SNOMED or OMOP-native analysis;
- `DOMAIN` and `CONCEPT_CLASS` - optional labels and validation.

The first slice should require only the tables it actually uses. Missing optional tables must disable specific capabilities explicitly rather than producing partial answers silently.

### 5.2 Local storage

Do not merge OMOP rows into the SNOMED `concepts` table. Candidate storage shapes to evaluate:

1. a separate SQLite adapter database beside the `sct` database;
2. a set of attached read-only SQLite tables built from Athena CSVs;
3. Parquet output for analysis plus a small indexed SQLite identity resolver.

The preferred initial shape is a separate, regenerable SQLite adapter database. It avoids schema collision, lets Athena and SNOMED releases vary independently, and allows both databases to be attached read-only for joined queries. Its provenance must include source filenames/fingerprints, Athena vocabulary versions, builder version, schema version, and build time.

### 5.3 Join contract

SNOMED linkage is valid only when all of the following hold:

- `CONCEPT.vocabulary_id` identifies SNOMED CT under the imported Athena distribution's conventions;
- `CONCEPT.concept_code` is a syntactically valid SCTID;
- the SCTID is resolved against the explicitly selected local `sct` database;
- both the Athena and SNOMED provenance are returned with results.

An absent SCTID in the local edition is a reportable mismatch, not proof that either side is wrong. National editions, release timing, inactive-content choices, and Athena refresh cadence can all produce legitimate differences.

---

## 6. Candidate command surface

If implementation proceeds, prefer one `sct omop` family rather than unrelated top-level commands:

```text
sct omop import-vocabulary
sct omop resolve
sct omop concept-set import
sct omop concept-set export
sct omop concept-set audit
```

Names are provisional until real Athena and ATLAS workflows have been exercised.

### 6.1 `sct omop import-vocabulary`

Build the regenerable adapter store from a user-supplied Athena distribution. Validate required headers, integer/code fields, duplicate keys, relationship endpoints, vocabulary metadata, and source fingerprints before publishing output atomically.

### 6.2 `sct omop resolve`

Resolve either direction without ambiguity:

- OMOP `concept_id` to `vocabulary_id`, `concept_code`, and, when SNOMED, SCTID plus local concept details;
- `vocabulary_id` + `concept_code` to matching OMOP concept rows;
- SCTID to SNOMED OMOP concept rows in the selected Athena release.

Batch stdin and structured output should follow existing read-command contracts. Never accept a bare number without a flag or input schema that disambiguates OMOP ID from SCTID.

### 6.3 `sct omop concept-set import`

Read an ATLAS/OHDSI concept-set definition and create a draft `.codelist` plus retained OMOP metadata. Import must preserve, or explicitly report inability to preserve:

- inclusion versus exclusion;
- descendant inclusion;
- mapped-concept inclusion;
- source versus standard concept identity;
- concept-set name and provenance.

An OMOP concept-set expression with dynamic descendant/mapped intent must not silently become a timeless flat list. Viable representations to evaluate are:

- `.codelist` members plus structured methodology/warnings recording materialisation;
- `.codelist` with a companion adapter definition retaining OMOP intent;
- direct conversion to ECL only for the subset whose semantics are genuinely equivalent.

### 6.4 `sct omop concept-set export`

Export a `.codelist` or ECL-defined set to ATLAS-compatible JSON. Literal members map through the selected Athena vocabulary. Descendant and exclusion semantics should be retained where the target format supports them. If an exact semantic round-trip is impossible, refuse by default; an explicit materialisation mode may export the effective member set with both release identities and a warning that dynamic intent was flattened.

### 6.5 `sct omop concept-set audit`

Produce a deterministic governance report covering:

- unresolved OMOP IDs or SCTIDs;
- inactive SNOMED concepts and historical associations;
- OMOP invalid reasons and validity windows;
- OMOP standard/source concept status;
- `Maps to` targets and multiplicity;
- differences between the selected Athena vocabulary and SNOMED release;
- hierarchy coverage and optional new descendants since a previous SNOMED release;
- preferred-term/name drift;
- exact source fingerprints and versions.

The audit consumes vocabulary and concept-set data only, never patient rows.

---

## 7. R and Python affordances

OMOP/OHDSI work is strongly R- and SQL-centred, with substantial Python use. Initial support should meet those communities where they already work rather than immediately committing to another native binding.

### 7.1 R first-class CLI interoperability

Provide documented examples using:

- `processx` for reliable CLI invocation and exit-status handling;
- `jsonlite` for structured JSON output;
- `DBI`/`RSQLite` for read-only access to adapter SQLite;
- `duckdb` and `arrow` for Parquet analysis;
- `dplyr` for concept-set review and reporting.

Add a public walkthrough that imports one synthetic ATLAS concept set, resolves it against a synthetic Athena fixture and the existing synthetic RF2 fixture, audits lifecycle/hierarchy, and exports it again.

Only build an R package after real use demonstrates that process invocation and tabular artefacts are inadequate. A later package could be either a thin CLI wrapper or native bindings to the Rust SDK, but must not duplicate terminology logic in R.

### 7.2 Python

The existing `sct-py` package is the natural native surface. After the adapter contract stabilises, it may expose typed OMOP identity resolution and concept-set conversion. Python and R outputs must share the same engine types and golden fixtures.

### 7.3 SQL and files remain products

CSV, JSON, Parquet, and read-only SQLite are not fallback interfaces; they are primary interoperability surfaces for both communities. Output schemas need explicit versioning and identity names.

---

## 8. Phased learning plan

### Phase 0 - domain spike

Before production code:

1. obtain a legitimately licensed Athena distribution;
2. document its actual file names, schemas, encoding, vocabulary metadata, and refresh identity;
3. export several representative ATLAS concept sets containing descendants, exclusions, mapped concepts, source concepts, and invalid concepts;
4. reproduce the equivalent workflow in R/ATLAS to understand user expectations;
5. write down semantic mismatches and licensing constraints.

### Phase 1 - identity resolver

Build a synthetic Athena fixture and implement the smallest end-to-end adapter:

1. import the necessary `CONCEPT` and `VOCABULARY` columns into a separate derived store;
2. resolve OMOP `concept_id` to/from (`vocabulary_id`, `concept_code`);
3. enrich SNOMED rows through the existing read-only SDK/database;
4. return dual provenance;
5. publish CLI, JSON schema, R example, and tests.

This is the smallest credible implementation and establishes the identity boundary every later feature needs.

### Phase 2 - one ATLAS concept-set round-trip

Import and export one well-understood concept-set subset while preserving include/exclude and descendant semantics. Add golden JSON tests and an explicit unsupported-construct inventory.

### Phase 3 - audit

Combine OMOP validity/mapping information with `sct` lifecycle, hierarchy, and release comparison to produce a governance report.

### Phase 4 - broader integrations

Only after adoption evidence: `sct-py` methods, a thin R package, ATLAS workflow documentation, OMOP-ready codelist export, MCP tools, or FHIR/OMOP bridges.

---

## 9. Testing and assurance

### 9.1 Synthetic fixtures

Commit a minimal licence-free OMOP vocabulary fixture containing:

- SNOMED standard and non-standard/source concepts;
- at least one non-SNOMED source vocabulary;
- active and invalid concepts with validity dates;
- one-to-one, one-to-many, and absent `Maps to` relationships;
- concept IDs deliberately numerically similar to SCTIDs to prove type/flag disambiguation;
- a small `CONCEPT_ANCESTOR` graph whose OMOP ancestry differs visibly from the RF2 hierarchy;
- representative ATLAS concept-set JSON with includes, exclusions, descendants, and mapped concepts.

Cross-test SNOMED resolution against `tests/fixtures/rf2/` through the real `sct sqlite` schema. Agent-generated fixtures are not sufficient evidence by themselves: compare field meanings and representative exports against official OMOP/OHDSI documentation and a real licensed Athena distribution during the domain spike.

### 9.2 Core invariants

- OMOP IDs and SCTIDs never share an untyped API parameter or output field.
- Import/export golden tests prove no unsupported intent is silently dropped.
- Every output carries Athena and SNOMED provenance independently.
- Missing source tables/capabilities fail before output is written.
- SQL uses bound parameters for all user values.
- Adapter reads open both databases read-only.
- No test or feature requires patient-level data.
- No licensed Athena or SNOMED content enters the repository.

### 9.3 Round-trip property

For the declared supported ATLAS subset:

```text
normalise(export(import(definition))) == normalise(definition)
```

Where `.codelist` is involved, separately assert that the effective member set is exact against both the chosen Athena release and SNOMED edition. Semantic intent and materialised membership are two distinct assertions.

---

## 10. Licensing and provenance

- Athena distributions combine vocabularies with different licences. Record and surface the vocabulary metadata supplied by the distribution; do not imply that the bundle has one blanket licence.
- SNOMED CT remains subject to SNOMED International and national release licensing. OMOP packaging does not remove those obligations.
- Do not publish Athena-derived SNOMED rows, test fixtures copied from Athena, or prebuilt adapter databases unless redistribution rights are established explicitly.
- Record both the Athena vocabulary version and local SNOMED edition/release. One must never stand in for the other.
- Reports intended for sharing should support provenance redaction where release identity is considered sensitive, following `sct bench --no-provenance`, while retaining enough schema/version information to interpret the output.

---

## 11. Open design questions

1. What exact ATLAS concept-set JSON version(s) and normalisation rules should be supported?
2. Should dynamic OMOP intent live directly in `.codelist` format v2, or in an OMOP companion definition? This intersects `R13` multi-terminology codelist design and must not pre-empt it accidentally.
3. How should `includeMapped` compose with OHDSI `Maps to` multiplicity and invalid targets?
4. Does a separate SQLite adapter store provide enough performance and R ergonomics, or is Parquet plus a resolver index preferable?
5. Which Athena metadata reliably identifies the complete vocabulary release and per-vocabulary versions?
6. Should OMOP `CONCEPT_ANCESTOR` ever be queryable directly, or only used for audit/comparison so RF2 remains authoritative for SNOMED hierarchy?
7. What should an OMOP-ready codelist export contain when no Athena concept row exists for a valid national-extension SCTID?
8. Is an R package justified by real users, and if so should it wrap the CLI or bind the Rust SDK?
9. Which output and concept-set formats are already used by `CohortGenerator`, ATLAS, and other OHDSI tools, and which are internal/unstable?

---

## 12. References

- OHDSI, [OMOP Common Data Model](https://ohdsi.github.io/CommonDataModel/).
- OHDSI, [Data Standardization](https://www.ohdsi.org/data-standardization/).
- OHDSI, [The Book of OHDSI](https://ohdsi.github.io/TheBookOfOhdsi/).
- OHDSI Athena, [Standardized Vocabularies](https://athena.ohdsi.org/).
- [`cross-terminology-mapping.md`](cross-terminology-mapping.md) - existing source/pivot/target mapping model and provenance rules.
- [`commands/ecl-compress.md`](commands/ecl-compress.md) - exact extensional/intensional conversion and round-trip principles.
- [`roadmap.md`](roadmap.md) `R13` - multi-terminology codelist design gate.
- [`../docs/commands/history.md`](../docs/commands/history.md) - inactive SNOMED concept audit surface.
- [`../docs/sdk/python.md`](../docs/sdk/python.md) - existing Python binding surface.
