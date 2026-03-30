Run a local MCP (Model Context Protocol) server over stdio, backed by a SNOMED CT SQLite database.

**If you found `sct` via Claude or AI tooling, start here.** This command gives Claude Desktop or Claude Code live access to SNOMED CT — search, concept lookup, hierarchy browsing — with no cloud API and sub-5ms startup.

---

## What can Claude do?

Once connected, you can ask Claude things like:

- *"What is the SNOMED CT code for myocardial infarction?"*
- *"List the children of the Procedure hierarchy related to cardiac surgery."*
- *"What are the ancestors of concept 22298006?"*
- *"Find SNOMED concepts related to 'type 2 diabetes complications'."*

Claude uses five tools under the hood: `snomed_search`, `snomed_concept`, `snomed_children`, `snomed_ancestors`, and `snomed_hierarchy`.

---

## Quick setup

```bash
# Build the database (one-time, ~30s)
sct sqlite --input snomed.ndjson --output snomed.db

# Start the server
sct mcp --db snomed.db
```

---

## Claude Desktop configuration

Add to `claude_desktop_config.json`:

**macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
**Linux:** `~/.config/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "sct": {
      "command": "/absolute/path/to/sct",
      "args": ["mcp", "--db", "/absolute/path/to/snomed.db"]
    }
  }
}
```

Use absolute paths — Claude Desktop does not inherit your shell's `$PATH`. Run `which sct` to find the binary path.

Restart Claude Desktop after editing the config.

---

## Claude Code configuration

Run `/mcp` inside Claude Code and add a new stdio server, or edit `~/.claude.json`:

```json
{
  "mcpServers": {
    "sct": {
      "type": "stdio",
      "command": "/home/you/.cargo/bin/sct",
      "args": ["mcp", "--db", "/absolute/path/to/snomed.db"],
      "env": {}
    }
  }
}
```

---

## Usage

```
sct mcp --db <SQLITE_DB>
```

## Options

| Flag | Description |
|---|---|
| `--db <FILE>` | Path to the SQLite database produced by `sct sqlite`. |

---

## Tools

### `snomed_search`

Free-text search over SNOMED CT concepts using FTS5.

```json
{
  "query": "heart attack",
  "limit": 10
}
```

Returns: `id`, `preferred_term`, `fsn`, `hierarchy`
Default limit: 10. Maximum: 100.

---

### `snomed_concept`

Full detail for a single concept by SCTID.

```json
{
  "id": "22298006"
}
```

Returns all fields: id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path, parents, children_count, attributes, active, module, effective_time.

If the SCTID is not found, returns a descriptive message rather than an error.

---

### `snomed_children`

Immediate IS-A children of a concept.

```json
{
  "id": "22298006",
  "limit": 50
}
```

Returns: `id`, `preferred_term`, `fsn` for each child, ordered by preferred term.
Default limit: 50. Maximum: 500.

---

### `snomed_ancestors`

Full ancestor chain from a concept up to the SNOMED CT root.

```json
{
  "id": "22298006"
}
```

Returns: `id`, `preferred_term`, `fsn`, ordered from root down to immediate parent.

---

### `snomed_hierarchy`

All concepts in a named top-level hierarchy.

```json
{
  "hierarchy": "Clinical finding",
  "limit": 100
}
```

Returns: `id`, `preferred_term`, `fsn`, ordered by preferred term.
Default limit: 100. Maximum: 1000.

Common hierarchy names: `Clinical finding`, `Procedure`, `Substance`, `Organism`, `Body structure`, `Pharmaceutical / biologic product`, `Observable entity`, `Event`, `Social context`, `Environment / geographical location`, `Staging and scales`, `Qualifier value`, `Record artefact`, `Physical object`, `Physical force`, `Foundation metadata concept`, `SNOMED CT Model Component`, `Attribute`, `Namespace concept`.

---

## Protocol details

- Transport: stdio, newline-delimited JSON (MCP spec 2025-11-25)
- Backwards compatible with Content-Length framing (MCP spec 2024-11-05)
- Connection: read-only (`PRAGMA query_only = ON`)
- Startup time: typically < 5ms

---

## Schema version handling

`sct mcp` checks the `schema_version` stored in the database at startup:

- If `schema_version` matches the expected version: starts normally.
- If `schema_version` is **newer** than expected: logs a warning to stderr and continues — most queries will still work.
- If `schema_version` is **much newer** (unsupported): refuses to start with an error explaining which version of `sct` is needed.

This means you can usually run a newer database with an older `sct mcp` binary without breaking anything, but you'll see a warning.
