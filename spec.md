# SNOMED Local-First Tooling - Technical Specification

## Overview

This project provides a layered, local-first toolchain for working with SNOMED CT clinical terminology. The design follows a strict separation between:

1. A deterministic **build stage** that transforms RF2 release files into a canonical intermediate artefact
2. A set of independent **consumer tools** that express that artefact in different forms for different use cases

The philosophy is "convention over configuration" and "data over services". SNOMED CT is a dataset. It should be possible to work with it like any other dataset - from the command line, from a script, from an LLM tool, without running a server.

---

## Design Principles

- **Offline-first** - no network dependency at query time
- **Deterministic** - the same RF2 input always produces the same artefact
- **Single-file portability** - the core artefact is a single file you can copy, version, and share
- **Standard tooling** - queryable with `sqlite3`, `duckdb`, `ripgrep`, `jq` and similar without any custom binary
- **Layered** - each layer is independently useful; you do not need the outer layers to use the inner ones
- **LLM-native** - outputs are designed for direct consumption by language models and AI tooling

---

## The Onion Model

```
┌─────────────────────────────────────────────┐
│           MCP Server (Rust binary)          │  <- Layer 4: AI tool use
├─────────────────────────────────────────────┤
│     Vector Embeddings (Arrow IPC / Ollama)  │  <- Layer 3: semantic search
├─────────────────────────────────────────────┤
│      SQLite + FTS5  /  DuckDB Parquet       │  <- Layer 2: structured query
├─────────────────────────────────────────────┤
│         Canonical NDJSON artefact           │  <- Layer 1: the core artefact
├─────────────────────────────────────────────┤
│           RF2 Snapshot (input)              │  <- Source: SNOMED release
└─────────────────────────────────────────────┘
```

Each layer consumes the layer below it. The NDJSON artefact at Layer 1 is the stable interface between the build stage and all consumer tools.

---

## Layer 0 - Input: RF2 Snapshot

SNOMED CT is distributed as RF2 (Release Format 2), a set of tab-separated files covering:

- `sct2_Concept_Snapshot_*.txt` - concept identifiers and status
- `sct2_Description_Snapshot_*.txt` - human-readable terms and synonyms
- `sct2_Relationship_Snapshot_*.txt` - IS-A and attribute relationships
- `der2_cRefset_Language_*.txt` - language reference sets (preferred terms by locale)

RF2 is relational. To get anything useful from it you must join across multiple files. This is the join that Layer 1 performs, once, repeatably.

---

## Layer 1 - The Canonical Artefact: NDJSON

`sct ndjson` reads an RF2 snapshot directory and produces a single `.ndjson` file where each line is a self-contained JSON object representing one active concept.

### Build command

```bash
sct ndjson --rf2 ./SnomedCT_InternationalRF2_PRODUCTION_20250101/ \
           --locale en-GB \
           --output snomed-20250101.ndjson
```

### Per-concept JSON schema

```json
{
  "id": "22298006",
  "fsn": "Myocardial infarction (disorder)",
  "preferred_term": "Heart attack",
  "synonyms": ["Cardiac infarction", "Infarction of heart", "MI - Myocardial infarction"],
  "hierarchy": "Clinical finding",
  "hierarchy_path": ["SNOMED CT concept", "Clinical finding", "Disorder of cardiovascular system", "Ischemic heart disease", "Myocardial infarction"],
  "parents": [{"id": "414795007", "fsn": "Ischemic heart disease (disorder)"}],
  "children_count": 47,
  "active": true,
  "module": "900000000000207008",
  "effective_time": "20020131",
  "attributes": {
    "finding_site": [{"id": "302509004", "fsn": "Entire heart (body structure)"}],
    "associated_morphology": [{"id": "55641003", "fsn": "Infarct (morphologic abnormality)"}]
  },
  "schema_version": 1
}
```

### Properties of the artefact

- One line per active concept (inactive concepts omitted by default, includable with `--include-inactive`)
- Stable ordering by concept ID
- Locale-aware preferred terms (configurable; defaults to `en-GB` for UK SNOMED edition)
- Self-contained: no external references needed to interpret a line
- Human-readable and machine-readable
- Greppable with standard tools: `grep "22298006" snomed.ndjson`

