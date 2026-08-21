# `sct serve` - FHIR R4 Terminology Server

A spec for implementing `sct serve`: a FHIR R4 HTTP terminology server backed by the SQLite
artefact produced by `sct sqlite`. The goal is a standards-compliant, drop-in replacement for
hosted FHIR terminology services (Ontoserver, Snowstorm, NHS Terminology Server) in development,
testing, and organisational production use.

> **Status: Phase 1 shipped + stored ValueSets** (feature-gated `serve`). `/metadata`,
> `CodeSystem/$lookup` / `$validate-code` / `$subsumes`, and `ValueSet/$expand` are implemented in
> `src/commands/serve/` with hand-rolled FHIR JSON. **The "biggest gap" below - ECL - is closed:**
> `$expand` runs the full [`sct` ECL engine](../ecl.md) (`crate::ecl`), so hierarchy, refset `^`,
> boolean, and attribute refinement all work. **Stored/named ValueSets are also shipped (§4 below):**
> `--codelists <dir>` serves `.codelist` files (composition resolved) as ValueSets via
> `GET /ValueSet`, `GET /ValueSet/{id}`, `/$expand`, and `ValueSet/$validate-code`. A list's
> front-matter `canonical_url` overrides the derived `{base}/ValueSet/{id}` when set, and
> `GET /ValueSet?status=` filters the registry by FHIR status. User docs:
> [`docs/commands/serve.md`](../../docs/commands/serve.md). The sections below are the original
> design; annotations note where reality has moved on.

---

## Overview

```bash
sct serve --db snomed.db [--port 8080] [--host 127.0.0.1]
```

Starts a long-running HTTP server that exposes FHIR R4 `CodeSystem` and `ValueSet` operations
over the SQLite database. FHIR clients (EHR systems, HL7 validators, SMART apps, integration
engines) can point at it with no change to their configuration other than the base URL.

---

## Motivation

Most FHIR terminology servers are:

- **Remote** - every lookup is a network round trip (200–2000 ms)
- **Operationally heavy** - Elasticsearch, PostgreSQL, Docker, JVM heap tuning
- **Expensive** - Ontoserver licences are per-organisation; Snowstorm requires significant
  infrastructure
- **Opaque** - you cannot inspect the data with standard tools

`sct serve` is backed by a plain SQLite file. Lookups are sub-millisecond. The entire server is
a single statically-linked binary. The database is inspectable with `sqlite3`. It can run on a
developer laptop, in a CI container, on a Raspberry Pi, or in a small VM without any
infrastructure overhead.

---

## Assumptions challenged

Before treating `sct serve` as an Ontoserver drop-in replacement, the following gaps need to
be understood and consciously accepted or worked around. They are documented here rather than
buried in implementation notes because they will surface in production.

### 1. ECL coverage is partial - this is the biggest gap

Ontoserver's commercial value is largely its Expression Constraint Language (ECL) engine.
`ValueSet/$expand` with ECL is the workhorse of modern SNOMED usage in UK clinical systems.

`sct serve` can handle the common ECL subset using the `concept_isa` table:

| ECL expression | Implementation | Status |
|---|---|---|
| `<<74400008` | subtypes (recursive) via CTE on `concept_isa` | Supported |
| `<!73211009` | direct children via single JOIN on `concept_isa` | Supported |
| `>>74400008` | ancestors (recursive) via CTE on `concept_isa` | Supported |
| `>!73211009` | direct parents via single JOIN on `concept_isa` | Supported |
| `74400008` | single concept | Supported |
| `A OR B` | set union | Supported (simple cases) |
| `A AND B` | set intersection | Supported (simple cases) |
| `A MINUS B` | set difference | Supported (simple cases) |
| `^900000000000497000` | member of reference set | **Supported** (Simple refsets loaded) |
| `* : 246075003 = 372687004` | attribute filter | **Supported** (schema-v4 `concept_relationships`) |
| `< 373873005 \|Pharmaceutical\| : 411116001 = <<17234001` | complex attribute | **Supported** |

