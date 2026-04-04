# sct gui `experimental!` :lucide-test-tube

Browser-based UI for exploring SNOMED CT. Starts a local web server bound to `127.0.0.1` and opens your browser automatically.

Same data as [`sct tui`](tui.md) — search, concept detail, hierarchy browsing, IS-A navigation — in a point-and-click interface with a graph visualisation tab.

> **Optional feature.** `sct gui` is not included in the default binary. Build with `--features gui` (see [Installation](#installation)).

---

## Usage

```bash
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
# Expects a `snomed.db` in the current directory
sct gui

# Specify a database and port
sct gui --db /data/snomed.db --port 9000

# Start headless (e.g. in a remote session or CI)
sct gui --no-open
# then open http://127.0.0.1:8420 in your browser or forward the port

# Use an environment variable for the database path
SCT_DB=/data/snomed.db sct gui
```

Stop the server with `Ctrl-C`.

---

## Interface

The GUI is a single-page app embedded in the binary (no external files needed). It has three tabs:

### Detail tab

Full concept view: preferred term, FSN, synonyms, hierarchy path, parent concepts, children count, and clinical attributes. Click any linked concept to navigate.

### Graph tab

D3 force-directed graph of the IS-A neighbourhood around the currently selected concept:

- **Focal concept** displayed at the centre
- **Parent concepts** shown above, connected by IS-A edges
- **Up to 50 children** shown below

Nodes are draggable; the graph supports zoom and pan. Click any node to navigate to that concept and reload the graph around it.

### Hierarchy browser

Filter the results list to a specific top-level hierarchy (Clinical finding, Procedure, Body structure, etc.).

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
| `GET /api/graph/:id` | IS-A neighbourhood graph (focal + parents + up to 50 children) |

### Example API calls

```bash
# Search
curl 'http://127.0.0.1:8420/api/search?q=heart+attack&limit=5'

# Concept detail
curl 'http://127.0.0.1:8420/api/concept/22298006'

# Children of Diabetes mellitus
curl 'http://127.0.0.1:8420/api/children/73211009'

# Graph data for a concept
curl 'http://127.0.0.1:8420/api/graph/22298006'
```

### Graph response shape

```json
{
  "focal_id": "22298006",
  "nodes": [
    {"id": "22298006", "label": "Heart attack", "type": "focal"},
    {"id": "414795007", "label": "Ischaemic heart disease", "type": "parent"},
    {"id": "57054005",  "label": "Acute myocardial infarction", "type": "child"}
  ],
  "edges": [
    {"source": "414795007", "target": "22298006"},
    {"source": "22298006",  "target": "57054005"}
  ],
  "parent_count": 1,
  "child_count": 47
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

# Build everything (tui + gui)
cargo install --path sct --features full
```

---

## Prerequisites

Requires a `snomed.db` database. Build one with:

```bash
sct sqlite --input snomed.ndjson --output snomed.db
```

---

## See also

- [`sct tui`](tui.md) — keyboard-driven terminal UI, same data, no browser required
- [`sct lexical`](lexical.md) — CLI keyword search
- [`sct mcp`](mcp.md) — expose the database to an AI assistant
