# SNOMED Local-First Tooling — Architecture Overview

## Overview

This project provides a layered, local-first toolchain for working with SNOMED CT clinical
terminology. The design follows a strict separation between:

1. A deterministic **build stage** that transforms RF2 release files into a canonical
   intermediate artefact
2. A set of independent **consumer tools** that express that artefact in different forms for
   different use cases

The philosophy is "convention over configuration" and "data over services". SNOMED CT is a
dataset. It should be possible to work with it like any other dataset — from the command line,
from a script, from an LLM tool, without running a server.

---

## Design Principles

- **Offline-first** — no network dependency at query time
- **Deterministic** — the same RF2 input always produces the same artefact
- **Single-file portability** — the core artefact is a single file you can copy, version, and share
- **Standard tooling** — queryable with `sqlite3`, `duckdb`, `ripgrep`, `jq` without any custom binary
- **Layered** — each layer is independently useful; you do not need the outer layers to use the inner ones
- **LLM-native** — outputs are designed for direct consumption by language models and AI tooling

---

## The Onion Model

```
┌─────────────────────────────────────────────┐
│           MCP Server (Rust binary)          │  ← Layer 4: AI tool use
├─────────────────────────────────────────────┤
│     Vector Embeddings (Arrow IPC / Ollama)  │  ← Layer 3: semantic search
├─────────────────────────────────────────────┤
│      SQLite + FTS5  /  DuckDB Parquet       │  ← Layer 2: structured query
├─────────────────────────────────────────────┤
│         Canonical NDJSON artefact           │  ← Layer 1: the core artefact
├─────────────────────────────────────────────┤
│           RF2 Snapshot (input)              │  ← Source: SNOMED release
└─────────────────────────────────────────────┘
```

Each layer consumes the layer below it. The NDJSON artefact at Layer 1 is the stable interface
between the build stage and all consumer tools.

---

## Layer 0 — Input: RF2 Snapshot

SNOMED CT is distributed as RF2 (Release Format 2), a set of tab-separated files:

- `sct2_Concept_Snapshot_*.txt` — concept identifiers and status
- `sct2_Description_Snapshot_*.txt` — human-readable terms and synonyms
- `sct2_Relationship_Snapshot_*.txt` — IS-A and attribute relationships
- `der2_cRefset_Language_*.txt` — language reference sets (preferred terms by locale)

RF2 is relational. To get anything useful you must join across multiple files. This is the join
that Layer 1 performs, once, repeatably.

---

## Build Pipeline

```
RF2 Snapshot
    │
    ▼
sct ndjson            ← deterministic transform, run once per release
    │
    ▼
snomed-YYYYMMDD.ndjson   ← canonical artefact; everything else is derived
    │
    ├──▶ sct sqlite   → snomed.db                (SQL + FTS5)
    ├──▶ sct parquet  → snomed.parquet            (DuckDB / analytics)
    ├──▶ sct markdown → snomed-concepts/          (RAG / LLM file reading)
    └──▶ sct embed    → snomed-embeddings.arrow   (semantic vector search)
                                │
                          sct mcp → stdio MCP server (wraps SQLite)
```

---

## Command index

Each command has its own spec in `specs/commands/`:

| Command | Layer | Description |
|---|---|---|
| [`sct ndjson`](commands/ndjson.md) | 1 | RF2 → canonical NDJSON artefact |
| [`sct sqlite`](commands/sqlite.md) | 2a | NDJSON → SQLite + FTS5 database |
| [`sct parquet`](commands/parquet.md) | 2b | NDJSON → Apache Parquet file |
| [`sct markdown`](commands/markdown.md) | 2c | NDJSON → flat Markdown files |
| [`sct embed`](commands/embed.md) | 3 | NDJSON → Arrow IPC embeddings (Ollama) |
| [`sct mcp`](commands/mcp.md) | 4 | Stdio MCP server over SQLite |
| [`sct diff`](commands/diff.md) | — | Compare two NDJSON artefacts |
| [`sct info`](commands/info.md) | — | Inspect any `sct`-produced file |
| [`sct lexical`](commands/lexical.md) | — | FTS5 keyword search |
| [`sct semantic`](commands/semantic.md) | — | Cosine similarity search (Ollama) |
| [`sct tui`](commands/tui.md) | — | Keyboard-driven terminal UI |
| [`sct gui`](commands/gui.md) | — | Browser-based UI |
| [`sct completions`](commands/completions.md) | — | Shell completion scripts |
| [`sct codelist`](commands/codelist.md) | — | Build, validate, and publish code lists |
| [`sct serve`](commands/serve.md) | 5 | FHIR R4 HTTP terminology server (planned) |

See [`specs/bench.md`](bench.md) for the `bench/` benchmarking script suite.

---

## Implementation notes

- All subcommands compile into a single `sct` binary (`cargo install sct`)
- `sct ndjson` is the critical-path component; correctness matters more than speed
- `sct sqlite`, `sct parquet`, `sct markdown` are streaming NDJSON consumers with progress bars
- `sct mcp` is read-only and stateless; opens SQLite on startup, serves until stdin EOF
- `sct embed` requires an external Ollama process; all other subcommands are fully offline
- All subcommands accept `--help`, produce useful errors, and exit cleanly
- The NDJSON artefact format is a public interface versioned with `schema_version`; currently version `1`

---

## Documentation maintenance

`docs/walkthrough.md` is the primary user-facing feature tour. It should be kept in sync
with the implementation. When making changes to this project, update `docs/walkthrough.md`
if any of the following change:

- A new command is added or an existing one is removed
- Command flags or behaviour change in a user-visible way
- Timing or performance figures change significantly
- A planned feature moves from roadmap to implemented (remove the *(planned)* tag)
- A new layer or output format is introduced

The walkthrough is also the source material for the Remotion demo — each top-level section
(prefixed `## N —`) corresponds to a demo scene. Keep section headings stable.

---

## UK-specific notes

The UK SNOMED CT Clinical Edition (available from NHS TRUD) includes:

- The SNOMED International release
- UK clinical extension
- dm+d (Dictionary of Medicines and Devices) drug extension

`sct ndjson` supports layering multiple RF2 snapshots via multiple `--rf2` flags to produce a
unified UK edition artefact. The `--locale en-GB` flag selects GB English preferred terms from
the UK language reference set.

## TODO

- static security analysis setup
- Transitive closure table example/investigate
