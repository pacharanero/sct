# sct

A fast, local-first SNOMED CT toolkit written in Rust. Convert a SNOMED CT RF2
release into queryable formats in seconds. Almost ridiculously fast on modern
hardware. Free and open source. No Java. No Elasticsearch. Docker optional.

```bash
cargo install sct-rs
```

```bash
sct ndjson  --rf2 ~/path-to-your-SNOMED-RF2.zip/
```

```bash
sct sqlite  --ndjson snomed.ndjson
```

```bash
sct lexical "heart attack"
```

[:octicons-arrow-right-24: Full walkthrough](walkthrough/index.md) ·
[:octicons-arrow-right-24: Get your own terminology server](deploy/index.md) ·
[:octicons-arrow-right-24: Why build this?](why/why-build-this.md) ·
[:octicons-arrow-right-24: Benchmarks](benchmarks.md)

---

<div class="grid cards" markdown>

-   :material-pipe:{ .lg .middle } __Build the pipeline__

    ---

    Convert an RF2 snapshot into **SQLite**, **Parquet**, **Markdown**, or
    **Arrow embeddings** in a single command. 837,930 concepts in under a
    minute on a laptop.

    [:octicons-arrow-right-24: Walkthrough](walkthrough/index.md)

-   :material-database-search:{ .lg .middle } __Search__

    ---

    **Full-text search** via FTS5 for keywords and phrases. **Typo-tolerant**
    fuzzy and prefix search via a mmap'd **FST index**. **Semantic vector
    search** via local Ollama embeddings. All offline.

    [:octicons-arrow-right-24: sct lexical](commands/lexical.md)
    · [:octicons-arrow-right-24: sct fst](commands/fst.md)
    · [:octicons-arrow-right-24: sct semantic](commands/semantic.md)

-   :material-format-list-checks:{ .lg .middle } __Code lists & ECL__

    ---

    Build version-controllable clinical **code lists**, and populate them with
    **SNOMED CT Expression Constraint Language** - `sct codelist add --ecl
    "<<73211009"` expands a query into concrete concepts.

    [:octicons-arrow-right-24: sct codelist](commands/codelist.md)

-   :material-robot:{ .lg .middle } __Connect to AI__

    ---

    A local **MCP server** exposes SNOMED CT as tools for Claude, Cursor, and
    any other MCP-compatible client. Ask questions about concepts, hierarchies,
    and relationships directly in your AI assistant.

    [:octicons-arrow-right-24: sct mcp](commands/mcp.md)

-   :material-server:{ .lg .middle } __Run a terminology server__

    ---

    Start a FHIR R4 SNOMED CT terminology server on a clean VPS with Docker
    Compose. First boot downloads from TRUD, builds `snomed.db`, and serves
    `$lookup`, `$expand`, `$subsumes`, and `$translate`.

    [:octicons-arrow-right-24: Get your own server](deploy/index.md)
    · [:octicons-arrow-right-24: sct serve](commands/serve.md)

-   :material-compass:{ .lg .middle } __Explore__

    ---

    A keyboard-driven **terminal UI** and a local **web GUI** for browsing
    concepts, navigating hierarchies, and inspecting relationships - no browser
    extension or remote service needed.

    [:octicons-arrow-right-24: sct tui](commands/tui.md)
    · [:octicons-arrow-right-24: sct gui](commands/gui.md)

</div>