### Schema versioning

Every record includes a `schema_version` integer field. Consumers use this to detect incompatible format changes. The current version is `1`. Consumers that encounter a version they do not recognise should warn or refuse to start rather than silently misinterpreting data.

### Determinism guarantee

Given the same RF2 snapshot directory and the same locale flag, `sct ndjson` always produces byte-for-byte identical output. This means the artefact can be checksummed, versioned alongside code, and used in reproducible pipelines.

---

## Layer 2a - SQLite + FTS5

`sct sqlite` reads the NDJSON artefact and loads it into a single `snomed.db` SQLite file with full-text search.

```bash
sct sqlite --input snomed-20250101.ndjson --output snomed.db
```

### Schema

```sql
CREATE TABLE concepts (
    id              TEXT PRIMARY KEY,
    fsn             TEXT NOT NULL,
    preferred_term  TEXT NOT NULL,
    synonyms        TEXT,    -- JSON array
    hierarchy       TEXT,
    hierarchy_path  TEXT,    -- JSON array
    parents         TEXT,    -- JSON array of {id, fsn}
    children_count  INTEGER,
    attributes      TEXT,    -- JSON object
    active          INTEGER,
    module          TEXT,
    effective_time  TEXT,
    schema_version  INTEGER
);

CREATE VIRTUAL TABLE concepts_fts USING fts5(
    id,
    preferred_term,
    synonyms,
    fsn,
    content='concepts',
    content_rowid='rowid'
);

-- Fast IS-A traversal without JSON parsing
CREATE TABLE concept_isa (
    child_id  TEXT NOT NULL,
    parent_id TEXT NOT NULL
);
```

### Example queries

```bash
# Free-text search from the CLI - no binary required beyond sqlite3
sqlite3 snomed.db "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 10"

# Exact concept lookup
sqlite3 snomed.db "SELECT json(attributes) FROM concepts WHERE id = '22298006'"

# All children of a hierarchy
sqlite3 snomed.db "SELECT id, preferred_term FROM concepts WHERE hierarchy = 'Procedure' LIMIT 20"
```

The resulting `snomed.db` is a single portable file. It can be committed to git-lfs, attached to a release, or `scp`'d to another machine.

---

## Layer 2b - DuckDB / Parquet

`sct parquet` produces a Parquet file, directly queryable by DuckDB without any import step.

```bash
sct parquet --input snomed-20250101.ndjson --output snomed-20250101.parquet
```

This enables columnar analytics over SNOMED content:

```bash
duckdb -c "SELECT hierarchy, COUNT(*) as concept_count FROM 'snomed-20250101.parquet' GROUP BY hierarchy ORDER BY concept_count DESC"
```

DuckDB's FTS extension can be applied on top of the Parquet file for free-text search. The Parquet format is well-suited to integration with data science tooling (Python/pandas, R, Polars) without requiring a running service.

---

## Layer 2c - Flat Markdown Files

`sct markdown` produces Markdown output from the NDJSON artefact in one of two modes:

```bash
# One file per concept (default)
sct markdown --input snomed-20250101.ndjson --output ./snomed-concepts/

# One file per top-level hierarchy
sct markdown --input snomed-20250101.ndjson --output ./snomed-concepts/ --mode hierarchy
```

### `--mode concept` (default)

One `.md` file per concept, named by SCTID and organised into subdirectories by hierarchy:

```
snomed-concepts/
  clinical-finding/
    22298006.md
    ...
  procedure/
    ...
```

### `--mode hierarchy`

One `.md` file per top-level hierarchy (~19 files), each containing all concepts in that hierarchy as H2 sections. Useful for bulk LLM ingestion where all related concepts should share context.

```
snomed-concepts/
  clinical-finding.md
  procedure.md
  ...
```

### Per-concept file format

Each file is human and LLM-readable:

```markdown
# Heart attack
**SCTID:** 22298006
**FSN:** Myocardial infarction (disorder)
**Hierarchy:** Clinical finding > Disorder of cardiovascular system > Ischemic heart disease

## Synonyms
- Cardiac infarction
- Infarction of heart
- MI - Myocardial infarction

## Relationships
- **Finding site:** Entire heart (body structure) [302509004]
- **Associated morphology:** Infarct [55641003]

## Hierarchy
- SNOMED CT concept
  - Clinical finding
    - Disorder of cardiovascular system
      - Ischemic heart disease
        - **Myocardial infarction** (this concept)
```

