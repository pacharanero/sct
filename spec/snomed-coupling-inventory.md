# SNOMED-required coupling inventory (`R63-inventory`)

Read-only audit for `R63` ("make the toolset SNOMED-optional"). No behaviour
changes in this document - it lists every place the toolchain currently
assumes a SNOMED release is loaded, so a human can sequence the wind-back
after `R61` (LOINC) lands. Entries are grouped by the categories named in the
roadmap item, each with file:line, why it assumes SNOMED, and a judgement on
whether unwinding it looks mechanical or needs design.

## 1. Default DB discovery (`crate::paths`)

- `src/paths.rs:367-403` - The `Kind` enum that drives the five-step discovery
  chain (env var -> CWD -> config -> canonical name -> newest match) has
  exactly two variants, `Db` and `Embeddings`, both SNOMED-shaped:
  - `Kind::Db::human_name()` returns the literal string `"SNOMED CT database"`
    (`src/paths.rs:391`), which lands verbatim in the "not found" error.
  - `Kind::Db::cwd_name()` / the canonical filename is `CANONICAL_DB =
    "snomed.db"` (`src/paths.rs:56`).
  - `Kind::Db::build_hint()` (`src/paths.rs:396-401`) hardcodes the SNOMED
    build recipe (`sct trud download --edition uk_monolith --pipeline`, `sct
    sqlite --ndjson snomed.ndjson`).
  - **Judgement:** design work, not mechanical. `Kind` is a closed enum keyed
    to one artefact type; a second per-terminology database (LOINC's own
    `loinc.db`, per the `R61` same-database decision this is currently a
    side-table not a separate file, but the principle generalises) needs
    either a third `Kind` variant with its own canonical name/hint, or the
    enum needs to become parametric over terminology. The five-step
    algorithm itself (`resolve()`, `src/paths.rs:429+`) is not SNOMED-specific
    and can likely be reused as-is.
  - `resolve_db()` / `resolve_embeddings()` (`src/paths.rs:421-427`) are the
    only two public entry points every command calls; a new terminology's
    path resolution would need parallel functions or a generalised signature.

- `src/paths.rs:707-731` (tests) - assert the literal strings `"No SNOMED CT
  database found"`, `"$SCT_DB"`, `"./snomed.db"`, `"sct trud download"` in the
  not-found error. These tests would need updating (not necessarily breaking)
  if `Kind` grows a terminology-neutral message.

## 2. Per-command entry points that fail without a SNOMED build

Every command below calls `crate::paths::resolve_db` (directly or via
`resolve(Kind::Db, ...)`) and therefore fails immediately, with the
SNOMED-worded error above, if no database is discoverable. This is not a
defect - these commands have nothing to do without a database - but it means
the "no SNOMED release loaded" failure mode is universal today, and each
site would need to learn (or be told) which terminology's database it wants
once more than one exists:

`src/commands/bench.rs`, `src/commands/codelist.rs`,
`src/commands/diagram.rs`, `src/commands/ecl.rs`, `src/commands/gui.rs`,
`src/commands/history.rs`, `src/commands/lexical.rs`,
`src/commands/lookup.rs`, `src/commands/map.rs`, `src/commands/mcp.rs`,
`src/commands/proximal_primitives.rs`, `src/commands/read2.rs`,
`src/commands/refset.rs`, `src/commands/size.rs`, `src/commands/tui.rs`.

(`src/commands/sqlite.rs` and `src/commands/tct.rs` are the *build* commands
and intentionally do not call `resolve_db` - they write a new database.)

- **Judgement:** mechanical once `Kind`/`resolve_db` above support more than
  one terminology - these call sites just pass through whatever the resolver
  returns. The design work is entirely upstream in `paths.rs`.

## 3. The `Snomed` SDK type and `snomed_*` MCP tool names

