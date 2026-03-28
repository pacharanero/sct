# `sct mcp` — Run a local MCP server over the SNOMED CT SQLite database

Starts a Model Context Protocol server over stdio, exposing the SQLite database as a set of
tools for use in Claude Desktop, Cursor, and any other MCP-compatible AI client. Single binary,
no runtime dependencies, read-only, starts in under 100 ms.

---

## Synopsis

```bash
sct mcp --db <database>
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--db <file>` | `snomed.db` in cwd, or `$SCT_DB` | Path to the SQLite database produced by `sct sqlite`. |

---

## MCP tools exposed

| Tool | Description |
|---|---|
| `snomed_search` | Free-text search — returns concept ID, preferred term, FSN, hierarchy |
| `snomed_concept` | Full concept detail by SCTID |
| `snomed_children` | Immediate IS-A children of a concept |
| `snomed_ancestors` | Full ancestor chain up to root |
| `snomed_hierarchy` | List all concepts in a named top-level hierarchy |

---

## Claude Desktop configuration

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

---

## Examples

```bash
# Start manually and send an initialize request to verify startup
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  | (stdbuf -o0 sct mcp --db snomed.db & sleep 0.3; kill %1) 2>/dev/null
```

---

## Design constraints

- **Single binary** — no runtime dependencies beyond the SQLite file
- **Stdio transport only** — no HTTP, no TLS, no port management
- **Read-only** — never modifies the database
- **Starts in under 100 ms** — suitable for Claude Desktop without perceptible delay (verified:
  response appears within \< 5 ms of receiving the first message)
- **Schema version check** — validates `schema_version` on startup; warns if the database is
  newer than the binary, refuses to start if the version gap exceeds 5

---

## Planned: semantic search tool

A future `snomed_semantic_search` tool will load the Arrow IPC file produced by `sct embed` and
return nearest-neighbour concepts for a natural-language query. Controlled by an optional
`--embeddings <arrow-file>` flag; if absent the tool is simply not registered.
