# sct

A local-first SNOMED CT toolchain that's 10-100x faster than IHTSDO Snowstorm. One binary — from raw RF2 release to NDJSON, then SQL, Parquet, Markdown, TUI, GUI, graphs and MCP/LLM tool use. All on your machine, no network calls, REST APIs, or external servers required.

This is very much a work in progress, but it's ready to use and I would very much like feedback on how it performs for you.

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
    ├── sct markdown──▶ snomed-concepts/ (RAG / LLM file reading) (untested)
    └── sct embed   ──▶ snomed-embeddings.arrow  (semantic vector search)
                              │
                         sct semantic ──▶ cosine similarity search (requires Ollama)

sct info  <file>              inspect any artefact
sct diff  --old <f> --new <f> compare two NDJSON releases (untested)
sct gui                       browser-based UI served over localhost
                              with graph visualisation and point-and-click exploration.
sct tui                       experimental terminal UI to explore concepts and relationships.
sct completions <shell>       generate shell completions (optional)
```

The NDJSON artefact at the centre is a stable, versionable, greppable file. All other outputs are derived from it and can be regenerated at any time.

---

## Why is this needed?

`sct` joins the relatively incomprehensible RF2 files into a single NDJSON artefact. For the UK Monolith Edition this NDJSON file is over 1Gb but it was still possible to load into VSCode to get a feel for the data structure, which is something that is impossible with the original RF2 files. This also means you can use standard tools like `jq` or `ripgrep` to query the data without needing a custom server or API.

SNOMED CT is distributed as RF2 — a set of tab-separated files that require joining across multiple tables to get anything useful. The entire healthcare industry relies on remote terminology servers for this, with the overhead of network calls and REST APIs. `sct` performs the join once creating an NDJSON artefact, and produces standard files you can query locally with `sqlite3`, `duckdb`, `jq`, `ripgrep`, or an LLM. No server, no API key, no network.

## Speed comparison

| Operation | `sct` + SQLite | Snowstorm Lite | `sct` speedup |
| --- | --- | --- | --- |
| Import - Clinical Edition | 22s | 209s | ~10x faster |
| Import - Full UK Monolith | ~57s | Failed (OOM)* | ∞ |
| Single concept lookup (SCTID) | 6ms | 491ms | ~80x faster |
| Free-text search (10 results) | 2ms | 202ms | ~100x faster |

> * Snowstorm Lite running in Docker with 24Gb of Java heap allocation ran out of memory on the full UK Monolith, which has 831k concepts. `sct` handled it in under a minute.

For more detailed benchmarks, see [docs/benchmarks.md](docs/benchmarks.md). Feel free to run the benchmarks yourself and share your results, perhaps as an Issue.

---

## Quick start

```bash
# 1. Clone the repository
git clone https://github.com/pacharanero/sct

# 2. Install
cargo install --path .

# 3. Download a distribution of SNOMED CT
#    UK:            https://isd.digital.nhs.uk/ → Monolith Edition, RF2: Snapshot
#                   (free under NHS England national licence — access is immediate)
#                   NB: You need to Subscribe to a release before you can see the Download option 🤯
#    International: https://mlds.ihtsdotools.org/ (allow up to a week for approval)

# 4. Convert RF2 → NDJSON (~10s for 831k concepts)
#    Pass the .zip directly — no manual extraction needed
sct ndjson --rf2 SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z.zip
# ✓  831,487 concepts written → snomedct-monolithrf2-production-20260311t120000z.ndjson

# 4. Load into SQLite with FTS5
sct sqlite --input snomedct-monolithrf2-production-20260311t120000z.ndjson

# 5. Query with standard tools — no custom binary needed
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 5"