- `src/sdk/mod.rs:47` - `pub struct Snomed { .. }` is the sole public SDK
  session type (`src/sdk/mod.rs:32-45` doc comment: "A read-only SNOMED CT
  query session"). Every consumer (CLI commands, the Python bindings, the MCP
  server, the FHIR server) constructs and holds a `Snomed`, not a
  terminology-generic type.
  - **Judgement:** needs design. Renaming is mechanical; the harder question
    is whether a second terminology gets its own session type, a shared
    trait, or (per the `R61` same-database decision) stays inside `Snomed`
    as additional methods over LOINC side tables in the same connection.
    The roadmap's near-term LOINC plan (side tables, same DB) suggests the
    second path for now, deferring the SDK-level split.

- `python/src/lib.rs:65-66` - `#[pyclass(name = "Snomed", module = "sct_py")]
  struct PySnomed` wraps `RustSnomed` 1:1 (imported as `Snomed as RustSnomed`,
  `python/src/lib.rs:10`). `python/sct_py/__init__.pyi:17` (`class Snomed:`)
  mirrors it in the type stub. Same coupling as above, one layer out; a
  renamed/generalised Rust type would need the Python binding and stub
  updated in lockstep, and would be a breaking change for any existing
  `python/` consumer (`from sct_py import Snomed`).

- `src/commands/mcp.rs` - every registered tool is prefixed `snomed_`:
  `snomed_search`, `snomed_concept`, `snomed_children`, `snomed_ancestors`,
  `snomed_hierarchy`, `snomed_map`, `snomed_refsets`, `snomed_refset_members`,
  `snomed_refset_compare`, `snomed_refset_profile`, `snomed_semantic_search`
  (tool list at `src/commands/mcp.rs:1602-1674`; dispatch at
  `src/commands/mcp.rs:1040-1065` and `942-1005`). Result payloads also use
  SNOMED-specific key names, e.g. `"snomed_concepts"`
  (`src/commands/mcp.rs:1451-1452, 1892, 1967`), `"snomed_id"`
  (`src/commands/mcp.rs:1451, 1932`), `"snomed_release"`
  (`src/commands/mcp.rs:1528, 1567, 2136, 2182, 2454`).
  - **Judgement:** design decision with a real compatibility cost. These are
    a public MCP contract already documented and relied on by MCP clients
    (`spec/roadmap.md`'s own `R58` shipped "MCP output-schema conformance"
    against these exact names). Renaming to a terminology-namespaced form
    (e.g. `sct_search` + a `terminology` argument, or `loinc_*` siblings) is
    a breaking change and needs a deprecation/aliasing story, not a
    mechanical rename. The roadmap already flags this directly: "CLI
    namespacing (`sct snomed <command>` / `sct loinc <command>`) is a later
    step, deferred for now" (`spec/roadmap.md:10`) - the same reasoning
    applies one-for-one to the MCP tool names.

## 4. `SNOMED_SYSTEM` and FHIR single-system assumptions

- `src/commands/serve/fhir.rs:10` - `pub const SNOMED_SYSTEM: &str =
  "http://snomed.info/sct";` is the primary code system URI the FHIR server
  serves content for. `system_to_internal`/`internal_to_system`
  (`src/commands/serve/fhir.rs:14-33`) already map several *crossmap target*
  systems (`icd10`, `opcs4`, `ctv3`, `read2`) alongside `snomed` - that part
  is not a coupling to fix, it is existing multi-system support for
  `$translate`. The coupling is narrower: every call site that gates on
  `system_to_internal(s) == Some("snomed")` specifically
  (`src/commands/serve/ops.rs:176,228`) is asserting that the *primary*
  system being looked up/validated/expanded must be SNOMED - `$translate`'s
  own source/target resolution (`src/commands/serve/ops.rs:1283-1291`) uses
  the same function generically and imposes no such restriction.