**Update (resolved):** the ECL engine was built (`src/ecl/`, `crate::ecl`) and `$expand` now uses
it directly, so the rows above are supported. The remaining ECL gaps are the genuinely rare
constructs the engine defers (cardinality `[1..*]`, reverse `R`, dotted attributes) - see
[`spec/ecl.md`](../ecl.md) §5. `sct serve` is no longer blocked from replacing Ontoserver on ECL
grounds for the common-to-advanced subset.

### 2. Reference set membership and payload families

UK clinical systems use SNOMED reference sets heavily: drug extension, national reference sets,
map reference sets, language reference sets, simple reference sets for administrative coding.
The ECL `^` operator (member-of) requires refset membership data.

**Loaded today:**
- Concept-level Simple refsets (`der2_Refset_Simple*Snapshot*.txt`) are loaded into the
  `refset_members` table via `sct ndjson --refsets simple` (default) + `sct sqlite`. This covers
  the UK administrative refsets (SCR exclusion, care connect, accessibility, etc.) - the bulk
  of what clinical systems need `^` ECL for. The member-of operator intentionally retains these
  concept-set semantics.
- Language refsets (used for preferred term selection) - unchanged.
- CTV3 / Read v2 simple map refsets - unchanged.
- Extended Map refsets feed the shipped ICD-10/OPCS-4 `ConceptMap/$translate` projection when
  the map refset identifies a known target.
- Association refsets feed `concept_history` for inactive-concept forwarding.
- Complex Map, Extended Map, and Attribute Value members are preserved losslessly under
  `--refsets all` in dedicated tables rather than being inserted into `refset_members`. The
  Attribute Value inactivation reason is the substrate for the next coherent inactive-concept /
  history presentation.

### 3. Single edition, single version per process

`sct serve` serves whatever is in `--db`. This note originally recommended accepting and
logging a `$lookup` `version` parameter for a different release without acting on it - "pragmatic,
document clearly". `R17` (August 2026) superseded that: silently ignoring a parameter whose whole
purpose is to pin the release a client is willing to accept is exactly the failure class that
shipped three `ValueSet/$expand` defects, so `$expand`'s `check-system-version`/`system-version`
and `$lookup`'s `system`/`version` (`R17b-lookup`) all now **refuse** a value that disagrees with
what's loaded (`400`, not `404` - the concept may well exist, just not at the requested vintage),
rather than answering from the wrong release. See the governing invariant under `R17` below and
[`check_lookup_system`/`check_lookup_version`](../../src/commands/serve/ops.rs).

Ontoserver can host multiple editions and versions concurrently and route requests accordingly.
`sct serve` is explicitly single-edition. A multi-instance deployment (one process per edition)
behind a reverse proxy is the intended pattern for multi-edition use.

### 4. No stored ValueSet resources

Ontoserver maintains a registry of named `ValueSet` resources that FHIR clients can `GET` by
ID. `sct serve` has no such registry. Instead, it supports SNOMED's *implicit ValueSet* URL
pattern, where the ECL expression is embedded in the URL:

```
http://snomed.info/sct?fhir_vs=ecl/<<74400008
```

Named ValueSets (e.g. `http://hl7.org/fhir/ValueSet/condition-code`) would require a separate
ValueSet registry (YAML files or a lightweight table in the DB). This is deferred; scope it if
clients need it.

### 5. ConceptMap and $translate are out of scope for phase 1

`ConceptMap/$translate` is used for SNOMED→ICD-10, SNOMED→OPCS-4, SNOMED→Read v2 maps. The
roadmap concept-maps feature is the prerequisite. CTV3 and Read v2 reverse maps are already in
the SQLite schema (`concept_maps` table), so a basic `$translate` for those two systems is
achievable in phase 2.

### 6. SMART on FHIR / OAuth2

