# `sct gui` — Browser-based UI for SNOMED CT exploration

Starts a local HTTP server bound to `127.0.0.1` and opens a single-page app in your default
browser. Provides a search interface, concept detail view, hierarchy browser, and parent/child
navigation — with no external dependencies.

---

## Synopsis

```bash
sct gui [--db <database>] [--port <port>] [--no-open]
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--db <file>` | `snomed.db` in cwd, or `$SCT_DB` | SQLite database produced by `sct sqlite`. |
| `--port <n>` | `8420` | TCP port to listen on. |
| `--no-open` | false | Start the server but do not open the browser automatically. |

---

## API routes

| Route | Description |
|---|---|
| `GET /` | Embedded `index.html` (SPA) |
| `GET /api/search?q=&limit=` | FTS5 search results (JSON) |
| `GET /api/concept/:id` | Full concept detail (JSON) |
| `GET /api/children/:id` | Immediate IS-A children (JSON) |
| `GET /api/parents/:id` | Direct parents (JSON) |
| `GET /api/hierarchy` | List of top-level hierarchy names (JSON) |

---

## Examples

```bash
sct gui
sct gui --db /data/snomed.db --port 9000
sct gui --no-open  # start server, connect from another device
```

---

## Design notes

- Bound to `127.0.0.1` only — never accessible from the network.
- The SPA (`assets/index.html`) is embedded at compile time via `include_str!`.
- Built with [Axum](https://github.com/tokio-rs/axum).
- Requires the `gui` Cargo feature: `cargo build --features gui`.
- For terminal-based exploration, see `sct tui`.
