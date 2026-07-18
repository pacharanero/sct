# sct mcp

Start a local MCP (Model Context Protocol) server backed by the SNOMED CT SQLite database. Exposes SNOMED CT as a set of tools for Claude Desktop, Claude Code, Cursor, and any other MCP-compatible AI client.

Single binary, no runtime dependencies. Startup scales with database size rather than being a fixed cost - low milliseconds against a small database, a few hundred milliseconds against a full national-edition database with a transitive closure table (see [Benchmarks](../benchmarks.md#mcp-server-startup-time)). The SNOMED CT database is always read-only; codelist tools can read and write `.codelist` files.

Design rationale for `snomed_semantic_search` lives in [`spec/commands/mcp.md`](https://github.com/pacharanero/sct/blob/main/spec/commands/mcp.md).

---

## Usage

```
sct mcp [--db <DB>] [--embeddings <ARROW>] [--model <MODEL>] [--ollama-url <URL>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--db <FILE>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |
| `--embeddings <FILE>` | - | Arrow IPC embeddings file produced by `sct embed`. When supplied, the `snomed_semantic_search` tool is registered. Not auto-discovered - requires explicit opt-in because the tool needs Ollama. |
| `--model <MODEL>` | `nomic-embed-text` | Ollama embedding model (must match the model used by `sct embed`). |
| `--ollama-url <URL>` | `http://localhost:11434` | Ollama API base URL. |

---

## Tools exposed

### SNOMED CT lookup

| Tool | Available | Description |
|---|---|---|
| `snomed_search` | Always | Free-text search - returns concept ID, preferred term, FSN, hierarchy |
| `snomed_concept` | Always | Full concept detail by SCTID |
| `snomed_children` | Always | Immediate IS-A children of a concept |
| `snomed_ancestors` | Always | Full ancestor chain up to root |
| `snomed_hierarchy` | Always | List all concepts in a named top-level hierarchy |
| `snomed_refsets` | Always | List reference sets loaded in the database, with member counts |
| `snomed_refset_members` | Always | List the concepts belonging to a given reference set |
| `snomed_refset_compare` | Always | Compare membership of two reference sets (only-in-A / only-in-B / in-both) |
| `snomed_refset_profile` | Always | Breakdown of a reference set's members by top-level hierarchy |
| `snomed_map` | Always (UK edition only) | Bidirectional SNOMED↔CTV3/Read v2 cross-map |
| `snomed_semantic_search` | Requires `--embeddings` | Nearest-neighbour semantic search via vector embeddings |

### Code list management

| Tool | Description |
|---|---|
| `codelist_list` | List `.codelist` files in a directory, with title, status, and concept count |
| `codelist_read` | Read a codelist - returns metadata and concept lists (active, excluded, pending) |
| `codelist_new` | Scaffold a new `.codelist` file with YAML front-matter template |
| `codelist_add` | Add concept(s) by SCTID - resolves preferred terms from the database |
| `codelist_remove` | Move a concept to explicitly excluded, preserving the audit trail |
| `codelist_validate` | Validate against the database - inactive concepts, term drift, pending items |
| `codelist_stats` | Concept count, hierarchy breakdown, leaf/intermediate ratio, release age |
| `codelist_export` | Export the codelist as `csv`, `opencodelists-csv`, or `markdown` |

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

### Building a codelist interactively

> "Create a codelist for asthma diagnosis codes in codelists/asthma.codelist, then find the main asthma concepts and add them."

Claude:
1. Calls `codelist_new` to scaffold the file
2. Calls `snomed_search` with `"asthma"` to find candidate concepts
3. Calls `snomed_children` on the top-level asthma concept to explore subtypes
4. Calls `codelist_add` with the chosen SCTIDs
5. Calls `codelist_validate` to confirm everything is active and correct
6. Calls `codelist_stats` to summarise the result

> "The occupational asthma concept shouldn't be in there - exclude it with a note."

Claude calls `codelist_remove` with the SCTID and `comment: "occupational pathway - separate codelist"`.

> "Export this as CSV for upload."

Claude calls `codelist_export` with `format: "opencodelists-csv"` and returns the content.

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
- **Database access:** read-only - the SNOMED CT database is never modified
- **Codelist files:** `codelist_new`, `codelist_add`, and `codelist_remove` write `.codelist` files on disk; all other tools are read-only
- **Startup time:** scales with database size, not fixed - ~2.5 ms against the tiny test fixture, ~373 ms against the full UK Monolith with a transitive closure table (2.6 GB). See [Benchmarks](../benchmarks.md#mcp-server-startup-time).
- **Schema version check:** validates `schema_version` on startup; warns if the database is newer than the binary, refuses to start if the gap exceeds 5 versions
