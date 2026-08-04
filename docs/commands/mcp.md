# sct mcp

Start a local MCP (Model Context Protocol) server backed by the SNOMED CT SQLite database. Exposes SNOMED CT as a set of tools for Claude Desktop, Claude Code, Cursor, and any other MCP-compatible AI client.

The core terminology and codelist tools run in one binary with no external service; optional semantic search still uses the configured Ollama endpoint. Startup opens SQLite in place rather than loading it into memory. The recorded pre-SDK v0.18.2 baseline remained a few milliseconds even against a full national-edition database, while the current `rmcp` path is awaiting a fresh published measurement (see [Benchmarks](../benchmarks.md#mcp-server-startup-time)). The SNOMED CT database is always read-only; codelist tools can read and write `.codelist` files only beneath an explicitly configured filesystem root.

Design rationale for `snomed_semantic_search` lives in [`spec/commands/mcp.md`](https://github.com/pacharanero/sct/blob/main/spec/commands/mcp.md).

---

## Usage

```
sct mcp [--db <DB>] [--codelist-root <DIR>] [--embeddings <ARROW>] [--model <MODEL>] [--ollama-url <URL>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--db <FILE>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |
| `--codelist-root <DIR>` | `.` | Root directory exposed to codelist tools. Relative tool paths resolve beneath it; traversal and symlink paths are rejected. Set this explicitly for desktop clients whose launch directory may be unpredictable. |
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
| `snomed_map` | Always (mapping data required) | Cross-map between SNOMED CT, CTV3, Read v2, ICD-10, and OPCS-4, with optional history forwarding and a direct target terminology |
| `snomed_semantic_search` | Requires `--embeddings` | Nearest-neighbour semantic search via vector embeddings |

### Unusable-TCT diagnostics

The server checks transitive-closure usability for each `snomed_ancestors` call, including the completion marker, schema, indexes, and source/closure invalidation triggers. This keeps a long-running server accurate if another process builds, repairs, or invalidates the TCT. If the tool succeeds through the slower recursive-CTE fallback, its result includes a namespaced warning in `_meta` while its text and `structuredContent` data remain unchanged:

```json
{
  "_meta": {
    "org.sct/diagnostics": [
      {
        "code": "unusable-transitive-closure",
        "level": "warning",
        "message": "this database has no usable transitive-closure table, so this ancestor query uses slower recursive CTEs. Build or repair it for a big speed-up: `sct tct --db <db>` (or use `sct sqlite --transitive-closure` when creating the database)."
      }
    ]
  }
}
```

MCP clients can surface or log this metadata without treating the successful call as an error. Build or repair the TCT to remove the diagnostic.

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
      "args": ["mcp", "--db", "/path/to/snomed.db",
               "--codelist-root", "/path/to/codelists"]
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
               "--codelist-root", "/path/to/codelists",
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

### Terminology cross-mapping

> "What's the CTV3 code for myocardial infarction?"

Claude calls `snomed_map` with SCTID `22298006`, terminology `snomed`, and `to: "ctv3"`. Specifying `to` returns the direct conversion; omitting it for a SNOMED CT input returns mappings to every supported target.

```json
{
  "code": "22298006",
  "from": "snomed",
  "to": "ctv3",
  "mapped": [
    {
      "source": "22298006",
      "target": "X200E"
    }
  ]
}
```

> "I have a legacy CTV3 code X200E. What's the current SNOMED concept?"

Claude calls `snomed_map` with code `X200E`, terminology `ctv3`, and `to: "snomed"`, and receives the full SNOMED concept details. For inactive SNOMED pivots, `forward_history: true` follows replacement associations before mapping.

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
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}' \
  | sct mcp --db snomed.db --codelist-root ./codelists
```

---

## Transport and protocol

- **Transport:** stdio only (JSON-RPC 2.0 over stdin/stdout)
- **Framing:** one compact JSON-RPC message per newline. Incoming messages are capped at 16 MiB; oversized and unterminated input is rejected without unbounded buffering. At most eight requests are accepted in flight through response transmission; further requests receive JSON-RPC server error `-32000` instead of entering an unbounded work or response queue. Content-Length framing is not part of the MCP stdio transport and is not accepted.
- **Protocol versions supported:** MCP 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25, and 2026-07-28. Current clients use stateless `server/discover` plus per-request metadata; older clients can still use the `initialize` / `notifications/initialized` lifecycle.
- **SDK:** protocol models, lifecycle negotiation, capabilities, and dispatch use the official Rust SDK, `rmcp`. `sct` adopted it before the upstream bounded-reader fix so current stateless discovery and dual-era behavior would not require another hand-written protocol stack; the local transport preserves the existing finite input limit until a fixed stable SDK release is available.
- **Database access:** read-only - the SNOMED CT database is never modified
- **Codelist files:** `codelist_new`, `codelist_add`, and `codelist_remove` atomically write `.codelist` files beneath `--codelist-root`; mutations are serialized within the server so concurrent requests do not overwrite each other. All other tools are read-only. Client-supplied paths that traverse above the root or use symlink components are rejected.
- **Filesystem threat model:** `--codelist-root` is a trusted same-user directory boundary, not an OS sandbox. Do not point it at a directory that an untrusted process can mutate concurrently; a separate process with permission to race path components needs an operating-system sandbox or distinct user account.
- **Tool contracts:** every tool advertises generated input and output schemas plus read-only/destructive/idempotent/open-world annotations. Successful calls return both a useful text block and `structuredContent`; caller-correctable execution failures return `isError: true`, while unknown methods and tools remain JSON-RPC errors.
- **Catalog caching:** current discovery and tool-list responses include a 60-second public cache hint. Legacy responses omit the 2026 result discriminator automatically.
- **Startup time:** the server opens SQLite without loading the terminology into memory. The pre-SDK baseline was a few milliseconds; remeasure the current lifecycle before quoting it as current evidence. See [Benchmarks](../benchmarks.md#mcp-server-startup-time).
- **Schema version check:** validates `schema_version` on startup; warns if the database is newer than the binary, refuses to start if the gap exceeds 5 versions
