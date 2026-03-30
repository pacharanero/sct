Keyboard-driven terminal UI for exploring SNOMED CT interactively — no browser required.

Three panels: **Hierarchy** (top-left), **Search / Results** (bottom-left), **Concept detail** (right). Navigate entirely with the keyboard.

> **Optional feature.** `sct tui` is not included in the default binary. Build with `--features tui` (see [Installation](#installation)).

---

## Usage

```
sct tui [--db <PATH>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--db <PATH>` | `./snomed.db` then `$SCT_DB` | SQLite database produced by `sct sqlite`. |

---

## Example

```bash
# Open the TUI using the database in the current directory
sct tui

# Specify a database path explicitly
sct tui --db /data/snomed.db

# Use an environment variable
SCT_DB=/data/snomed.db sct tui
```

---

## Layout

```
┌─────────────────────────┬───────────────────────────────────────────────┐
│  HIERARCHY              │                                               │
│                         │           CONCEPT DETAIL                      │
│  ▶ Clinical finding     │                                               │
│    Procedure            │  Heart attack                                 │
│    Body structure       │  SCTID: 22298006                              │
│    ...                  │  FSN:   Myocardial infarction (disorder)      │
│                         │                                               │
├─────────────────────────│  Hierarchy                                    │
│  SEARCH                 │    Clinical finding > Ischemic heart disease  │
│ ┌─────────────────────┐ │                                               │
│ │ heart attack_       │ │  Synonyms                                     │
│ └─────────────────────┘ │    Cardiac infarction                         │
│  RESULTS (12)           │    MI - Myocardial infarction                 │
│                         │    Infarction of heart                        │
│  ▶ Heart attack [22298006]                                              │
│    Acute MI [57054005]  │  Attributes                                   │
│    ...                  │    finding_site: Entire heart [302509004]     │
│                         │    associated_morphology: Infarct [55641003]  │
└─────────────────────────┴───────────────────────────────────────────────┘
```

---

## Keyboard reference

| Key | Action |
|:---|:---|
| `/` | Open search input (start typing immediately) |
| `Enter` | Confirm search / open selected concept |
| `Esc` | Cancel search input, return to results panel |
| `Tab` | Cycle focus: Hierarchy → Search/Results → Detail → Hierarchy |
| `←` / `→` | Move focus left / right |
| `↑` / `↓` | Move selection up / down in focused panel |
| `PgUp` / `PgDn` | Scroll detail panel |
| `b` | Back — return to previously viewed concept |
| `h` | Jump focus to Hierarchy panel |
| `q` / `Q` | Quit |
| `Ctrl-C` | Quit |

---

## Workflow

1. **Browse by hierarchy** — focus the Hierarchy panel (press `h`), use `↑↓` to select a hierarchy category, press `Enter` to load its concepts into the Results list.

2. **Search** — press `/` to open the search input. Type a term and wait (150 ms debounce) or press `Enter` to search immediately. Results appear in the Results list ranked by FTS5 relevance.

3. **Inspect a concept** — use `↑↓` in the Results list, press `Enter` to load the full concept detail on the right.

4. **Navigate relationships** — the detail panel lists parents and attributes with their SCTIDs. Type the SCTID into the search box to jump to any related concept, or use `b` to navigate back through your history (up to 20 steps).

---

## FTS5 query syntax

The search box accepts plain terms as well as FTS5 expressions:

| Input | Behaviour |
|:---|:---|
| `diabetes` | Prefix match — finds `diabetes`, `diabetic`, etc. |
| `heart attack` | Exact phrase match |
| `"heart attack"` | Explicit phrase match |
| `diabetes* OR hypertension` | Boolean OR |
| `finding_site:heart` | Field-scoped search |

---

## Installation

`sct tui` is gated behind the `tui` Cargo feature to keep the default binary small. It adds [ratatui](https://ratatui.rs) and [crossterm](https://github.com/crossterm-rs/crossterm) as dependencies.

```bash
# Build with TUI support
cargo install --path sct --features tui

# Or build locally
cargo build --release --manifest-path sct/Cargo.toml --features tui

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

- [`sct lexical`](mcp.md) — CLI keyword search without launching the TUI
- [`sct gui`](gui.md) — browser-based UI with the same data, accessible over localhost
- [`sct mcp`](mcp.md) — expose the same database to an AI assistant via the Model Context Protocol
