# Refset Discovery and Evidence

**Status:** Proposed, human-led stages `R84`-`R88` in the [roadmap](roadmap.md#terminology-capability). This specifies additional capability, not shipped behaviour. New option names, API signatures and command syntax remain provisional until each stage's design gate is resolved.

## Existing Surface

[`sct refset`](../docs/commands/refset.md) already provides `list`, `info`, `members` with limits, pairwise `compare`, and hierarchy `profile`. The CLI delegates through [`Snomed`](../src/sdk/mod.rs) to [`src/refset.rs`](../src/refset.rs); MCP and other adapters should reuse that engine. Extend discovery in `list`/`info`, not with redundant catalogue, search or preview commands. Dataset coverage is a different operation from hierarchy profiling or comparing two refsets.

[`src/rf2.rs`](../src/rf2.rs) currently projects active simple memberships for included concepts. [`ConceptRecord`](../src/schema.rs) carries refset IDs, and [`refset_members`](../src/commands/sqlite.rs) stores only `(refset_id, referenced_component_id)`. TextDefinition records are not exposed. Lossless payload-refset companion streams already exist; reuse their provenance approach without mixing payload rows into concept membership (see [cross-terminology mapping](cross-terminology-mapping.md)).

## Meaning and Boundaries

- Publisher TextDefinition is source evidence, not an inferred clinical algorithm or a logical OWL definition. Preserve it separately from optional human-authored purpose/role annotations; annotations must carry author/source and review status, and name-based suggestions must be labelled provisional.
- A TextDefinition `effectiveTime` dates that description version, not the refset's introduction. A concept's date and lifecycle are separate again. Earliest observed evidence is not proof of first publication.
- Terminology lifecycle, membership lifecycle, clinical status values and assertion polarity are distinct. A terminology-active member can represent a negative assertion or an inactive clinical problem. Do not infer polarity from an `active` flag or classify a whole value vocabulary as a positive disease cohort.
- Counts describe loaded membership and its import policy, not a complete RF2 inventory. Zero loaded rows can mean filtered or unimported members; an absent refset concept is distinct from a present concept with no loaded members. Even a present concept with zero members is not proof that it is a refset.
- FHIR `ValueSet.purpose` is not automatically equivalent to RF2 TextDefinition. Any later FHIR mapping requires an explicit, specification-checked semantic decision and conformance tests; do not populate it merely because definition text is available.
- Public examples and committed fixtures use synthetic definitions and observations only. No licensed definition text, private dataset details or patient identifiers belong in these documents or test outputs. Clinical predicates and external medicine-risk evidence remain human/domain responsibilities.

## R84: Canonical TextDefinition

Ingest TextDefinition Snapshot records through RF2 -> canonical NDJSON before any SQLite or UI work. Retain all RF2 fields: description `id`, `effectiveTime`, `active`, `moduleId`, `conceptId`, `languageCode`, `typeId`, `term` and `caseSignificanceId`. Preserve multiple definitions, languages and publisher spelling verbatim, including inactive description records; active-only presentation is a query projection, not destructive ingest. Do not silently lose definitions whose concept was filtered out of the concept stream.

**Design gate:** choose a versioned embedded representation or provenance-bound companion stream, document source selection and deterministic layering by description identity, and specify retention independently of concept/refset filters. Snapshot loading must not accidentally read Full or Delta files. This stage does not implement Full history.

**Acceptance:** deterministic canonical round trips retain the complete selected Snapshot evidence, including inactive replacements in layered inputs. Capability/provenance distinguishes metadata unavailable (old artefact, omitted input or unsupported build) from metadata loaded with zero matching definitions; where known, report why it is unavailable. An empty array alone must not erase that distinction. Record the input release identity and retention policy. Existing canonical consumers must either handle the new schema as documented or reject unsupported versions explicitly.

## R85: SQLite and Shared Retrieval

Depends on `R84`. Derive indexed TextDefinition storage and its capability metadata solely from canonical NDJSON. Expose typed concept-definition retrieval through the shared Rust SDK, usable by lookup and refset queries without reopening RF2 archives. Return definition identity, full source fields and release provenance, keeping concept status/date separately labelled. Default display selects active definitions, with explicit access to retained inactive records and language selection; never silently replace multiple definitions with one guessed purpose.

**Design gate:** settle the SQLite schema, typed result and selection contract, and behaviour for existing persisted databases. Older builds must report unavailable metadata with rebuild guidance, not claim that the publisher supplied no definition. A requested metadata-dependent query must not silently become name-only discovery.

**Acceptance:** real SQLite builds reproduce canonical values and capability states; readers use the shared read-only opener and bound SQL, and a missing DB is never created. Wire lookup/refset info to the shared retrieval and verify Rust/Python parity. MCP and GUI consumers must reuse that result contract when exposed, with output-schema/UI boundary tests rather than separate RF2 readers. Full MCP/GUI presentation can be separately scoped after this retrieval slice; FHIR mapping remains gated above.

## R86: Purpose-Aware Discovery

Depends on `R85`. Extend existing `refset list` with explicit name/definition search and useful module/lifecycle filters, and `refset info` with joined definition evidence, capability state, concept lifecycle, loaded member counts and a bounded member preview. Keep source text and reviewed/provisional role annotations separate. Reuse member queries: a tiny set should show every loaded member; a larger preview must state total, returned count and truncation and point to existing `members` for exploration.

**Design gate:** agree search semantics, language/active-definition selection, preview threshold and opt-in/default presentation without breaking existing structured or stdin-batch contracts. Names for new flags remain provisional. Do not expand `list` into a purported complete inventory: empty/unimported refset inventory needs independent evidence and is outside this stage.

**Acceptance:** a definition-only match is discoverable, missing metadata is explicit, counts do not change with preview limits, ordering has a stable ID tie-breaker, and both positive and negative synthetic assertion values remain visible with their independent lifecycle flags. Existing `members`, `compare`, hierarchy `profile`, and batch behaviour remain intact. Discovery is evidence for review, not a claim of phenotype equivalence or clinical validity.

## R87: Local Dataset Coverage

Independently deliverable against existing loaded membership; does not depend on TextDefinition. Add an explicit typed local helper/API for caller-provided code sets or streams and caller-selected fields, separate from hierarchy `refset profile`. Start with exact recorded-code membership. Require a matching mode and terminology release identity; descendant expansion and association/history matching must be separately selected, with unsupported modes refused rather than approximated. Historical coverage additionally depends on `R25` and `R88`.

Return per-field and combined distinct-code denominators, matched counts and unmatched codes, distinguishing codes absent from the terminology build from known nonmembers. Optional encounter coverage requires an explicit nonblank encounter key and a documented duplicate-key policy. Count unique encounters independently of code counts: repeated columns, multiple matching codes and cross-field overlap count once in the combined encounter union, never as a sum. Report zero denominators explicitly rather than implying a meaningful percentage.

**Design gate:** settle the minimal input/result types, finite input/aggregation limits and duplicate-key policy. Prefer caller-side encounter aggregation over adding a patient-file importer. Any CLI adapter is a later human decision; no new command syntax is committed here. Patient rows and encounter keys never enter the terminology DB or canonical artefacts; no encounter identifiers are emitted or logged. Coverage is descriptive overlap, not sensitivity, specificity or truth labels.

**Acceptance:** independent row-first synthetic counting agrees with helper results for repeated fields, multiple matches, cross-field overlap, unknown codes, empty inputs and invalid/duplicate keys. Matching mode, build filters and release provenance accompany results. The terminology DB remains unchanged and no patient-level storage is created.

## R88: Member Evidence for R25

This is an evidence-preservation dependency/extension of [existing `R25`](roadmap.md#larger-product-capabilities), not a second temporal implementation. Preserve simple-member UUID, effective time, active status, module, refset and referenced component through canonical artefacts into derived evidence storage, separate from the existing concept-membership projection. Snapshot evidence retention can ship first; versioned Full member evidence must follow the common temporal model selected by `R25`.

**Design gate:** coordinate identity/version keys and module-version dependency resolution with `R25`. Reuse its reconstruction, two-release diff and association infrastructure; do not offer an independent global-date-cutoff algorithm. As-at membership and historical coverage require validated edition reconstruction and must refuse insufficient evidence. Introduction reporting must distinguish earliest observed membership, description and concept records from proven introduction, with the scope of available history stated.

**Acceptance:** synthetic membership activation, inactivation, reactivation and module-dependent versions survive canonical/SQLite round trips without changing current projection semantics. Independently enumerated expected editions validate temporal consumers under `R25`; Snapshot-only or incomplete module history cannot claim exact as-at results or introduction dates. Keep `R25`'s existing evidence and scope intact.

## Verification Contract

- Extend the [committed synthetic RF2 fixture](../tests/fixtures/rf2/) and run RF2 -> canonical NDJSON -> the real `sct sqlite` schema in isolated temporary directories with isolated `SCT_DATA_HOME`, following [`tests/end_to_end.rs`](../tests/end_to_end.rs). Hand-built in-memory tables alone are not acceptance evidence.
- Cover multiple/absent/inactive definitions, independent definition/concept dates, layered replacements, missing refset concepts, filtered members, old artefacts and loaded-but-empty metadata. Assert exact values, capability states and deterministic results, not just successful parsing.
- Follow [output boundaries](output-boundaries.md): test quotes, CR/LF, tabs, pipes, backticks, markup and malformed identifiers at each representable input boundary. Inject values that RF2 TSV cannot represent at the canonical/SDK boundary, not as invalid RF2. JSON/YAML and any CSV interchange must round-trip original text; Markdown/terminal/GUI display must not create syntax or extra records. CSV quoting alone is not spreadsheet-formula sanitisation.
- Cross-check query logic with independently enumerated synthetic expectations and known concepts such as `22298006` (Myocardial infarction) and `46635009` (Type 1 diabetes mellitus). Optional licensed-release validation stays local and is not required for CI; it must not publish source definitions or dataset observations.
- Each delivered stage updates schema/SDK/command documentation and relevant boundary tests. Record skipped gates explicitly. These stages are not authorised for the autonomous nightly queue.
