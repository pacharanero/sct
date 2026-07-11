# `sct` Walkthrough

A hands-on tour of the `sct` SNOMED-CT local-first toolchain.

`sct` is a single Rust binary that transforms a SNOMED CT RF2 release into a set of
queryable, offline-first artefacts. No server required. No bloody Java.

It was initially created as an experiment in file-based data handling, offline-first tooling, and learning about the structure of SNOMED, but it turns out it's pretty fast and useful too, so I'm gradually adding features with the aim of creating something genuinely useful for practitioners, informaticians, and researchers working with SNOMED CT.

!!! tip "⚡ The 30-second demo: search-as-you-type over 800k concepts"
    Build the lexical index once, then watch every SNOMED concept autocomplete **instantly and offline** as you type - sub-millisecond per keystroke, no server:

    ```bash
    sct fst build --ndjson snomed.ndjson --output snomed.fst
    sct sayt --index snomed.fst
    ```

    The same engine embeds into your own app two more ways - a `--stdio` line protocol for native apps and an HTTP `/autocomplete` endpoint on `sct serve` for the web. See [`sct sayt`](../commands/sayt.md).

| Guide | What's inside |
|---|---|
| [Getting started](getting-started.md) | Install, download RF2, build NDJSON + SQLite, full-text search, CTV3 crossmaps |
| [Search-as-you-type](../commands/sayt.md) | Instant offline autocomplete: interactive TUI, stdio protocol, and an HTTP endpoint |
| [Refsets and code lists](refsets-codelists.md) | Browse reference sets, build and validate clinical code lists |
| [Parquet and DuckDB](parquet-duckdb.md) | Export to Parquet for analytics with DuckDB, pandas, Polars, or Spark |
| [Semantic search and LLMs](semantic-llm.md) | Markdown export for RAG, vector embeddings, semantic search, MCP server |
| [Transitive Closure Table](transitive-closure.md) | O(1) subsumption queries with precomputed ancestor-descendant pairs |
| [Interactive UIs](interactive-uis.md) | Terminal UI and browser-based GUI for browsing concepts |
| [Everything else](everything-else.md) | Release diff, artefact inspection, performance, layered builds, command reference |

---

## Data map

```mermaid
flowchart TD
    RF2["SNOMED RF2 release"] -->|"sct ndjson · build once per release (~52 s / 838k concepts)"| N[("canonical NDJSON artefact")]

    N -->|"sct sqlite"| DB[("snomed.db · SQL + full-text search<br/>+ transitive closure (sct tct)")]
    N -->|"sct parquet"| PQ[("snomed.parquet · analytics with DuckDB / pandas")]
    N -->|"sct markdown"| MD["snomed-concepts/ · one file per concept (RAG)"]
    N -->|"sct embed"| AR[("snomed-embeddings.arrow")]

    DB --> QUERY["sct lexical · lookup · ecl<br/>refset · map · diagram · codelist"]
    DB --> SERVE["sct serve · FHIR R4 server"]
    DB --> MCP["sct mcp · AI tool use via Claude"]
    AR --> SEM["sct semantic · vector search"]
```

### Key `sct` design principles

- **Offline** - everything happens on your local machine, no network calls or external servers required
- **Deterministic** - same RF2 + locale always produces identical output files, which can be version-controlled, diffed, and audited.
- **File based** - each artefact is a single portable file (or directory of files) that can be copied, versioned, and used with standard tools. No custom server or API needed at query time.
- **No special tools required** - query the SQLite database with `sqlite3`, do analytics with DuckDB or pandas, search with `jq` or `ripgrep`, read concept details in VSCode or a Markdown viewer, or use the MCP server to integrate with LLMs like Claude.
