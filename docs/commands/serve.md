# sct serve

Run a **FHIR R4 terminology server** over a SNOMED CT SQLite database - a lightweight, local, drop-in alternative to hosted services (Ontoserver, Snowstorm, the NHS FHIR Terminology Server) for development, testing, and small-scale production.

!!! note "Included by default"
    `sct serve` is compiled in by default (the `serve` Cargo feature is part of `default`), so it is present in the released binaries, `cargo install sct-rs`, and `s/install`. For a minimal build without the async HTTP stack, use `--no-default-features`.

**When to use:** a FHIR client (EHR, HL7 validator, SMART app, integration engine) needs `$lookup` / `$validate-code` / `$subsumes` / `$expand` and you want sub-millisecond, offline, single-binary terminology with no Elasticsearch, JVM, or Docker. The entire server is backed by one inspectable `snomed.db` file.

---

## Usage

```
sct serve [--db <FILE>] [--port <PORT>] [--host <HOST>] [--fhir-base <PATH>] [--codelists <DIR>]
```

| Flag | Default | Description |
|---|---|---|
| `--db <FILE>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |
| `--port <PORT>` | `8080` | TCP port to listen on. |
| `--host <HOST>` | `127.0.0.1` | Address to bind. Use `0.0.0.0` to accept remote connections. |
| `--fhir-base <PATH>` | `/` | Base path for all routes. Set to `/fhir` for Ontoserver-compatible URLs. |
| `--codelists <DIR>` | `./codelists` (or `$SCT_CODELISTS` / `[codelists] dir`) | Directory of `.codelist` files to serve as named FHIR ValueSets. |
| `--fst <FILE>` | `snomed.fst` beside the database, if present | FST index (from `sct fst build`) powering the `GET /autocomplete` endpoint. |
| `--read-only` | on | The server never writes; the flag documents that intent. |

```bash
# Local dev server
sct serve --db snomed.db

# Ontoserver-compatible base path, reachable on the network
sct serve --db snomed.db --host 0.0.0.0 --port 8080 --fhir-base /fhir
```

Responses are `application/fhir+json`. An `Accept` header that requests XML exclusively gets a `406` (XML is not supported).

## Search-as-you-type endpoint

With an FST index available (`--fst`, or a `snomed.fst` beside the database), the server also exposes a non-FHIR **`GET /autocomplete?q=<partial>&count=<n>`** endpoint - sub-millisecond, typo-tolerant autocomplete backed by the [FST index](fst.md), for a web front-end to hit per keystroke:

```bash
sct serve --db snomed.db --fst snomed.fst
curl 'http://localhost:8080/autocomplete?q=myocard&count=5'
# {"query":"myocard","hits":[{"id":"22298006","display":"Myocardial infarction","score":0.77,"tag":"disorder"}, ...]}
```

`id` is a JSON string (SCTIDs exceed 2^53). Without an FST index the endpoint returns `501`. It shares its engine with [`sct sayt`](sayt.md), which also offers an interactive TUI and a stdio line protocol over the same index.

## Docker Compose

For a full self-host walkthrough - a `caddy` reverse proxy in front of `sct`
for automatic HTTPS, optional basic auth, CORS, and the bootstrap/config
reference - see [Get your own terminology server](../deploy/index.md).

---

## Operations (Phase 1)

| Endpoint | What it does |
|---|---|
| `GET /metadata` | CapabilityStatement declaring the supported operations (add `?mode=terminology` for a **TerminologyCapabilities** statement) |
| `CodeSystem/$lookup` | Concept details: display, designations (FSN + synonyms), parents, children, ancestors, inactive, moduleId, effectiveTime |
| `CodeSystem/$validate-code` | Whether a code exists (and an optional `display` matches) |
| `CodeSystem/$subsumes` | Subsumption between two codes (`subsumes` / `subsumed-by` / `equivalent` / `not-subsumed`) |
| `ValueSet/$expand` | Expand by free-text `filter` (FTS5), **ECL**, or a stored `.codelist` (by canonical URL) |
| `ValueSet/$validate-code` | Whether a code is a member of a ValueSet (stored `.codelist` or implicit ECL) |
| `GET /ValueSet` | Searchset Bundle of the stored `.codelist` ValueSets |
| `GET /ValueSet/{id}` | The stored ValueSet resource (with `compose`) |
| `GET /ValueSet/{id}/$expand` | Expand a stored ValueSet by id |
| `ConceptMap/$translate` | Map a code across terminologies when the loaded database has `crossmaps` data (SNOMED CT ↔ ICD-10 / OPCS-4 / CTV3 / Read v2) |
| `POST /` (batch) | A FHIR `batch` Bundle of the above operations, executed in one request |

GET and POST are both accepted; parameters are read from the query string.

## Batch requests

`POST` a FHIR `batch` (or `transaction`) `Bundle` to the base path to run many operations in one round trip - handy for a client that would otherwise fire dozens of sequential `$lookup` / `$validate-code` / `$translate` calls. Each entry's `request.url` is a GET operation URL; the response is a `batch-response` Bundle with one entry per request (in order), each carrying an HTTP `response.status` and the result resource (or an `OperationOutcome` for that entry). Entries succeed or fail independently. The server is read-only, so entries must use `GET`.

```bash
curl -X POST 'http://localhost:8080/fhir' -H 'Content-Type: application/fhir+json' -d '{
  "resourceType": "Bundle", "type": "batch",
  "entry": [
    { "request": { "method": "GET", "url": "CodeSystem/$lookup?system=http://snomed.info/sct&code=22298006" } },
    { "request": { "method": "GET", "url": "CodeSystem/$subsumes?system=http://snomed.info/sct&codeA=46635009&codeB=73211009" } }
  ]
}'
```

### `$expand` and ECL

`$expand` accepts the FHIR implicit SNOMED ValueSet URL. The text `filter` runs over FTS5; the `ecl/` form runs the **full [`sct` ECL engine](ecl.md)** - so `$expand` supports hierarchy (`<<`, `<!`, `>>`, `>!`), refset membership (`^`), boolean (`AND`/`OR`/`MINUS`), and attribute refinement (`:`), well beyond simple subtype expansion. ECL and `filter` combine (intersection).

```bash
# Subtypes of Diabetes mellitus (URL-encoded ECL "<<73211009")
curl 'http://localhost:8080/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl/%3C%3C73211009'

