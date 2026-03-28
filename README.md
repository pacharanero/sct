# sct

A local-first SNOMED CT toolchain. One binary — from raw RF2 release to SQL, Parquet, Markdown, TUI, GUI and MCP/LLM tool use.

```
RF2 Snapshot
    │
    ▼ sct ndjson                                    (~10s for 831k concepts)
    │
canonical NDJSON artefact
    │
    ├── sct sqlite  ──▶ snomed.db        (SQL + FTS5, MCP backend)
    │       │
    │       ├── sct lexical  ──▶ keyword search (FTS5)
    │       └── sct mcp      ──▶ stdio MCP server (Claude Desktop / Claude Code)
    ├── sct parquet ──▶ snomed.parquet   (DuckDB / analytics)
    ├── sct markdown──▶ snomed-concepts/ (RAG / LLM file reading)
    └── sct embed   ──▶ snomed-embeddings.arrow  (semantic vector search)
                              │
                         sct semantic ──▶ cosine similarity search (requires Ollama)

sct info  <file>              inspect any artefact
sct diff  --old <f> --new <f> compare two NDJSON releases
sct completions <shell>       generate shell completions
```

The NDJSON artefact at the centre is a stable, versionable, greppable file. All other outputs are derived from it and can be regenerated at any time.

---

## Why

`sct` joins RF2 once — deterministically — and gives you standard files you query offline forever.

SNOMED CT is distributed as RF2 — a set of tab-separated files that require joining across multiple tables to get anything useful. The entire healthcare industry relies on remote terminology servers for this, with the overhead of network calls and REST APIs. `sct` performs the join once and produces standard files you can query locally with `sqlite3`, `duckdb`, `jq`, `ripgrep`, or an LLM. No server, no API key, no network.

---

## Quick start

```bash
# 1. Install
cargo install --path sct

# 2. Download SNOMED CT
#    UK:            https://isd.digital.nhs.uk/ → Monolith Edition, RF2: Snapshot
#                   (free under NHS England national licence — access is immediate)
#    International: https://mlds.ihtsdotools.org/ (allow up to a week for approval)

# 3. Convert RF2 → NDJSON (~10s for 831k concepts)
sct ndjson --rf2 ~/.downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/
# ✓  831,487 concepts written → snomedct-monolithrf2-production-20260311t120000z.ndjson

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
- [sct completions](docs/completions.md) — print shell completion scripts (bash, zsh, fish, powershell, elvish)
- [sct tui](docs/tui.md) — keyboard-driven terminal UI for interactive SNOMED CT exploration *(optional feature)*
- [sct gui](docs/gui.md) — browser-based UI served over localhost for point-and-click exploration *(optional feature)*

Run any subcommand with `--help` for full option reference.

---

## Which output do I want?

| Goal | Command |
|---|---|
| Query with SQL / keyword search | `sct sqlite` then `sct lexical` |
| Analytics / DuckDB | `sct parquet` |
| RAG / LLM file ingestion | `sct markdown` |
| Semantic / meaning-based search | `sct embed` then `sct semantic` |
| Claude Desktop or Claude Code | `sct sqlite` then `sct mcp` |

---

## Installation

Requires Rust stable 1.70+: [rustup.rs](https://rustup.rs)

```bash
git clone https://github.com/pacharanero/sct
cd sct
cargo install --path sct
```

This installs the default binary (all subcommands except `tui` and `gui`). To include the optional interactive interfaces:

```bash
# Terminal UI (adds ratatui + crossterm)
cargo install --path sct --features tui

# Browser UI (adds axum + tokio)
cargo install --path sct --features gui

# Both
cargo install --path sct --features full
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
