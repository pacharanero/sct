# sct gui

Browser-based UI for exploring SNOMED CT. Starts a local web server bound to `127.0.0.1` and opens your browser automatically.

Same data as [`sct tui`](tui.md) — search, concept detail, hierarchy browsing, IS-A navigation — in a point-and-click interface with no terminal required.

> **Optional feature.** `sct gui` is not included in the default binary. Build with `--features gui` (see [Installation](#installation)).

---

## Usage

```
sct gui [--db <PATH>] [--port <PORT>] [--no-open]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--db <PATH>` | `./snomed.db` then `$SCT_DB` | SQLite database produced by `sct sqlite`. |
| `--port <PORT>` | `8420` | TCP port to listen on. |
| `--no-open` | *(flag)* | Start the server but do not open a browser window. |

---

## Example

```bash
# Start with defaults — opens http://127.0.0.1:8420 in the browser
sct gui

# Specify a database and port
sct gui --db /data/snomed.db --port 9000

# Start headless (e.g. in a remote session)
sct gui --no-open
# then open http://127.0.0.1:8420 in your browser or forward the port

# Use an environment variable for the database path
SCT_DB=/data/snomed.db sct gui
```

Stop the server with `Ctrl-C`.

---

## Interface

The GUI is a single-page app embedded in the binary (no external files needed). It provides:

- **Search bar** at the top — type to search by preferred term, synonym, or FSN via FTS5. Results update as you type (debounced).
- **Results list** — click any concept to load its detail.
- **Concept detail panel** — shows FSN, synonyms, hierarchy path, parent concepts, children count, and clinical attributes.
- **Parent / children navigation** — click any linked concept to traverse the IS-A hierarchy.
- **Hierarchy browser** — filter results to a top-level hierarchy (Clinical finding, Procedure, Body structure, etc.).

The UI is dark-themed with NHS Blue header, readable at any window size.

---

## API

The server exposes a JSON API consumed by the embedded frontend. All routes are read-only and bound to `127.0.0.1` only.

| Endpoint | Description |
|:---|:---|
| `GET /` | Embedded SPA (index.html) |
| `GET /api/search?q=<TERM>&limit=<N>` | FTS5 search; returns id, preferred_term, fsn, hierarchy |
| `GET /api/concept/:id` | Full concept detail by SCTID |
| `GET /api/children/:id` | Immediate IS-A children (up to 200) |
| `GET /api/parents/:id` | Direct parents |
| `GET /api/hierarchy` | List of top-level hierarchy names |

### Example API calls

```bash
# Search
curl 'http://127.0.0.1:8420/api/search?q=heart+attack&limit=5'

# Concept detail
curl 'http://127.0.0.1:8420/api/concept/22298006'

# Children of Diabetes mellitus
curl 'http://127.0.0.1:8420/api/children/73211009'

# All top-level hierarchies
curl 'http://127.0.0.1:8420/api/hierarchy'
```

### Search response shape

```json
{
  "query": "heart attack",
  "total": 12,
  "results": [
    {
      "id": "22298006",
      "preferred_term": "Heart attack",
      "fsn": "Myocardial infarction (disorder)",
      "hierarchy": "Clinical finding"
    }
  ]
}
```

### Concept detail response shape

```json
{
  "id": "22298006",
  "fsn": "Myocardial infarction (disorder)",
  "preferred_term": "Heart attack",
  "synonyms": ["Cardiac infarction", "MI - Myocardial infarction"],
  "hierarchy": "Clinical finding",
  "hierarchy_path": ["SNOMED CT concept", "Clinical finding", "...", "Myocardial infarction"],
  "parents": [{"id": "414795007", "fsn": "Ischemic heart disease (disorder)"}],
  "children_count": 47,
  "attributes": {
    "finding_site": [{"id": "302509004", "fsn": "Entire heart (body structure)"}],
    "associated_morphology": [{"id": "55641003", "fsn": "Infarct (morphologic abnormality)"}]
  }
}
```

---

## FTS5 query syntax

The search bar accepts plain terms as well as FTS5 expressions:

| Input | Behaviour |
|:---|:---|
| `diabetes` | Prefix match — finds `diabetes`, `diabetic`, etc. |
| `heart attack` | Exact phrase match |
| `"heart attack"` | Explicit phrase match |
| `diabetes* OR hypertension` | Boolean OR |

---

## Security

The server binds exclusively to `127.0.0.1` — it is not accessible from other machines on the network. It is read-only (no write routes). Do not expose the port externally.

---

## Installation

`sct gui` is gated behind the `gui` Cargo feature to keep the default binary small. It adds [axum](https://github.com/tokio-rs/axum), [tokio](https://tokio.rs), and [open](https://github.com/Byron/open-rs) as dependencies.

```bash
# Build with GUI support
cargo install --path sct --features gui

# Or build locally
cargo build --release --manifest-path sct/Cargo.toml --features gui

# Build everything (tui + gui)
cargo install --path sct --features full
```

---

## Prerequisites

Requires a `snomed.db` database. Build one with:

```bash
sct sqlite --input snomed.ndjson --output snomed.db
```

See [sct sqlite](sqlite.md) for the full build pipeline.

---

## Next steps

- [`sct tui`](tui.md) — keyboard-driven terminal UI, same data, no browser required
- [`sct lexical`](mcp.md) — CLI keyword search
- [`sct mcp`](mcp.md) — expose the database to an AI assistant via the Model Context Protocol
