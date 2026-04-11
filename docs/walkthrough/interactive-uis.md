# Interactive UIs

Browse SNOMED CT concepts in a terminal UI or a local web GUI.

---

## Terminal UI  `experimental!` :lucide-test-tube:

To reduce the size of the default `sct` binary, the interactive terminal UI is an optional feature that needs to be enabled at build time with the `tui` feature flag. If you built `sct` without it, you can rebuild with: `cargo install --path . --features tui`

> **Docs**: [`sct tui`](../commands/tui.md)

```bash
sct tui --db snomed.db
```

Three-panel layout:

- **Top-left:** Hierarchy browser
- **Bottom-left:** Search box + results
- **Right:** Full concept detail

Keybindings: `/` search, `Tab` switch panels, `↑↓` navigate, `Enter` select, `q` quit.

## Browser UI `experimental!` :lucide-test-tube:

> **Docs**: [`sct gui`](../commands/gui.md)

The browser-based UI is another optional feature that needs to be enabled at build time with the `gui` feature flag. If you built `sct` without it, you can rebuild with: `cargo install --path . --features gui`

```bash
sct gui --db snomed.db
# Opens http://127.0.0.1:8420 in your browser

sct gui                  # --db defaults to ./snomed.db or $SCT_DB
sct gui --port 9000      # custom port
sct gui --no-open        # start server but don't open browser
```

Single-page app with three tabs:

- **Detail** — full concept view: preferred term, FSN, synonyms, attributes, parents, children count
- **Graph** — D3 force-directed graph showing the focal concept (centre), its parents (above), and up to 50 children (below). Draggable nodes, zoom/pan, click any node to navigate.
- **Hierarchy** — browse the 19 top-level SNOMED hierarchies

Bound to localhost only — never accessible from the network.
