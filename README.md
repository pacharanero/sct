# sct

A local-first SNOMED CT toolchain. One binary, six subcommands — from raw RF2 release to SQL, Parquet, Markdown, and AI tool use.

```
RF2 Snapshot
    │
    ▼ sct ndjson
    │
canonical NDJSON artefact
    │
    ├── sct sqlite  ──▶ snomed.db        (SQL + FTS5, MCP backend)
    ├── sct parquet ──▶ snomed.parquet   (DuckDB / analytics)
    ├── sct markdown──▶ snomed-concepts/ (RAG / LLM file reading)
    └── sct embed   ──▶ snomed-embeddings.arrow  (semantic vector search)
                              │
                         sct mcp ──▶ stdio MCP server (Claude Desktop)
```

The NDJSON artefact at the centre is a stable, versionable, greppable file. All other outputs are derived from it and can be regenerated at any time.

---

## Why

SNOMED CT is distributed as RF2 — a set of tab-separated files that require joining across multiple tables to get anything useful. The entire healthcare industry relies on remote terminology servers for this, with the overhead of network calls and REST APIs. `sct` performs the join once, deterministically, and produces standard files you can query locally with `sqlite3`, `duckdb`, `jq`, `ripgrep`, or an LLM.

---

## Quick start

```bash
# 1. Install
cargo install --path sct

# 2. Download SNOMED CT (UK users: https://isd.digital.nhs.uk/ → Monolith Edition, RF2: Snapshot)

# 3. Convert RF2 → NDJSON (~10s for 831k concepts)
sct ndjson --rf2 ~/.downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/

# 4. Load into SQLite with FTS5
sct sqlite --input snomedct-monolithrf2-production-20260311t120000z.ndjson

# 5. Query with standard tools — no custom binary needed
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 5"

# 6. Start the MCP server for Claude Desktop
sct mcp --db snomed.db
```

---

## Subcommands

- [sct ndjson](docs/ndjson.md) — convert an RF2 Snapshot directory to a canonical NDJSON artefact
- [sct sqlite](docs/sqlite.md) — load NDJSON into a SQLite database with FTS5
- [sct parquet](docs/parquet.md) — export NDJSON to a Parquet file for DuckDB / analytics
- [sct markdown](docs/markdown.md) — export NDJSON to per-concept Markdown files (or per-hierarchy with `--mode hierarchy`)
- [sct mcp](docs/mcp.md) — start a local MCP server over stdio backed by the SQLite database
- [sct embed](docs/embed.md) — generate Ollama vector embeddings and write an Arrow IPC file
- [sct lexical](docs/lexical.md) — keyword (FTS5) search over the SQLite database
- [sct semantic](docs/semantic.md) — semantic similarity search over the Arrow IPC embeddings file (requires Ollama)
- `sct info <file>` — inspect any `.ndjson`, `.db`, or `.arrow` artefact and print a summary
- `sct diff --old <file> --new <file>` — compare two NDJSON releases and report what changed

Run any subcommand with `--help` for full option reference.

---

## Installation

Requires Rust stable 1.70+: [rustup.rs](https://rustup.rs)

```bash
git clone https://github.com/pacharanero/sct
cd sct
cargo install --path sct
```

Or build without installing:

```bash
cargo build --release --manifest-path sct/Cargo.toml
# Binary: sct/target/release/sct
```

Pre-built binaries for Linux x86_64, macOS arm64, and macOS x86_64 are available on the [Releases page](https://github.com/pacharanero/sct/releases).

---

## Getting SNOMED CT

SNOMED CT is licensed. Download the RF2 Snapshot for your region:

- **UK:** [NHS Digital TRUD](https://isd.digital.nhs.uk/) → *SNOMED CT Monolith Edition, RF2: Snapshot*. Covered by the NHS England national licence.
- **International:** [MLDS](https://mlds.ihtsdotools.org/) or [NLM](https://www.nlm.nih.gov/healthit/snomedct/us_edition.html).

Download the **Monolith Snapshot** if available — it bundles the international base, clinical extension, and drug extension in one directory.

---

## See also

- [spec.md](spec.md) — technical specification for all layers
- [roadmap.md](roadmap.md) — implementation progress
- [BENCHMARKS.md](BENCHMARKS.md) — timing measurements