Ontoserver supports SMART on FHIR for enterprise SSO. `sct serve` will not implement OAuth2 in
the initial phases. For internal or development use this is acceptable. For internet-facing
deployment, operate behind a reverse proxy (nginx/Caddy) that handles TLS and auth.

### 7. $closure operation

The `ConceptMap/$closure` operation maintains an incremental transitive closure table for
FHIR clients that cache subsumption results. It is rarely used and is explicitly out of scope.

---

## FHIR R4 operations

### Must-have (phase 1)

| Endpoint | Method | Description |
|---|---|---|
| `/metadata` | GET | CapabilityStatement declaring supported operations |
| `/CodeSystem/$lookup` | GET, POST | Look up a SNOMED concept by SCTID |
| `/CodeSystem/$validate-code` | GET, POST | Check if a code is active and valid |
| `/CodeSystem/$subsumes` | GET, POST | Subsumption (is-a) check between two codes |
| `/ValueSet/$expand` | GET, POST | Expand a ValueSet - text filter + simple ECL |

### Should-have (phase 2)

| Endpoint | Method | Description |
|---|---|---|
| `/ValueSet/$validate-code` | GET, POST | Validate a code against a named or ECL ValueSet |
| `/CodeSystem` | GET | Bundle listing the loaded SNOMED CT CodeSystem resource |
| `/CodeSystem/{id}` | GET | Retrieve the SNOMED CT CodeSystem resource metadata |
| `/` (batch) | POST | FHIR batch Bundle of GET requests |

### Nice-to-have (phase 3)

| Endpoint | Method | Description |
|---|---|---|
| `/ConceptMap/$translate` | GET, POST | CTV3/Read2→SNOMED and SNOMED→CTV3/Read2 |
| `/ValueSet` | GET | List synthetic SNOMED implicit ValueSets |

---

## CodeSystem/$lookup detail

**Request:**

```
GET /CodeSystem/$lookup?system=http://snomed.info/sct&code=22298006&property=display&property=designation&property=parent&property=child
```

**Input parameter handling (`R17b-lookup`, transcribed from all 7 R4 `$lookup` parameters -
see [`tests/fhir_conformance.rs`](../../tests/fhir_conformance.rs)):**

| Parameter | Behaviour |
|---|---|
| `code` | Required (with `system`/`coding` absent, since `coding` is refused - see below) |
| `system` | Optional; must be SNOMED CT (`http://snomed.info/sct`, or bare `snomed`) when supplied - a mismatched `system` is refused with `400`, not silently treated as SNOMED |
| `version` | Optional; must equal the loaded release's recorded version when supplied - a mismatch is refused with `400` (see the superseded recommendation under "Single edition, single version per process" above) |
| `coding` | Refused with `400`: this server reads parameters from the query string only and does not parse a POST `Parameters` body, so the standard `Coding`-datatype invocation cannot be honoured. Supply `system` and `code` instead |
| `date` | Refused with `400`: this server holds a single release and cannot look up concept information as at a past date |
| `displayLanguage` | Accepted, no effect: like `$expand`, `sct` bakes one locale's preferred terms and designations into the database at `sct ndjson --locale` build time, so no requested language can change `$lookup`'s output |
| `property` | Optional, repeatable; selects which properties are returned (see below). Absent means the default set (`display` + `designation`) |

A request body (e.g. a POST `Parameters` resource) is refused outright for the same reason as
`coding`, rather than silently discarded.

**Supported `property` values:**

| Property | Source in SQLite | Notes |
|---|---|---|
| `display` | `concepts.preferred_term` | Always returned |
| `designation` | `concepts.synonyms` + `concepts.fsn` | Returns FSN + all synonyms |
| `parent` | `concept_isa.parent_id` JOIN `concepts` | Direct parents only |
| `child` | `concept_isa.child_id` JOIN `concepts` | Direct children only |
| `ancestor` | recursive CTE on `concept_isa` | All ancestors |
| `inactive` | `concepts.active` | Boolean |
| `moduleId` | `concepts.module` | SNOMED module SCTID |
| `effectiveTime` | `concepts.effective_time` | RF2 effective date |

