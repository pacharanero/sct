# sct

A fast, local-first SNOMED CT toolkit written in Rust. Convert a SNOMED CT RF2
release into queryable formats in seconds. Almost ridiculously fast on modern hardware. Free and open source. No Java. No Docker. No terminology server.

```bash
cargo install sct-rs
```

```bash
sct ndjson  --rf2 ~/path-to-your-SNOMED-RF2.zip/
```

```bash
sct sqlite  --input snomed.ndjson
```

```bash
sct lexical "heart attack"
```

[:octicons-arrow-right-24: Full walkthrough](walkthrough.md) ·
[:octicons-arrow-right-24: Why build this?](why-build-this.md) ·
[:octicons-arrow-right-24: Benchmarks](benchmarks.md)

---

<div class="grid cards" markdown>

-   :material-pipe:{ .lg .middle } __Build the pipeline__

    ---

    Convert an RF2 snapshot into **SQLite**, **Parquet**, **Markdown**, or
    **Arrow embeddings** in a single command. 831k concepts in under 30 seconds
    on a laptop.

    [:octicons-arrow-right-24: Walkthrough](walkthrough.md)

-   :material-database-search:{ .lg .middle } __Search__

    ---

    **Full-text search** via FTS5 for keywords and phrases. **Semantic vector
    search** via local Ollama embeddings for meaning-based queries. Both work
    entirely offline.

    [:octicons-arrow-right-24: sct lexical](commands/lexical.md)
    · [:octicons-arrow-right-24: sct semantic](commands/semantic.md)

-   :material-robot:{ .lg .middle } __Connect to AI__

    ---

    A local **MCP server** exposes SNOMED CT as tools for Claude, Cursor, and
    any other MCP-compatible client. Ask questions about concepts, hierarchies,
    and relationships directly in your AI assistant.

    [:octicons-arrow-right-24: sct mcp](commands/mcp.md)

-   :material-compass:{ .lg .middle } __Explore__

    ---

    A keyboard-driven **terminal UI** and a local **web GUI** for browsing
    concepts, navigating hierarchies, and inspecting relationships — no browser
    extension or remote service needed.

    [:octicons-arrow-right-24: sct tui](commands/tui.md)
    · [:octicons-arrow-right-24: sct gui](commands/gui.md)

</div>