This layer is specifically designed for RAG (retrieval-augmented generation) indexing and for direct LLM file reading via tools like Claude Code or the filesystem MCP.

---

## Layer 3 - Vector Embeddings

`sct embed` takes the NDJSON artefact and produces an Apache Arrow IPC file containing one embedding per concept. Embeddings are generated via a locally-running [Ollama](https://ollama.com) instance — no bundled model, no external API key required.

```bash
sct embed --input snomed-20250101.ndjson \
          --model nomic-embed-text \
          --output snomed-embeddings.arrow
```

Each concept is embedded as: `"{preferred_term}. {fsn}. Synonyms: {synonyms joined}. Hierarchy: {path joined}"`.

The Arrow IPC file has columns `id`, `preferred_term`, `hierarchy`, and `embedding` (FixedSizeList<Float32>). It can be queried directly in DuckDB, loaded into Python via PyArrow, or imported into LanceDB or any Arrow-compatible vector store. No vector database server is required at query time.

### Prerequisites

```bash
ollama pull nomic-embed-text
ollama serve
```

If Ollama is not reachable, `sct embed` exits with a clear error and instructions.

---

## Layer 4 - Rust MCP Server

The outermost layer is a subcommand of `sct` that wraps the SQLite database (Layer 2a) and exposes it as a local MCP (Model Context Protocol) server over stdio.

```bash
sct mcp --db snomed.db
```

### MCP tools exposed

| Tool | Description |
|---|---|
| `snomed_search` | Free-text search returning concept ID, preferred term, FSN, hierarchy |
| `snomed_concept` | Full concept detail by SCTID |
| `snomed_children` | Immediate children of a concept |
| `snomed_ancestors` | Full ancestor chain up to root |
| `snomed_hierarchy` | List all concepts in a named top-level hierarchy |

### Claude Desktop config

```json
{
  "mcpServers": {
    "snomed": {
      "command": "sct",
      "args": ["mcp", "--db", "/path/to/snomed.db"]
    }
  }
}
```

### Design constraints

- Single binary, no runtime dependencies
- Reads SQLite via `rusqlite` (statically linked)
- Stdio transport only - no HTTP, no TLS, no port management
- Starts in under 100ms
- Read-only
- Validates `schema_version` on startup: warns if the database is newer than the binary, refuses to start if the version gap is too large (> 5 versions)

---

## Build Pipeline Summary

```
RF2 Snapshot
    │
    ▼
sct ndjson            ← deterministic transform, run once per release
    │
    ▼
snomed-YYYYMMDD.ndjson   ← the canonical artefact; everything else is derived
    │
    ├──▶ sct sqlite   → snomed.db                (SQL + FTS5)
    ├──▶ sct parquet  → snomed.parquet            (DuckDB / analytics)
    ├──▶ sct markdown → snomed-concepts/          (RAG / LLM file reading)
    └──▶ sct embed    → snomed-embeddings.arrow   (semantic vector search)
                                │
                          sct mcp → stdio MCP server (wraps SQLite)
```

---

## Implementation Notes

- All subcommands are compiled into a single `sct` binary (Rust, `cargo install`)
- `sct ndjson` is the critical path component; correctness matters more than speed
- `sct sqlite`, `sct parquet`, `sct markdown` are streaming NDJSON consumers with progress bars
- `sct mcp` is read-only and stateless; it opens the SQLite file on startup and serves until EOF on stdin
- `sct embed` requires an external Ollama process; all other subcommands are fully offline
- All subcommands accept `--help`, produce useful errors, and exit cleanly
- The NDJSON artefact format is a public interface versioned with `schema_version`; currently at version 1

---

## UK-Specific Notes

The UK SNOMED CT Clinical Edition (available from NHS Digital TRUD) includes:

- The SNOMED International release
- UK clinical extension
- dm+d (Dictionary of Medicines and Devices) drug extension

`sct ndjson` supports layering multiple RF2 snapshots (base + extension) via multiple `--rf2` flags to produce a unified UK edition artefact. The `--locale en-GB` flag selects GB English preferred terms from the UK language reference set.

TRUD API key support for automated downloads is a future consideration.

---

## Benchmarking Tooling

The benchmarking suite lives in `bench/` at the repository root. It is a set of Bash scripts requiring only `bash`, `curl`, `sqlite3`, `jq`, and `bc` — no Rust build required. [`hyperfine`](https://github.com/sharkdp/hyperfine) is an optional dependency that provides statistically-rigorous timing (median, mean, stddev over _N_ runs) and is used automatically when available.

### Purpose

To provide a reproducible, automated, fair comparison between `sct` (local SQLite) and any FHIR R4 terminology server. The suite is primarily useful for:

- Demonstrating the latency advantage of local-first tooling in talks and documentation
- Detecting regressions in `sct` query performance across releases
- Evaluating third-party terminology servers before adopting them in a workflow

### Design goals

- **Automatable** — runs headlessly, suitable for CI
- **Portable** — only POSIX tools + `curl` + `sqlite3` + `jq` required
- **Fair** — both sides answer the same semantic question; differences in result shape are noted but not penalised
- **Transparent** — prints every query issued on both sides; no black-box timing

### Repository layout

```
bench/
  bench.sh            ← entry point; orchestrates all operations
  lib/
    timing.sh         ← timing primitives (hyperfine wrapper + manual fallback)
    fhir.sh           ← curl wrappers for each FHIR operation
    local.sh          ← sqlite3 wrappers for each equivalent local operation
    report.sh         ← table/JSON/CSV rendering
  operations/
    lookup.sh         ← single-concept lookup by SCTID
    search.sh         ← free-text search (FTS5 vs ValueSet/$expand)
    children.sh       ← direct children of a concept
    ancestors.sh      ← full ancestor chain
    subsumption.sh    ← is concept A a subtype of concept B?
    bulk.sh           ← batch lookup of N concepts
  fixtures/
    concepts.txt      ← SCTIDs used as lookup/hierarchy fixtures
    search_terms.txt  ← free-text queries used for search fixtures
  README.md
```

### Entry point

```bash
bench/bench.sh [OPTIONS]

Options:
  --server URL          Base URL of FHIR terminology server (required for remote comparison)
                        e.g. https://terminology.openehr.org/fhir
  --db PATH             Path to snomed.db (default: ./snomed.db)
  --runs N              Number of timed iterations per operation (default: 10)
  --warmup N            Number of warmup runs before timing (default: 2)
  --operations LIST     Comma-separated subset to run: lookup,search,children,ancestors,
                        subsumption,bulk  (default: all)
  --format FORMAT       Output format: table (default), json, csv
  --no-remote           Benchmark local operations only (skip FHIR calls)
  --timeout SECS        Per-request timeout for remote calls (default: 30)
  --output FILE         Write report to file in addition to stdout
```

### Test fixtures

The suite ships with a fixed set of well-known, stable SCTIDs covering a range of hierarchies and concept depths. Fixtures are deliberately kept small (10–20 concepts) so the warm-up pass completes quickly.

**`bench/fixtures/concepts.txt`**

```
22298006    # Myocardial infarction (disorder)
73211009    # Diabetes mellitus (disorder)
195967001   # Asthma (disorder)
44054006    # Type 2 diabetes mellitus (disorder)
84114007    # Heart failure (disorder)
34000006    # Crohn's disease (disorder)
13645005    # Chronic obstructive lung disease (disorder)
230690007   # Cerebrovascular accident (disorder)
396275006   # Osteoarthritis (disorder)
49436004    # Atrial fibrillation (disorder)
80146002    # Appendectomy (procedure)
302509004   # Entire heart (body structure)
119292006   # Wound of trunk (disorder)
271737000   # Anaemia (disorder)
59282003    # Pulmonary embolism (disorder)
```

**`bench/fixtures/search_terms.txt`**

```
heart attack
diabetes type 2
asthma
blood pressure
fracture femur
hypertension
appendicitis
pulmonary embolism
renal failure
```

### Operations

Each operation script runs both the local and remote variants, handles timing, and emits a single-row result for the report aggregator.

#### 1. Concept lookup (`lookup.sh`)

Resolve a SCTID to its display name, FSN, and top-level hierarchy.

| Side | Implementation |
|---|---|
| Local | `SELECT id, preferred_term, fsn, hierarchy FROM concepts WHERE id = ?` |
| FHIR | `GET {base}/CodeSystem/$lookup?system=http://snomed.info/sct&code={SCTID}&property=display&property=designation` |

Fixture: all 15 SCTIDs in `concepts.txt`; report uses the median across all.

#### 2. Free-text search (`search.sh`)

Search for concepts matching a short phrase, returning the top 10 results.

| Side | Implementation |
|---|---|
| Local | `SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH ? LIMIT 10` |
| FHIR | `GET {base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs&filter={term}&count=10` |

Fixture: all 9 terms in `search_terms.txt`; report uses the median across all. Result counts from both sides are printed for spot-check inspection.

#### 3. Direct children (`children.sh`)

Retrieve all immediate children of a concept.

| Side | Implementation |
|---|---|
| Local | `SELECT c.id, c.preferred_term FROM concept_isa ci JOIN concepts c ON ci.child_id = c.id WHERE ci.parent_id = ?` |
| FHIR | `GET {base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl%2F<!{SCTID}&count=1000` (ECL `<!SCTID` = direct children) |

Fixture: fixed parent concept `73211009` (Diabetes mellitus — moderate fan-out, ~20 children) plus `195967001` (Asthma — larger fan-out).

#### 4. Ancestor chain (`ancestors.sh`)

Walk the full IS-A path from a concept up to the root.

| Side | Implementation |
|---|---|
| Local | Recursive CTE: `WITH RECURSIVE anc(id) AS (SELECT parent_id FROM concept_isa WHERE child_id = ? UNION ALL SELECT parent_id FROM concept_isa ci JOIN anc ON ci.child_id = anc.id) SELECT id, preferred_term FROM concepts WHERE id IN (SELECT id FROM anc)` |
| FHIR | Iterative `CodeSystem/$lookup` with `property=parent` until root, or `CodeSystem/$lookup` with `property=*` to retrieve `parent`/`ancestor` properties if the server supports `property=ancestor` |

Fixture: `44054006` (Type 2 diabetes — depth ~8); `230690007` (Cerebrovascular accident — depth ~7). The number of round-trips required on the FHIR side is noted in the report.

#### 5. Subsumption test (`subsumption.sh`)

Check whether concept A is subsumed by concept B (A is-a B).

| Side | Implementation |
|---|---|
| Local | CTE path query: check if `B` appears in the ancestor chain of `A` |
| FHIR | `GET {base}/CodeSystem/$subsumes?system=http://snomed.info/sct&codeA={A}&codeB={B}` |

Fixture: 5 pairs — 3 true subsumptions, 2 false. Both positive and negative cases are timed.

#### 6. Bulk lookup (`bulk.sh`)

Resolve 50 concepts in a single request/query.

| Side | Implementation |
|---|---|
| Local | `SELECT id, preferred_term, fsn FROM concepts WHERE id IN (id1, id2, ...)` — single query |
| FHIR | FHIR batch `POST {base}` with 50 `GET CodeSystem/$lookup` entries in a `Bundle` of type `batch`, falling back to 50 sequential requests if the server does not support batch |

This operation most clearly illustrates the per-query overhead of HTTP round-trips. The report flags whether batch mode was used on the FHIR side.

### Timing implementation

```bash
# lib/timing.sh

# If hyperfine is available, delegate to it for statistically rigorous timing.
# Output: median milliseconds as a plain number.
time_operation() {
  local label="$1"; shift   # human label
  local runs="$1"; shift    # --runs N
  local warmup="$1"; shift  # --warmup N
  # remaining args: the command to time

  if command -v hyperfine >/dev/null 2>&1; then
    hyperfine --runs "$runs" --warmup "$warmup" \
              --export-json /tmp/bench_result.json \
              "$@" >/dev/null 2>&1
    jq -r '.results[0].median * 1000 | floor' /tmp/bench_result.json
  else
    # Manual fallback: run $runs times, collect elapsed_ms, print median
    local times=()
    for _ in $(seq 1 "$((warmup + runs))"); do
      local start end_ns
      start=$(date +%s%N)
      "$@" >/dev/null 2>&1
      end_ns=$(date +%s%N)
      times+=( $(( (end_ns - start) / 1000000 )) )
    done
    # drop first $warmup values, compute median of remainder
    printf '%s\n' "${times[@]:$warmup}" | sort -n | awk 'BEGIN{c=0} {a[c++]=$1} END{print a[int(c/2)]}'
  fi
}
```

### Report format

Terminal output (default `--format table`):

```
sct benchmark — 2026-03-28
  Local DB : /home/marcus/snomed.db  (v20260101, 391 023 concepts)
  Remote   : https://terminology.openehr.org/fhir
  Runs     : 10 (+ 2 warmup)

┌──────────────────────────┬───────────────────┬───────────────────┬──────────────────┐
│ Operation                │ sct (local)       │ FHIR (remote)     │ Speedup          │
├──────────────────────────┼───────────────────┼───────────────────┼──────────────────┤
│ Concept lookup           │       1.2 ms      │      248 ms       │    206× faster   │
│ Text search (top 10)     │       3.8 ms      │      334 ms       │     88× faster   │
│ Direct children          │       2.3 ms      │      421 ms       │    183× faster   │
│ Ancestor chain           │       3.1 ms      │     1 840 ms      │    594× faster   │
│ Subsumption test         │       1.4 ms      │      209 ms       │    149× faster   │
│ Bulk lookup (50)         │       4.6 ms      │   12 450 ms  [1]  │  2 707× faster   │
├──────────────────────────┼───────────────────┼───────────────────┼──────────────────┤
│ TOTAL (sum)              │      16.4 ms      │    15 502 ms      │    945× faster   │
└──────────────────────────┴───────────────────┴───────────────────┴──────────────────┘

[1] Server does not support FHIR batch; 50 sequential requests issued.

Network latency to remote (median of 20 pings): 18 ms
Times shown are wall-clock median. Local times include sqlite3 process startup.
```

When `--format json` is specified, each row is emitted as a JSON object to stdout — suitable for downstream processing or CI artifact storage.

### Fairness notes

- **SQLite vs HTTP**: The primary cost difference is network round-trips. The report always states the measured ping latency so readers can distinguish "server is slow" from "network is slow."
- **sct includes process startup**: For the local side, each timed command is `sqlite3 snomed.db "..."` (or `sct lexical ...`), which includes process fork + open overhead. This is intentional — it reflects real-world CLI usage. The overhead is typically 5–15 ms and is noted in the report.
- **SNOMED version**: The suite prints the SNOMED effective date from the local DB and, where exposed by the server, the server's stated version. Users should compare results only when both sides reference the same content version.
- **Cache state**: The FHIR server may serve repeated requests from its own in-memory cache. Warm-up runs are issued before timing to put both sides in a hot-cache state. This is intentional — we are benchmarking the steady-state use case, not cold-start.
- **Network jitter**: All remote timings report the stddev alongside the median so readers can see whether the remote numbers are stable.

### Dependencies

| Tool | Required | Purpose |
|---|---|---|
| `bash` ≥ 4.0 | Yes | Script runtime |
| `curl` | Yes | FHIR HTTP calls |
| `sqlite3` | Yes | Local queries |
| `jq` | Yes | JSON parsing (FHIR responses + hyperfine output) |
| `bc` or `awk` | Yes | Arithmetic (manual timing fallback) |
| `hyperfine` | Recommended | Statistical timing (median, stddev, runs) |

`hyperfine` is available via `cargo install hyperfine`, `brew install hyperfine`, or most Linux package managers.

### Known limitations

- Ancestor chain via FHIR requires iterative `$lookup` calls on servers that do not support `property=ancestor` (not all do). The number of round-trips grows with concept depth and is noted in the report.
- Some operations have no direct FHIR equivalent (e.g. listing all concepts in a hierarchy by name). These are marked as `local only` in the output.
- The scripts assume the FHIR server exposes SNOMED CT at `http://snomed.info/sct`. Servers using a different CodeSystem URL will need the `--system` flag (future work).