# 6. Start the MCP server for Claude Desktop
sct mcp --db snomed.db
```

## Documentation

For all further information see the full documentation by either exploring the [docs/](docs/) directory or running the docs site locally with `s/docs`, or visit the docs on the GitHub Pages site: <https://pacharanero.github.io/sct/>

---

## Subcommands

* [sct ndjson](docs/ndjson.md) — convert an RF2 Snapshot directory to a canonical NDJSON artefact
* [sct sqlite](docs/sqlite.md) — load NDJSON into a SQLite database with FTS5
* [sct parquet](docs/parquet.md) — export NDJSON to a Parquet file for DuckDB / analytics
* [sct markdown](docs/markdown.md) — export NDJSON to per-concept Markdown files (or per-hierarchy with `--mode hierarchy`)
* [sct mcp](docs/mcp.md) — start a local MCP server over stdio backed by the SQLite database
* [sct embed](docs/embed.md) — generate Ollama vector embeddings and write an Arrow IPC file
* [sct lexical](docs/lexical.md) — keyword (FTS5) search over the SQLite database
* [sct semantic](docs/semantic.md) — semantic similarity search over the Arrow IPC embeddings file (requires Ollama)
* `sct info <file>` — inspect any `.ndjson`, `.db`, or `.arrow` artefact and print a summary
* `sct diff --old <file> --new <file>` — compare two NDJSON releases and report what changed
* [sct completions](docs/completions.md) — print shell completion scripts (bash, zsh, fish, powershell, elvish)
* [sct tui](docs/tui.md) — keyboard-driven terminal UI for interactive SNOMED CT exploration *(optional feature)*
* [sct gui](docs/gui.md) — browser-based UI served over localhost for point-and-click exploration *(optional feature)*

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
cargo install --path .
```

This installs the default binary (all subcommands except `tui` and `gui`). To include the optional interactive interfaces:

```bash
# Terminal UI (adds ratatui + crossterm)
cargo install --path . --features tui

# Browser UI (adds axum + tokio)
cargo install --path . --features gui

# Both
cargo install --path . --features full
```

Or build without installing:

```bash
cargo build --release
# Binary: target/release/sct
```

Pre-built binaries for Linux x86_64, macOS arm64, and macOS x86_64 are available on the [Releases page](https://github.com/pacharanero/sct/releases).

---

## Getting SNOMED CT

SNOMED CT is licensed. Download the RF2 Snapshot for your region:

* **UK:** [NHS Digital TRUD](https://isd.digital.nhs.uk/) → *SNOMED CT Monolith Edition, RF2: Snapshot*. Covered by the NHS England national licence.
* **International:** [MLDS](https://mlds.ihtsdotools.org/) or [NLM](https://www.nlm.nih.gov/healthit/snomedct/us_edition.html).

Download the **Monolith Snapshot** if available — it bundles the international base, clinical extension, and drug extension in one directory.

---

## Feedback

Please try it out and let me know how it performs for you, especially if you have a use case that isn't well supported by the current subcommands. Open an [Issue](https://github.com/pacharanero/sct/issues) for anything you want to report, from bugs to feature requests to general feedback.

## Development

A [devcontainer](https://containers.dev/) configuration is included in `.devcontainer/`. Open the project in VS Code and select "Reopen in Container" to get a ready-to-go environment with Rust, `sqlite3`, `duckdb`, `jq`, and `ripgrep` pre-installed.

Store SNOMED data files (zips, NDJSON, databases) in the `data-volume/` directory inside the container — it's backed by a Docker volume for faster I/O than the default bind mount.

## Contributing

Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines on how to contribute, report issues, or request features.

## Roadmap

See the [ROADMAP](specs/roadmap.md) for planned features, improvements, and long-term vision for the project.

## Trademarks and Copyright

### SNOMED CT®

SNOMED CT® is a registered trademark of SNOMED International. This project is an independent implementation and is not affiliated with SNOMED International. All SNOMED CT data is sourced from the official RF2 releases and remains copyright of SNOMED International. Please refer to the license terms for your use of SNOMED CT data. You must ensure you have an appropriate license to use SNOMED CT data in your jurisdiction.

### `sct`

`sct` is not trademarked. The source code and binaries are copyright Marcus Baw and Baw Medical Ltd, and provided to you under the terms of the AGPL-3.0 license.
