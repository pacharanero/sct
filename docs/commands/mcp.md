Start a local MCP (Model Context Protocol) server backed by the SNOMED CT SQLite database. Exposes SNOMED CT as a set of tools for Claude Desktop, Claude Code, Cursor, and any other MCP-compatible AI client.

Single binary, no runtime dependencies, read-only, starts in under 5 ms.

---

## Usage

```
sct mcp --db <DB> [--embeddings <ARROW>] [--model <MODEL>] [--ollama-url <URL>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--db <FILE>` | *(required)* | SQLite database produced by `sct sqlite`. |
| `--embeddings <FILE>` | — | Arrow IPC embeddings file produced by `sct embed`. When supplied, the `snomed_semantic_search` tool is registered. |
| `--model <MODEL>` | `nomic-embed-text` | Ollama embedding model (must match the model used by `sct embed`). |
| `--ollama-url <URL>` | `http://localhost:11434` | Ollama API base URL. |

---

## Tools exposed

| Tool | Always available | Description |
|---|---|---|
| `snomed_search` | ✅ | Free-text search — returns concept ID, preferred term, FSN, hierarchy |
| `snomed_concept` | ✅ | Full concept detail by SCTID |
| `snomed_children` | ✅ | Immediate IS-A children of a concept |
| `snomed_ancestors` | ✅ | Full ancestor chain up to root |
| `snomed_hierarchy` | ✅ | List all concepts in a named top-level hierarchy |
| `snomed_map` | ✅ (UK edition only) | Bidirectional SNOMED↔CTV3/Read v2 cross-map |
| `snomed_semantic_search` | Requires `--embeddings` | Nearest-neighbour semantic search via vector embeddings |

---

## Claude Desktop configuration

Add to `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) or the equivalent on your platform:

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

With semantic search enabled:

```json
{
  "mcpServers": {
    "snomed": {
      "command": "sct",
      "args": ["mcp", "--db", "/path/to/snomed.db",
               "--embeddings", "/path/to/snomed-embeddings.arrow"]
    }
  }
}
```

---

## Example interactions

### Terminology lookup

> "What are the subtypes of type 2 diabetes mellitus?"

Claude calls `snomed_children` with SCTID `44054006`, receives the list, and answers with accurate SNOMED-grounded terminology.

### Semantic search

> "Find me concepts related to difficulty swallowing"

Claude calls `snomed_semantic_search` with the query text, gets back cosine-similarity-ranked concepts, and can explore them further.

### UK CTV3 cross-mapping

> "What's the CTV3 code for myocardial infarction?"

Claude calls `snomed_map` with SCTID `22298006` and terminology `snomed`, receives:

```json
{
  "snomed_id": "22298006",
  "ctv3_codes": ["X200E"],
  "read2_codes": []
}
```

> "I have a legacy CTV3 code X200E. What's the current SNOMED concept?"

Claude calls `snomed_map` with code `X200E` and terminology `ctv3`, receives the full SNOMED concept details.

---

## Verifying startup

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  | (stdbuf -o0 sct mcp --db snomed.db & sleep 0.3; kill %1) 2>/dev/null
```

---

## Transport and protocol

- **Transport:** stdio only (JSON-RPC 2.0 over stdin/stdout)
- **Protocol versions supported:** MCP 2024-11-05 (Content-Length framing) and MCP 2025-03-26+ (newline-delimited JSON). The version is negotiated on `initialize`.
- **Read-only:** never modifies the database
- **Startup time:** < 5 ms (well under the 100 ms MCP budget)
- **Schema version check:** validates `schema_version` on startup; warns if the database is newer than the binary, refuses to start if the gap exceeds 5 versions