A `property` value not in this list is currently accepted and silently produces no matching
output part, rather than an `OperationOutcome` naming it as unsupported. This is narrower than
the `R17` invariant asks of the top-level *parameters* above (it cannot broaden a result - an
unrecognised property can only mean less comes back, never more, or a different code being
answered) but is still an accepted gap; a future pass could report it explicitly.

**Response shape:** standard FHIR `Parameters` resource.

---

## ValueSet/$expand detail

**Request patterns:**

```
# Text filter (free-text search via FTS5)
GET /ValueSet/$expand?url=http://snomed.info/sct?fhir_vs&filter=heart+attack&count=10&offset=0

# Subtypes of a concept (recursive)
GET /ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl%2F%3C%3C73211009&count=100

# Direct children
GET /ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl%2F%3C!73211009&count=100

# Single concept
GET /ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl%2F73211009
```

**Parameter handling:**

| Parameter | Behaviour |
|---|---|
| `url` | Parsed for implicit SNOMED ValueSet URL; required |
| `filter` | FTS5 search applied after ECL hierarchical filter |
| `count` | Page size; default 100, max 1000 |
| `offset` | Pagination offset; default 0 |
| `includeDesignations` | Include synonyms in each expansion entry; default false |
| `designation` | Repeatable; selects *which* designations come back once designations are included - a `system\|code` or bare description-type code (`900000000000003001` FSN, `900000000000013009` Synonym), `*` for all, or a BCP-47 language tag (`en`/`en-*` matches everything, since `sct` serves one English locale; any other language tag matches nothing). Presence implies `includeDesignations=true` even if that parameter was absent or false |
| `activeOnly` | Filter to `concepts.active = 1`; default true |
| `excludeNested` | Accepted, no effect: expansions are always flat, so `contains` never nests |
| `displayLanguage` | Resolved, not routed: `sct` loads one undifferentiated English locale per DB (`--locale` at `sct ndjson` build time selects the preferred-term dialect but is not persisted), so a request whose primary BCP-47 subtag is `en` is honoured verbatim and anything else falls back to bare `en`; the resolved value is echoed back on `expansion.parameter` (omitted entirely when the client didn't ask), never silently ignored |
| `version` | Accepted, logged, not used for routing |

**Response shape:** standard FHIR `ValueSet` resource with `expansion` element.

---

## CodeSystem/$subsumes detail

```
GET /CodeSystem/$subsumes?system=http://snomed.info/sct&codeA=44054006&codeB=73211009
```

Returns a `Parameters` resource with outcome `subsumedBy`, `subsumes`, `equivalent`, or
`not-subsumed`. Implemented via recursive CTE on `concept_isa` - same query as `sct lexical`
subsumption.

---

## CapabilityStatement (/metadata)

The CapabilityStatement is the contract between server and client. It must be accurate; clients
use it to decide what to attempt. Key fields:

```json
{
  "resourceType": "CapabilityStatement",
  "status": "active",
  "fhirVersion": "4.0.1",
  "kind": "instance",
  "software": {
    "name": "sct",
    "version": "<binary version>"
  },
  "implementation": {
    "description": "SNOMED CT FHIR R4 terminology server backed by SQLite",
    "url": "<--host>:<--port>"
  },
  "rest": [{
    "mode": "server",
    "resource": [
      {
        "type": "CodeSystem",
        "operation": [
          { "name": "lookup",         "definition": "http://hl7.org/fhir/OperationDefinition/CodeSystem-lookup" },
          { "name": "validate-code",  "definition": "http://hl7.org/fhir/OperationDefinition/CodeSystem-validate-code" },
          { "name": "subsumes",       "definition": "http://hl7.org/fhir/OperationDefinition/CodeSystem-subsumes" }
        ]
      },
      {
        "type": "ValueSet",
        "operation": [
          { "name": "expand",         "definition": "http://hl7.org/fhir/OperationDefinition/ValueSet-expand" },
          { "name": "validate-code",  "definition": "http://hl7.org/fhir/OperationDefinition/ValueSet-validate-code" }
        ]
      }
    ]
  }]
}
```

The CapabilityStatement is generated at startup from the feature flags compiled in and the DB
version loaded.

---

## HTTP server architecture

`sct serve` reuses the Axum + Tokio HTTP stack already present in `--features gui`. It should
be a separate `serve` Cargo feature that can be compiled without the GUI:

```
--features serve      # FHIR server only
--features gui        # browser UI only (current)
--features full       # gui + serve + tui
```

**Route table:**

```rust
Router::new()
    .route("/metadata",                         get(capability_statement))
    .route("/CodeSystem",                       get(list_code_systems))
    .route("/CodeSystem/:id",                   get(get_code_system))
    .route("/CodeSystem/$lookup",               get(lookup).post(lookup))
    .route("/CodeSystem/$validate-code",        get(validate_code_cs).post(validate_code_cs))
    .route("/CodeSystem/$subsumes",             get(subsumes).post(subsumes))
    .route("/ValueSet",                         get(list_value_sets))
    .route("/ValueSet/$expand",                 get(expand).post(expand))
    .route("/ValueSet/$validate-code",          get(validate_code_vs).post(validate_code_vs))
    .route("/",                                 post(batch_handler))
    .layer(/* CORS, logging, Accept header negotiation */)
```

**Content negotiation:**

- Requests with `Accept: application/fhir+json` or `Accept: application/json` receive JSON
- Requests with `Accept: application/fhir+xml` receive a `406 Not Acceptable` with an
  `OperationOutcome` (XML is out of scope)
- Default (no Accept header) returns JSON

**Error handling:**

All error responses return FHIR `OperationOutcome` resources with appropriate HTTP status codes:

| Condition | HTTP | OperationOutcome issue code |
|---|---|---|
| Unknown code | 404 | `not-found` |
| Invalid parameter | 400 | `invalid` |
| Unsupported operation | 501 | `not-supported` |
| DB error | 500 | `exception` |
| XML requested | 406 | `not-supported` |

---

## CLI design

```
sct serve [OPTIONS]

Options:
  --db <PATH>           Path to snomed.db [required]
  --port <PORT>         TCP port to listen on [default: 8080]
  --host <HOST>         Host/address to bind [default: 127.0.0.1]
  --fhir-base <PATH>    FHIR base path [default: /]
                        Set to /fhir for Ontoserver-compatible URLs
  --log-level <LEVEL>   Logging verbosity: error|warn|info|debug [default: info]
  --read-only           Refuse write operations (always true; flag is for explicit documentation)
```

**Example - local dev, Ontoserver-compatible base path:**

```
sct serve --db snomed.db --port 8080 --fhir-base /fhir
# Endpoints at http://localhost:8080/fhir/CodeSystem/$lookup etc.
```

**Example - network-accessible server:**

```
sct serve --db /data/snomed.db --host 0.0.0.0 --port 8080
```

---

## SQLite query mapping

### Concept lookup

```sql
SELECT id, preferred_term, fsn, synonyms, active, module, effective_time
FROM concepts
WHERE id = ?1
```

### Subtypes (`<<` / `<`) and supertypes (`>>` / `>`)

A `$expand` whose ValueSet is a single hierarchy or refset operator on one
concept (`<<`, `<`, `>>`, `>`, `<!`, `>!`, `^`, or a bare concept), with no text
filter, takes a fast path that never materialises the full id set in Rust: an
indexed `COUNT` for `expansion.total` and an index-ordered `LIMIT`/`OFFSET` for
the page. When the transitive-closure table (`concept_ancestors`, from `sct tct`
/ `sct sqlite --transitive-closure`) has a valid completion marker, schema,
indexes, and invalidation triggers, the descendant/ancestor set is served straight from the
`(ancestor_id, descendant_id)` index with no sort:

```sql
-- total (proper descendants; self added in code for `<<`)
SELECT COUNT(*) FROM concept_ancestors
WHERE ancestor_id = ?1 AND descendant_id != ?1;

-- page, served in index order (no sort)
SELECT descendant_id FROM concept_ancestors
WHERE ancestor_id = ?1 AND descendant_id != ?1
ORDER BY descendant_id LIMIT ?2 OFFSET ?3
```

For `<<` / `>>` the focus concept is prepended to the page (FHIR does not mandate
an ordering), shifting the body offset by one. Without a usable closure table the
same shape falls back to a recursive CTE over `concept_isa`, which is correct but
much slower on large hierarchies - `sct serve` warns at startup when the table is
missing or unusable. Compound ECL (booleans, attribute refinement), text filters,
and the whole-CodeSystem expansion fall through to the general path, which
evaluates the ECL engine and paginates in Rust.

```sql
-- recursive-CTE fallback (no closure table)
WITH RECURSIVE subtypes(id) AS (
    SELECT child_id FROM concept_isa WHERE parent_id = ?1
    UNION
    SELECT ci.child_id FROM concept_isa ci JOIN subtypes s ON ci.parent_id = s.id
)
SELECT id FROM subtypes ORDER BY id LIMIT ?2 OFFSET ?3
```

### Direct children (<!)

```sql
SELECT c.id, c.preferred_term, c.fsn
FROM concept_isa ci
JOIN concepts c ON c.id = ci.child_id
WHERE ci.parent_id = ?1 AND c.active = 1
ORDER BY c.preferred_term
LIMIT ?2 OFFSET ?3
```

### Ancestors (>>, recursive)

```sql
WITH RECURSIVE ancestors(id) AS (
    SELECT ?1
    UNION ALL
    SELECT ci.parent_id
    FROM concept_isa ci
    JOIN ancestors a ON ci.child_id = a.id
)
SELECT c.id, c.preferred_term, c.fsn
FROM ancestors an
JOIN concepts c ON c.id = an.id
WHERE c.id != ?1
```

### Subsumption (is codeA subsumed by codeB?)

```sql
WITH RECURSIVE ancestors(id) AS (
    SELECT ?1
    UNION ALL
    SELECT ci.parent_id
    FROM concept_isa ci
    JOIN ancestors a ON ci.child_id = a.id
)
SELECT EXISTS(SELECT 1 FROM ancestors WHERE id = ?2)
```

### Text search with optional hierarchy filter

```sql
-- Subtypes with text filter (ECL + filter combined)
WITH RECURSIVE subtypes(id) AS (
    SELECT ?1
    UNION ALL
    SELECT ci.child_id FROM concept_isa ci JOIN subtypes s ON ci.parent_id = s.id
)
SELECT c.id, c.preferred_term, c.fsn
FROM concepts_fts f
JOIN concepts c ON c.id = f.id
JOIN subtypes st ON st.id = c.id
WHERE concepts_fts MATCH ?2
AND c.active = 1
LIMIT ?3 OFFSET ?4
```

---

## Refset support (prerequisite for `^` ECL and `$translate`)

### Shipped: Simple refsets

Concept-level Simple refsets are loaded into the `refset_members` table whenever `sct ndjson`
runs with `--refsets simple` (the default). Refsets are themselves SNOMED CT concepts, so no
separate catalog table is needed - JOIN `refset_members.refset_id` to `concepts.id` to get
the refset's preferred term, module, FSN, etc.

```sql
CREATE TABLE refset_members (
    refset_id                TEXT NOT NULL,   -- SCTID of the reference set (also a concept)
    referenced_component_id  TEXT NOT NULL,   -- SCTID of the member concept
    PRIMARY KEY (refset_id, referenced_component_id)
);
CREATE INDEX idx_refset_members_by_concept
    ON refset_members(referenced_component_id);
```

This unblocks the `^` ECL member-of operator for Simple refsets.

### ConceptMap storage

`ConceptMap/$translate` reads the same general `crossmaps` table as
`sct transcode`. CTV3 / Read v2 rows are represented as external source code ->
SNOMED CT target rows. ICD-10 / OPCS-4 rows are represented as SNOMED CT source
-> external target rows and require `sct ndjson --refsets all`.

---

## Benchmarking integration

`benchmarks/bench.sh --server http://localhost:8080` already works against any FHIR R4 server.
Once `sct serve` is running, the same benchmark suite measures it head-to-head against
Ontoserver or any other target. No changes to the benchmark scripts are needed.

The expected outcome is that `sct serve` significantly closes the gap between local SQLite
and remote FHIR (since the network hop is removed when run locally), while still being
substantially faster than Ontoserver on the same hardware.

---

## Implementation phases

### Phase 1 - Core operations (foundation)

Deliverables:

- `sct serve` binary with `--features serve`
- `/metadata` CapabilityStatement
- `CodeSystem/$lookup` (GET + POST) - properties: display, designation, parent, child, inactive, moduleId, effectiveTime
- `CodeSystem/$validate-code` (GET + POST)
- `CodeSystem/$subsumes` (GET + POST)
- `ValueSet/$expand` (GET + POST) - text filter only (no ECL)
- FHIR `OperationOutcome` error responses throughout
- Content negotiation (JSON only; 406 for XML)
- Basic request/response logging (INFO: method, path, status, latency)

Acceptance criteria:

- `benchmarks/bench.sh --server http://localhost:8080` completes all operations
- HL7 FHIR validator reports no structural errors on sample responses
- `sct serve` passes all existing bench fixture queries against `sct`'s own SQLite DB

### Phase 2 - ECL hierarchy + pagination

Deliverables:

- `ValueSet/$expand` ECL support: `<<`, `<!`, `>>`, `>!`, single concept, boolean `OR`/`AND`/`MINUS`
- Pagination (`count` + `offset`) on all collection endpoints
- `ValueSet/$validate-code`
- `CodeSystem` resource read (`GET /CodeSystem` and `GET /CodeSystem/{id}`)
- FHIR batch Bundle handler (`POST /`)
- `--fhir-base` path prefix flag

Acceptance criteria:

- `benchmarks/bench.sh` children and ancestors operations work via FHIR ECL
- A FHIR client (e.g. HAPI FHIR test suite) can perform a full terminology operation cycle

### Phase 3 - Refsets + ConceptMap

Prerequisites: `refset_members` table (done - shipped in `sct ndjson --refsets simple`)

Deliverables:

- `^` ECL operator (member-of reference set) via `refset_members` table
- `ConceptMap/$translate` for CTV3 and Read v2 via `crossmaps`
- `ConceptMap/$translate` for ICD-10 and OPCS-4 via `crossmaps` (requires `--refsets all`)
- `ValueSet/$expand` with `^` ECL

### Phase 4 - R5 + hardening

Deliverables:

- FHIR R5 CapabilityStatement (additive; R4 responses remain valid for R4 clients)
- Full ECL attribute filter support (if feasible; evaluate ECL parser crates)
- Named ValueSet registry (YAML files loaded at startup)
- Performance profiling and query optimisation under concurrent load
- Docker image / systemd unit file for server deployment

---

## Non-goals

- XML serialisation (`application/fhir+xml`)
- SNOMED CT post-coordination or expression evaluation (NNF, etc.)
- Write operations (create/update/delete resources)
- SMART on FHIR / OAuth2 (defer to reverse proxy)
- `ConceptMap/$closure`
- Full ECL v2.1 (dossier, history supplements, language scoping) - phase 4 stretch goal only
- Multi-edition / multi-version routing within a single process
- Concept authoring or editorial workflows
- FHIR R2 / DSTU3 compatibility