- `src/commands/serve/fhir.rs:318-337` (`code_system()`) - the server's sole
  `CodeSystem` resource: fixed `id: "sct"` (`CODE_SYSTEM_ID`,
  `src/commands/serve/fhir.rs:318`), `url: SNOMED_SYSTEM`, `name:
  "SNOMEDCT"`. `GET /CodeSystem` and `GET /CodeSystem/{id}` (per the roadmap,
  already shipped) can therefore only ever describe this one resource.
- `src/commands/serve/ops.rs:178` - `$lookup`/`$validate-code` reject any
  `system` other than `SNOMED_SYSTEM` with `"`system` must be SNOMED CT
  ({SNOMED_SYSTEM}); this server does not serve '{s}'"`.
- `src/commands/serve/ops.rs:197,202` - `$validate-code`'s `version`
  parameter is rejected outright: `"cannot honour `version`: this database
  records no SNOMED CT release version"` / `"`version` requires SNOMED CT
  version {requested}, but this server has {loaded} loaded"`.
- `src/commands/serve/mod.rs:1262,1271,1383,1433` - `ValueSet/$expand`
  rejects `value-set-version`, `exclude-system`, and inline `valueSet`
  validation with messages naming SNOMED CT explicitly (`"this server serves
  SNOMED CT only, so excluding a system cannot be honoured"`, `"this server
  validates only against its own loaded SNOMED CT release"`).
