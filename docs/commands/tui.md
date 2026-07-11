# sct tui `experimental!` :lucide-test-tube:

Keyboard-driven terminal UI for exploring SNOMED CT interactively - no browser required.

Three panels: **Hierarchy** (top-left), **Search / Results** (bottom-left), **Concept detail** (right). Navigate entirely with the keyboard.

> **In the default build.** `sct tui` ships in the released binaries and `cargo install sct-rs` (the `tui` feature is on by default). It is only absent from `--no-default-features` builds - see [Installation](#installation).

---

## Usage

```
sct tui [--db <PATH>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--db <PATH>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |

---

## Example

```bash
# Open the TUI using the auto-discovered database
# (cwd → $SCT_DATA_HOME/data/ - see Path resolution)
sct tui

# Specify a database path explicitly
sct tui --db /data/snomed.db

# Use an environment variable
SCT_DB=/data/snomed.db sct tui

# After downloading a release with the pipeline, sct tui picks it up
# automatically from ~/.local/share/sct/data/
sct trud download --edition uk_monolith --pipeline
sct tui
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
| `b` | Back - return to previously viewed concept |
| `h` | Jump focus to Hierarchy panel |
| `s` | Toggle size estimates in the detail panel |
| `q` / `Q` | Quit |
| `Ctrl-C` | Quit |

---

## Workflow

1. **Browse by hierarchy** - focus the Hierarchy panel (press `h`), use `↑↓` to select a hierarchy category, press `Enter` to load its concepts into the Results list.

2. **Search** - press `/` to open the search input. Type a term and wait (150 ms debounce) or press `Enter` to search immediately. Results appear in the Results list ranked by FTS5 relevance.

3. **Inspect a concept** - use `↑↓` in the Results list, press `Enter` to load the full concept detail on the right.

4. **Navigate relationships** - the detail panel lists parents and attributes with their SCTIDs. Type the SCTID into the search box to jump to any related concept, or use `b` to navigate back through your history (up to 20 steps).

5. **Inspect subtree size** - press `s` in the detail panel to show the NDJSON and SQLite size estimates for the selected concept's subtree.

---

## FTS5 query syntax

The search box accepts plain terms as well as FTS5 expressions:

| Input | Behaviour |
|:---|:---|
| `diabetes` | Prefix match - finds `diabetes`, `diabetic`, etc. |
| `heart attack` | Exact phrase match |
| `"heart attack"` | Explicit phrase match |
| `diabetes* OR hypertension` | Boolean OR |
| `fsn:heart` | Field-scoped search - restrict to one FTS column (`id`, `preferred_term`, `synonyms`, `fsn`) |

---

## Installation

`sct tui` is part of the default `tui` Cargo feature, so it is present in the released binaries and in `cargo install sct-rs`. It adds [ratatui](https://ratatui.rs) and [crossterm](https://github.com/crossterm-rs/crossterm) as dependencies. Only a `--no-default-features` build (such as the Docker server image) omits it:

```bash
# Server-only build without the terminal UI
cargo install --path sct --no-default-features --features serve
```

---

## Prerequisites

Requires a SNOMED CT SQLite database. The simplest way to produce one is:

```bash
sct trud download --edition uk_monolith --pipeline
```

…which deposits a `.db` under `~/.local/share/sct/data/` that every read-side command (including `sct tui`) auto-discovers. See [Path resolution](../path-resolution.md) for the full chain.

Alternatively, build one explicitly:

```bash
sct sqlite --ndjson snomed.ndjson --output snomed.db
```

---

## See also

- [`sct gui`](gui.md) - browser-based UI with the same data
- [`sct lexical`](lexical.md) - CLI keyword search without the TUI
- [`sct mcp`](mcp.md) - expose the same database to an AI assistant
