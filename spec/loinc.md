# LOINC ingest and side tables (R61 design)

Design for the first LOINC pass: the canonical `loinc.ndjson` artefact, the SQLite side tables, the command shapes, and the licence compliance hooks. Decisions recorded here were taken against the staged **LOINC 2.83** release (see `R61` in [`roadmap.md`](roadmap.md) for the licence basis and shipping decision). This note is the implementation spec; changes to it are design decisions and get made explicitly.

## Source artefacts

- `LoincTable/Loinc.csv` - the full Table: 40 columns, 112,405 terms (99,737 ACTIVE, 5,436 TRIAL, 5,008 DEPRECATED, 2,224 DISCOURAGED), 7,355 rows carrying `EXTERNAL_COPYRIGHT_NOTICE`. This is the ingest source for canonical builds.
- `LoincTableCore/LoincTableCore.csv` - the same terms in a 15-column subset. This is the basis of the **shipped starter artefact** (see Shipping below), not a separate ingest path.
- Group 3 accessories (Answer lists, Part file, Groups, hierarchies, linguistic variants, Panels) are **out of scope** for the first pass, as is the `loinc.xml` RELMA table.
- There is **no machine-readable version file** in the release; the version comes from the download (zip name / release page). The ingest therefore takes the LOINC version as an explicit, required CLI argument and records it in provenance - never silently derived from a filename.

## Canonical `loinc.ndjson`

One JSON object per LOINC term, plus a first-line provenance record reusing the existing machinery verbatim (`_type: "sct_provenance"`, content fingerprint; `edition_label = "LOINC"`, `release_id = "2.83"`). Field values are carried **verbatim** from the CSV - licence condition (b) permits adding fields, never altering contents. Curated fields:

- `loinc_num`, `status` (ACTIVE/TRIAL/DEPRECATED/DISCOURAGED), `status_reason`, `status_text`
- The six axes verbatim: `component`, `property`, `time_aspct`, `system`, `scale_typ`, `method_typ`
- `fsn` - **derived** (an added field, permitted): the LOINC fully-specified name convention `{Component}:{Property}:{Time_aspct}:{System}:{Scale_typ}:{Method_typ}`, with empty trailing axes elided. The licence's display-name condition counts this FSN, the `long_common_name`, `shortname`, or `display_name` - all four are carried.
- `long_common_name`, `shortname`, `display_name`, `definition_description`
- `relatednames` - `RELATEDNAMES2` split on `;` into a JSON array (search synonyms)
- `class`, `classtype` (1 = Laboratory, 2 = Clinical, 3 = Claims attachments, 4 = Surveys)
- `example_units`, `example_ucum_units`
- `external_copyright_notice` (empty string when absent - never omitted, so the field's emptiness is itself evidence), `external_copyright_link`
- `version_first_released`, `version_last_changed`

Unlike the SNOMED pipeline, **all statuses are ingested** - the LOINC Table ships every status in one file, so status filtering happens at query time, not build time. The canonical artefact staying complete is also what makes it diffable across releases.

## SQLite side tables

`sct loinc sqlite --ndjson loinc.ndjson --db <path>` creates or replaces, inside the **same** database file as SNOMED (per the R61 storage decision):

- `loinc_concepts` - `loinc_num TEXT PRIMARY KEY`, the curated fields above as columns
- `loinc_fts` - FTS5 over `long_common_name`, `shortname`, `fsn`, `component`, and the joined `relatednames` array
- provenance via the existing `metadata` table, namespaced keys (`loinc.version`, `loinc.edition`, `loinc.fingerprint`)

The command refuses to run against a database whose SNOMED schema is absent **only** if that changes in future; today it simply adds the LOINC side tables and is the first step of `R63`'s SNOMED-optional direction: nothing in the build reads SNOMED tables. Re-running with the same version replaces the side tables atomically (drop + rebuild in one transaction); a different version replaces too, recording the change in provenance.

## Command shapes

- `sct loinc ndjson --loinc-table <Loinc.csv> --version <v> --output <loinc.ndjson>` - canonical artefact build. `--version` is required.
- `sct loinc sqlite --ndjson <loinc.ndjson> --db <path>` - side-table build.
- `sct loinc lookup <LOINC_NUM> [--db]` - exact-code lookup, any status, status always surfaced; honours `OutputFormat`.
- `sct loinc search <query> [--db] [--status active|all] [--limit]` - FTS5 search, default `active` (ACTIVE only; DISCOURAGED and TRIAL excluded from the default, DEPRECATED always). Data on stdout, "no results" hint on stderr - the established contract.

A `sct loinc` subcommand family keeps every LOINC touchpoint out of the SNOMED command surfaces - additive, no renames, and it is the natural shape for the eventual `R63`/namespacing world.

## Query-surface integration (first scope)

- **FHIR**: when the LOINC side tables are present, `sct serve` serves a second `CodeSystem` at `http://loinc.org`: `$lookup` and `$validate-code` honoured; `$subsumes` **refused** for LOINC (the Table has no subsumption hierarchy; the part hierarchy is Group 3, out of scope); implicit SNOMED ValueSet forms do not apply. `R17c`'s router-derived CapabilityStatement table gains a `loinc_present`-conditional entry - the exactness test must cover both states (with and without LOINC loaded), mirroring the `translate_available` pattern.
- **MCP**: `loinc_lookup` and `loinc_search` tools, following the established `snomed_*` schema-conformance discipline from day one (`assert_conforms` coverage in the same PR that adds the tools - the `snomed_hierarchy` regression showed what happens when a tool ships unverified).
- **Codelists**: deferred to `R13` (multi-terminology codelist format v2 is its own design gate).

## Licence compliance hooks

- The attribution notice (verbatim in [`docs/sdk/data-licensing.md`](../docs/sdk/data-licensing.md)) appears in `--version`/about output and the docs - added in the same PR as the first LOINC command.
- Extracted content always carries `loinc_num` + one of the four display names - satisfied structurally by the record shape.
- Locally-ingested full builds keep `external_copyright_notice` verbatim on every row; consumers extracting content must propagate it (the licence's condition, surfaced by the field never being omitted).

## Shipping (the starter artefact)

`loinc.db` built from `LoincTableCore` **minus** the `EXTERNAL_COPYRIGHT_NOTICE` rows (~105,000 terms; Section 10.2's sanctioned deletion path), attached to GitHub releases with the full LOINC licence text on the release page (same-page condition). Open logistics question: CI has no LOINC account, so the artefact is built by an `s/` script from the locally staged release and uploaded at release time - if that proves annoying, revisit (the licence would permit publishing the derived artefact from a public source, but local-build-first keeps the supply chain simple).

## Verification

- Ingest counts must match the release exactly (112,405 terms; the four-status distribution; 7,355 copyright-notice rows for 2.83).
- Spot-check stable, well-known codes: 2164-2 (Creatinine [Mass/volume] in Serum or Plasma), 8867-4 (Heart rate), 8480-6 (Systolic blood pressure).
- Tests use a synthetic LOINC CSV fixture with **invented LOINC-shaped codes whose check digits deliberately fail** - so a synthetic id can never collide with a real LOINC term (mirroring the synthetic RF2 fixture's safety property).
- Per the verification contract: the real `loinc sqlite` schema is what tests exercise; a hand-built in-memory schema is insufficient evidence.
