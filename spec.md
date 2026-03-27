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
│        Vector Embeddings (LanceDB)          │  <- Layer 3: semantic search
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

The build tool (`snomed-build`) reads an RF2 snapshot directory and produces a single `.ndjson` file where each line is a self-contained JSON object representing one active concept.

### Build command

```bash
snomed-build --rf2 ./SnomedCT_InternationalRF2_PRODUCTION_20250101/ \
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
  }
}
```

### Properties of the artefact

- One line per active concept (inactive concepts omitted by default, includable with `--include-inactive`)
- Stable ordering by concept ID
- Locale-aware preferred terms (configurable; defaults to `en-GB` for UK SNOMED edition)
- Self-contained: no external references needed to interpret a line
- Human-readable and machine-readable
- Greppable with standard tools: `grep "22298006" snomed.ndjson`

### Determinism guarantee

Given the same RF2 snapshot directory and the same locale flag, `snomed-build` always produces byte-for-byte identical output. This means the artefact can be checksummed, versioned alongside code, and used in reproducible pipelines.

---

## Layer 2a - SQLite + FTS5

The SQLite consumer reads the NDJSON artefact and loads it into a single `snomed.db` SQLite file with full-text search.

```bash
snomed-sqlite --input snomed-20250101.ndjson --output snomed.db
```

### Schema

```sql
CREATE TABLE concepts (
    id          TEXT PRIMARY KEY,
    fsn         TEXT NOT NULL,
    preferred_term TEXT NOT NULL,
    synonyms    TEXT,           -- JSON array
    hierarchy   TEXT,
    hierarchy_path TEXT,        -- JSON array
    parents     TEXT,           -- JSON array of {id, fsn}
    attributes  TEXT,           -- JSON object
    active      INTEGER,
    effective_time TEXT
);

CREATE VIRTUAL TABLE concepts_fts USING fts5(
    id,
    preferred_term,
    synonyms,
    fsn,
    content='concepts',
    content_rowid='rowid'
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

An alternative consumer produces a Parquet file, directly queryable by DuckDB without any import step.

```bash
snomed-parquet --input snomed-20250101.ndjson --output snomed-20250101.parquet
```

This enables columnar analytics over SNOMED content:

```bash
duckdb -c "SELECT hierarchy, COUNT(*) as concept_count FROM 'snomed-20250101.parquet' GROUP BY hierarchy ORDER BY concept_count DESC"
```

DuckDB's FTS extension can be applied on top of the Parquet file for free-text search. The Parquet format is well-suited to integration with data science tooling (Python/pandas, R, Polars) without requiring a running service.

---

## Layer 2c - Flat Markdown Files

A third consumer produces a directory of per-concept Markdown files, one per concept, named by SCTID.

```bash
snomed-markdown --input snomed-20250101.ndjson --output ./snomed-concepts/
```

Output structure:

```
snomed-concepts/
  clinical-finding/
    22298006.md
    ...
  procedure/
    ...
  pharmaceutical/
    ...
```

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

An optional consumer takes the NDJSON artefact and produces a local vector index using LanceDB (Rust-native, no server required).

```bash
snomed-embed --input snomed-20250101.ndjson \
             --model nomic-embed-text \
             --output snomed-20250101.lance
```

Each concept is embedded as: `"{preferred_term}. {fsn}. Synonyms: {synonyms joined}. Hierarchy: {path joined}"`.

This enables semantic search over SNOMED - finding concepts similar in meaning to a query even where term matching would fail. The Lance index is a directory of files on disk; no vector database server is required.

---

## Layer 4 - Rust MCP Server

The outermost layer is a small Rust binary that wraps the SQLite database (Layer 2a) and exposes it as a local MCP (Model Context Protocol) server over stdio.

```bash
snomed-mcp --db snomed.db
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
      "command": "snomed-mcp",
      "args": ["--db", "/path/to/snomed.db"]
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

---

## Build Pipeline Summary

```
RF2 Snapshot
    │
    ▼
snomed-build          ← deterministic transform, run once per release
    │
    ▼
snomed-YYYYMMDD.ndjson   ← the canonical artefact; everything else is derived
    │
    ├──▶ snomed-sqlite   → snomed.db          (SQL + FTS5)
    ├──▶ snomed-parquet  → snomed.parquet     (DuckDB / analytics)
    ├──▶ snomed-markdown → snomed-concepts/   (RAG / LLM file reading)
    └──▶ snomed-embed    → snomed.lance       (semantic vector search)
                                │
                                └──▶ snomed-mcp → stdio MCP server (wraps SQLite)
```

---

## Implementation Notes

- `snomed-build` is the critical path component; correctness matters more than speed here, so Python or Rust are both reasonable
- `snomed-sqlite`, `snomed-parquet`, `snomed-markdown` are simple streaming NDJSON consumers; any language works
- `snomed-mcp` should be Rust for distribution simplicity (single static binary, `cargo install`)
- All tools should accept `--help`, produce useful errors, and exit cleanly
- The NDJSON artefact format is a public interface and should be versioned with a schema version field

---

## UK-Specific Notes

The UK SNOMED CT Clinical Edition (available from NHS Digital TRUD) includes:

- The SNOMED International release
- UK clinical extension
- dm+d (Dictionary of Medicines and Devices) drug extension

`snomed-build` should support layering multiple RF2 snapshots (base + extension) to produce a unified UK edition artefact. The `--locale en-GB` flag selects GB English preferred terms from the UK language reference set.

TRUD API key support for automated downloads is a future consideration.
