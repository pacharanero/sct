# Path Resolution - DBs, Embeddings, and Config

> **Design spec** (rationale + full rules). The user-facing reference is
> [`docs/path-resolution.md`](../docs/path-resolution.md); keep the two in sync when behaviour changes.

A cross-cutting convention for **where `sct` looks** for databases, embeddings files, and configuration. All read-side commands (`sct lookup`, `sct lexical`, `sct refset`, `sct codelist`, `sct info`, `sct mcp`, `sct semantic`, `sct tui`, `sct gui`) discover their inputs through the rules below; `sct trud` retains its existing write-path resolution.

This spec exists because - prior to v0.3.11 - every command rolled its own discovery and the conventions disagreed. In particular, `sct trud download --pipeline` wrote a `.db` to `~/.local/share/sct/data/` that subsequent commands could not find (issue [#19](https://github.com/pacharanero/sct/issues/19)).

---

## Goals

1. **One discovery chain, used by every command.** No more bespoke `resolve_db_path` per file.
2. **Local-first ergonomics preserved.** A `snomed.db` sitting in the current directory still wins (so `cd ~/snomed-work && sct lexical "asthma"` keeps working).
3. **`sct trud --pipeline` "just works" for the next command.** Whatever `trud` writes under `~/.local/share/sct/data/` is auto-discovered by `sct tui`, `sct lookup`, `sct mcp`, etc.
4. **XDG conventions** for users who already organise their `$HOME` around them.
5. **Errors are diagnostic.** When discovery fails, the error lists every location checked.

---

## Base directories

| Variable | Default | Purpose |
|---|---|---|
| `$SCT_DATA_HOME` | `$XDG_DATA_HOME/sct` → `~/.local/share/sct` | Data root: built artefacts, downloaded releases. |
| `$SCT_CONFIG_HOME` | `$XDG_CONFIG_HOME/sct` → `~/.config/sct` | Config root: `config.toml`. |

`$XDG_DATA_HOME` and `$XDG_CONFIG_HOME` are the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html) variables. When unset they fall back to the conventional `~/.local/share` and `~/.config` paths on Linux/macOS. On Windows the same conventional paths are used (under `$USERPROFILE`); we do not consult `%APPDATA%` because users running `sct` on Windows typically come from the WSL/`scoop` world where dotfile-style paths are expected.

Directory layout under `$SCT_DATA_HOME`:

```
~/.local/share/sct/
├── releases/    downloaded RF2 zips from TRUD
└── data/        built artefacts (.ndjson, .db, .parquet, .arrow)
```

The `releases/` and `data/` subdirectory names are fixed (already in `trud.rs` constants).

---

## Database resolution

When a command takes `--db` and the flag is *not* supplied, it walks the following chain. The first existing file wins.

1. **`$SCT_DB` env var** - explicit per-shell override. If set, the path must exist; if it points at a missing file, fail loudly rather than silently falling through.
2. **`./snomed.db`** - preserves local-dev ergonomics. A project-local DB always beats a global one.
3. **`[paths] db = "…"`** from the config file (see [Config](#config-file)).
4. **`$SCT_DATA_HOME/data/snomed.db`** - canonical name if a user (or future `sct trud --link-latest`) has placed/symlinked one there.
5. **Newest `*.db` in `$SCT_DATA_HOME/data/`** - auto-discovers `sct trud download --pipeline` output, which writes files like `uk_sct2mo_42.1.0_20260506000001z.db`. Newest by `mtime`.

Explicit `--db <path>` always wins over the chain. The path may use `~` for `$HOME`.

### Why `$SCT_DB` beats `./snomed.db`

Earlier `tui.rs` / `gui.rs` did the opposite (CWD first, env var second). This spec inverts it: an env var is a user's *active* override and should not be silently overridden by whatever happens to be in CWD. `./snomed.db` remains step 2 - high priority, but lower than an explicit env.

### When nothing is found

The command exits non-zero with a message listing every path tried:

```
No SNOMED CT database found. Searched (in order):
  --db <path>                              (not supplied)
  $SCT_DB                                  (not set)
  ./snomed.db                              (does not exist)
  ~/.local/share/sct/data/snomed.db        (does not exist)
  ~/.local/share/sct/data/*.db (newest)    (no matches)

Build one with:
  sct trud download --edition uk_monolith --pipeline
  sct sqlite --ndjson snomed.ndjson
```

---

## Embeddings resolution

For `sct semantic` and `sct mcp --embeddings`, the same five-step chain applies with substitutions:

1. `$SCT_EMBEDDINGS`
2. `./snomed-embeddings.arrow`
3. `[paths] embeddings = "…"` from config
4. `$SCT_DATA_HOME/data/snomed-embeddings.arrow`
5. Newest `*.arrow` in `$SCT_DATA_HOME/data/`

The filename `snomed-embeddings.arrow` is the existing default produced by `sct embed`.

---

## Config file

A single config file at `$SCT_CONFIG_HOME/config.toml`. Sections are independent and may be added incrementally; commands ignore sections they don't care about.

```toml
# Default paths used when a command's --db / --embeddings flag is omitted.
# Slot in between $SCT_*/CWD env-and-cwd and the $SCT_DATA_HOME data dir
# (see resolution order above).
[paths]
db = "~/snomed/uk-monolith-2026-05.db"
embeddings = "~/snomed/embeddings.arrow"

# Existing sections (unchanged by this spec, documented for completeness):

[trud]
api_key = "…"
download_dir = "~/.local/share/sct/releases"
data_dir = "~/.local/share/sct/data"
default_edition = "uk_monolith"

[trud.editions.uk_monolith]
trud_item = 1799

[format]
concept = "{id} | {pt} ({hierarchy})"
concept_fsn_suffix = " - FSN: {fsn}"
```

### Config file resolution

The config file path itself follows a chain - but a simpler one than db/embeddings. Only the first one found is used (config sections are *not* layered across files; that would be more complexity than the current usage warrants).

1. `$SCT_CONFIG` env var
2. `./sct.toml` (project-local override; new in this spec)
3. `$SCT_CONFIG_HOME/config.toml`

If none exist, all sections default to empty - every command must already handle a missing config file (e.g. `format::ConceptFormat::load()` falls back to `Default`).

A `--config <path>` CLI flag is **not** added in this version. The env var covers one-shot overrides cleanly enough; we can add the flag later if a real need surfaces.

---

## Write paths (unchanged)

This spec covers **read** discovery only. Commands that write files keep their existing defaults:

| Command | Default output | Set via |
|---|---|---|
| `sct sqlite` | `./snomed.db` | `--output` |
| `sct ndjson` | `./snomed.ndjson` | `--output` |
| `sct parquet` | `./snomed.parquet` | `--output` |
| `sct embed` | `./snomed-embeddings.arrow` | `--output` |
| `sct trud download` | `$SCT_DATA_HOME/releases/<zip>` | `--output-dir` / `download_dir` in `[trud]` |
| `sct trud download --pipeline` build artefacts | `$SCT_DATA_HOME/data/<…>.db` | `--data-dir` / `data_dir` in `[trud]` |

Two write conventions remain - CWD for one-shot `sct sqlite` runs, data home for `sct trud` automation. Changing this is out of scope; the read-side spec makes the inconsistency invisible to downstream commands either way.

---

## The `sct paths` command

A new subcommand that prints the resolved values. Diagnostic, read-only, no flags.

```
$ sct paths
data home:       ~/.local/share/sct                                            (XDG default)
config home:     ~/.config/sct
config file:     ~/.config/sct/config.toml                                     (exists)

database:        ~/.local/share/sct/data/uk_sct2mo_42.1.0_20260506000001z.db   (auto, newest in data dir)
embeddings:      ─                                                             (not found)

trud releases:   ~/.local/share/sct/releases                                   (3 files)
trud data:       ~/.local/share/sct/data                                       (5 files)
```

Each row shows the resolved path and a parenthetical hint about *which* resolution rule matched (e.g. `--db flag`, `$SCT_DB`, `cwd`, `config [paths]`, `auto, newest in data dir`, `not found`). The hint is the diagnostic value - it tells the user exactly why a particular path won, which is what makes the "no DB found" debugging loop one command long.

`sct paths` does not take a query or filter. If we later need a machine-readable form, add `--json`.

---

## Implementation outline

A new `src/paths.rs` module owns the resolution functions:

```rust
pub fn data_home() -> PathBuf;
pub fn config_home() -> PathBuf;
pub fn config_path() -> PathBuf;
pub fn load_config() -> Config;          // shared with trud / format

pub fn resolve_db(arg: Option<&Path>) -> Result<Resolved>;
pub fn resolve_embeddings(arg: Option<&Path>) -> Result<Resolved>;

pub struct Resolved {
    pub path: PathBuf,
    pub source: Source,                  // for `sct paths` and error reporting
}

pub enum Source {
    Flag, Env(&'static str), Cwd, Config, DataHomeCanonical, DataHomeNewest,
}
```

The `Config` struct moves out of `trud.rs` into `paths.rs` and gains a `[paths]` section. `trud.rs` and `format.rs` re-export or use `paths::load_config` instead of rolling their own. The existing `sct_data_home()` / `expand_tilde()` helpers in `trud.rs` move into `paths.rs` and lose the `sct_` prefix (`data_home()`, `expand_tilde()`).

Every `--db: PathBuf` with `default_value = "snomed.db"` becomes `--db: Option<PathBuf>` and the command's `run()` opens with:

```rust
let db = paths::resolve_db(args.db.as_deref())?.path;
let conn = commands::open_db_readonly(&db, None)?;
```

The error message in `resolve_db` is the diagnostic block shown above. `tui.rs` and `gui.rs` drop their bespoke `resolve_db_path` (currently lines 68-86 and 72-90 respectively).

### Testing

`paths::resolve_db` is pure I/O against the filesystem and env. Tests use `tempfile::tempdir()` for the data home and scoped env mutations. Coverage targets:

- Every step of the chain wins in isolation (flag, env, cwd, config, data-home canonical, data-home newest)
- Env var set to a missing path → hard error, not fallthrough
- Newest-by-mtime tiebreak is stable
- `expand_tilde` round-trips `~/foo` and `~user/foo` (we currently only support `~/`; document the limitation)
- Missing chain → error message contains every path tried

The existing trud tests already use `unsafe { std::env::set_var }` (called out in roadmap as fragile). New tests inherit that pattern but the underlying race is unchanged.

---

## Migration notes

For users:

- No behaviour change if `--db` is supplied explicitly.
- No behaviour change if a `snomed.db` exists in CWD.
- New behaviour: commands now discover `sct trud --pipeline` output automatically. The only way this could surprise someone is if they had a stale `.db` under `~/.local/share/sct/data/` and ran a read-side command from a directory with no local DB - they would now get results from the stale DB instead of an error. Mitigation: `sct paths` shows which DB was picked and why.

For commands:

- No API breakage. All `--db` and `--embeddings` flags continue to work.
- The change from `PathBuf` with `default_value` to `Option<PathBuf>` is internal; clap parses identically from the user's perspective.

For docs:

- Every command page that documents `--db` or `--embeddings` updates the default-value cell to point at `docs/path-resolution.md`.
- New `docs/path-resolution.md` is the user-facing companion to this spec.