- `src/commands/serve/ops.rs:810,839,1157` and
  `src/commands/serve/fhir.rs:128,140,224,252,258,264,268,340` - every FHIR
  response-building helper (`$lookup` designations, `$expand` `contains`
  entries, ValueSet compose, CapabilityStatement's `codeSystem` list) stamps
  `SNOMED_SYSTEM` into the payload directly rather than reading it from a
  per-request or per-database terminology identity.
- `src/commands/serve/mod.rs:468` - request routing itself gates on `url !=
  fhir::SNOMED_SYSTEM` to decide whether a `system` parameter is even
  acceptable.
- `src/commands/codelist.rs:1643` - a **second, independent** definition of
  `SNOMED_SYSTEM` (same URI, same name, different module) used by codelist
  v1's FHIR import/export (`src/commands/codelist.rs:1054-1056,1693`), which
  rejects any other `system` in a `ValueSet.compose` group with `"codelist
  format v1 accepts SNOMED CT only"`. This is the exact constant the roadmap
  already earmarks for change: format v2 (`R13`) is specifically about
  allowing non-SNOMED source codes here.
- **Judgement:** design work, and the single biggest surface. The FHIR layer
  assumes throughout - in routing, in error messages, and in every response
  builder - that there is exactly one code system with one fixed URI. Making
  this multi-system means at minimum: a lookup table from `system` URI to
  the right query backend (SNOMED tables vs. LOINC side tables), a
  `CodeSystem` resource per loaded terminology, and rewording every
  "SNOMED CT only" refusal to name the actual set of systems loaded. None of
  this is a rename; it changes control flow.

## 5. The database schema itself

- `src/commands/sqlite.rs:590-705` - every table (`concepts`, `concept_isa`,
  `concept_relationships`, `concept_maps`, `refset_members`,
  `complex_map_refset_members`, `extended_map_refset_members`,
  `attribute_value_refset_members`, `concept_history`, `metadata`) is
  RF2-shaped: `concept_isa` encodes SNOMED's IS-A hierarchy specifically,
  `refset_members` and its variants are SNOMED reference-set mechanics, and
  the roadmap's own architecture decision (`spec/roadmap.md:10`) already
  states this is deliberate and permanent: "No other terminology's codes
  ever become rows in the SNOMED `concepts` table." LOINC is designed to
  land as separate side tables in the same database, not as `concepts` rows.
  - **Judgement:** not something to "wind back" - this is the settled
    architecture, not an oversight. Listed here only because it is the
    foundation every other coupling in this document sits on top of: a
    query path that joins straight into `concepts`/`concept_isa` without
    going through the `Snomed` SDK surface would be a new, undocumented
    SNOMED-required coupling and should be caught in review.

## 6. Error-message wording (beyond the FHIR/paths ones above)

- `src/commands/lookup.rs:107,240,258` - CTV3 lookup: `"No SNOMED CT mapping
  found for CTV3 code '{code}'."` Wording assumes the target of every
  crossmap lookup is SNOMED; correct today (crossmaps only ever resolve to
  SNOMED SCTIDs) but would need a system-aware message if crossmaps became
  bidirectional across terminologies.
- `src/commands/map.rs:301` - `"{code} ({from})  ->  (no SNOMED CT match)"`,
  same assumption in the human-readable table output.
- `src/commands/mcp.rs:1926,1957` - `"No mappings found for SNOMED CT
  concept {} in this database."` / `"No SNOMED CT mapping found for {} code
  '{}'."` - MCP-side equivalents of the two above.
- `src/commands/mcp.rs:1190` - `"SNOMED CT database access is read-only.
  Codelist paths are restricted to {}."` - generic read-only guard, worded
  with the specific database name.
- `src/commands/codelist.rs:895` - `bail!("the source contains no explicit
  SNOMED CT concepts")` when a codelist import finds nothing convertible.
- `src/commands/codelist.rs:1101` - `bail!("{location}: expected a numeric
  SNOMED CT code, got {code:?}")` - codelist v1 assumes every code is a
  numeric SCTID; format v2 (`R13`) is exactly the design gate for this.
- **Judgement:** mechanical to reword once the underlying operation is
  actually multi-terminology (items 3-5 above); premature to change the
  wording before that, since today every one of these messages is accurate.

## 7. Tests that would break without SNOMED data

- The entire test suite's query-correctness tier depends on
  `tests/fixtures/rf2/` (a committed synthetic RF2 release) and the `build()`
  helper in `tests/end_to_end.rs`, per `AGENTS.md`'s verification contract.
  Every test file that calls that helper - `end_to_end.rs`, `cli.rs`,
  `codelist_compose.rs`, `ecl_eval.rs`, `embed_semantic.rs`,
  `fhir_conformance.rs`, `fst_index.rs`, `mcp-protocol.rs`, `rf2_parsing.rs`,
  `sdk.rs`, `serve.rs`, `snapshots.rs`, `transcode.rs`, `trud_api.rs`, plus
  the unit tests inside `src/commands/mcp.rs` (e.g. the cross-checks against
  concept `7000000` at `src/commands/mcp.rs:3070,3115-3117`) - would need a
  LOINC (or other) equivalent fixture and builder before it could exercise a
  non-SNOMED path; none of them currently can.
- `src/paths.rs:707-770` (unit tests) assert the literal SNOMED-worded
  strings from item 1 above and the `snomed.db`/`snomed-embeddings.arrow`
  canonical names (`src/paths.rs:60-61`); these are the first tests that
  would need to change, not just gain siblings, once `Kind` generalises.
- **Judgement:** additive, not corrective. The fixture-based discipline
  itself is sound and should be replicated for LOINC (`R61` already scopes
  its own ingest verification) rather than changed; the `paths.rs` literal
  assertions are the one place existing tests actively encode the
  SNOMED-only assumption and will need editing alongside item 1's fix.

## Summary for sequencing

Trivial/mechanical once the design items land: item 2 (command entry
points) and item 6 (error wording) follow automatically from fixing items 1,
3, and 4 - they contain no independent design decisions of their own.

Needs a human design decision, roughly in the order a fix would have to
happen: **(1) `paths.rs`'s `Kind` enum and canonical-name/build-hint
scheme**, then **(4) the FHIR server's single-`SNOMED_SYSTEM` assumption**
(routing, `CodeSystem` resource, per-operation refusals), then **(3) the
`Snomed` SDK type / Python binding / `snomed_*` MCP tool names**, the last
of these carrying real backward-compatibility cost because the MCP tool
names are an already-shipped, externally consumed contract. Item 5 (schema
shape) is intentionally out of scope - it is the architecture, not a
coupling to unwind.