# Free-text expansion
curl 'http://localhost:8080/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs&filter=heart+attack&count=10'

# Attribute refinement (finding site)
curl 'http://localhost:8080/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl/%3C%3C404684003%20:%20363698007%20=%20%3C%3C39057004'
```

`count` (default 100, max 1000) and `offset` paginate; the `expansion.total` reflects the full match set. `includeDesignations=true` adds FSN + synonyms to each entry.

### Stored ValueSets from `.codelist` files

Point `--codelists <dir>` (default `./codelists`) at a directory of [`.codelist`](codelist.md) files and the server exposes each as a named FHIR ValueSet. Composition is resolved at startup, so a list that `includes:` others is served as its full effective member set. **Security model: "public by placement"** - only files in the served directory are exposed; keep private lists elsewhere. Expansion reconciles each concept's display against the live database (falling back to the stored term for concepts absent from the loaded edition).

```bash
sct serve --db snomed.db --codelists ./codelists &

# List the served ValueSets
curl 'http://localhost:8080/ValueSet'

# Read one (full resource with compose.include.concept)
curl 'http://localhost:8080/ValueSet/diabetes'

# Expand it (by id, or by its canonical URL via $expand?url=...)
curl 'http://localhost:8080/ValueSet/diabetes/$expand?count=20'

# Validate membership
curl 'http://localhost:8080/ValueSet/$validate-code?url=http://localhost:8080/ValueSet/diabetes&code=46635009'
```

The canonical URL of a served list is `{server-base}/ValueSet/{id}`. `$validate-code` also works against an implicit ECL value set (`?url=http://snomed.info/sct?fhir_vs=ecl/...`).

### Cross-terminology translation (`ConceptMap/$translate`)

Map a code between SNOMED CT, ICD-10, OPCS-4, CTV3, and Read v2 using the same maps as [`sct map`](map.md). `$translate` is advertised in `/metadata` only when the loaded SQLite database has the `crossmaps` table. `sct trud download --multi-terminology` builds the full map set; manually, ICD-10 / OPCS-4 need [`sct ndjson --refsets all`](ndjson.md), and Read v2 needs [`sct read2 import`](read2.md) over TRUD item 9.

```bash
# SNOMED CT -> ICD-10
curl 'http://localhost:8080/ConceptMap/$translate?system=http://snomed.info/sct&code=22298006&targetsystem=http://hl7.org/fhir/sid/icd-10'

# Bare names also accepted; reverse works too (ICD-10 -> SNOMED CT).
# ICD-10 input is tolerant of the undotted form (I219 as well as I21.9).
curl 'http://localhost:8080/ConceptMap/$translate?system=icd10&code=I219&targetsystem=snomed'
```

Returns a `Parameters` resource with `result` (boolean) and a `match` part per mapping. This is a drop-in target for the existing **DMWB Excel add-in**, which can point at a FHIR server - giving analysts the familiar worksheet workflow on a fast, offline backend.

### Examples

```bash
sct serve --db snomed.db &

curl 'http://localhost:8080/metadata'
curl 'http://localhost:8080/CodeSystem/$lookup?code=22298006&property=parent&property=designation'
curl 'http://localhost:8080/CodeSystem/$validate-code?code=22298006'
curl 'http://localhost:8080/CodeSystem/$subsumes?codeA=46635009&codeB=73211009'
```

Errors are FHIR `OperationOutcome` resources with the appropriate status (`404` unknown code, `400` invalid parameter, `406` XML requested, `500` server error).

---

## Scope and limitations

This is **Phase 1**. Known boundaries (see [`spec/commands/serve.md`](https://github.com/pacharanero/sct/blob/main/spec/commands/serve.md) for the full picture):

- **Single edition / single version** per process - the server serves whatever is in `--db`; a `version` parameter is accepted and logged but not used for routing.
- **Stored ValueSets** come from `.codelist` files (read-only, served from `--codelists`); there is no write/CRUD API for ValueSets, and no stored `ConceptMap` resources. `$closure`, multi-version routing, and FHIR R5 are later phases.
- **`^` (refset) ECL** depends on refsets being loaded (`sct ndjson --refsets simple` + `sct sqlite`); **attribute refinement** depends on the schema-v4 `concept_relationships` table (rebuild with a current `sct`).
- **No auth / SMART on FHIR** - run it behind your own gateway if exposing it beyond localhost.
- **JSON only** - XML requests get a `406`.
