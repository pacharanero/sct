# `sct tui` — Keyboard-driven terminal UI for SNOMED CT exploration

Opens a full-screen terminal interface backed by the SQLite database. Three-panel layout:
hierarchy list (top-left), search/results (bottom-left), concept detail (right). No browser
required.

---

## Synopsis

```bash
sct tui [--db <database>]
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--db <file>` | `snomed.db` in cwd, or `$SCT_DB` | SQLite database produced by `sct sqlite`. |

---

## Keybindings

| Key | Action |
|---|---|
| `/` | Focus search input |
| `Tab` / `←` `→` | Switch between panels |
| `↑` `↓` | Navigate list |
| `Enter` | Select concept / drill down |
| `q` | Quit |
| `Ctrl-C` | Quit |

---

## Example

```bash
sct tui
sct tui --db /data/snomed.db
```

---

## Design notes

- Built with [ratatui](https://github.com/ratatui-org/ratatui) and crossterm.
- Requires the `tui` Cargo feature: `cargo build --features tui`.
- Opens the database read-only with a 32 MB page cache for responsive navigation.
- For browser-based exploration, see `sct gui`.
