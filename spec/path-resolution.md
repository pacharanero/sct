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
5. **Newest `*.db` in `$SCT_DATA_HOME/data/`** - auto-discovers `sct trud download --pipeline` output, which writes files like `snomedct-monolithrf2-production-20260701t120000z.db`. Newest by `mtime`.
6. **Newest `*.db` in the working directory** - last resort. Build commands name their output after their input (see [Write paths](#write-paths-the-stem-propagates)), so a database you just built locally is `<release>.db`, which step 2's exact-name match does not see. This step keeps the zero-flag workflow working in a directory you have just built in.

Step 6 runs last deliberately: it cannot shadow a flag, env var, config entry, or canonically named file, so no resolution that succeeded before it existed changes behaviour. It only converts what used to be a "not found" error into a match.

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
  ./*.db (newest)                          (no matches)

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
6. Newest `*.arrow` in the working directory

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

## Write paths: the stem propagates

**Every build command names its output after its input**, so the release identity set at the top of the pipeline survives to the bottom:

```text
SnomedCT_MonolithRF2_PRODUCTION_20260701T120000Z.zip
  └── sct ndjson   → snomedct-monolithrf2-production-20260701t120000z.ndjson
        ├── sct sqlite   → snomedct-monolithrf2-production-20260701t120000z.db
        ├── sct parquet  → snomedct-monolithrf2-production-20260701t120000z.parquet
        ├── sct fst      → snomedct-monolithrf2-production-20260701t120000z.fst
        └── sct embed    → snomedct-monolithrf2-production-...-embeddings.arrow
```

| Command | Default output | Set via |
|---|---|---|
| `sct ndjson` | `<slug of first --rf2>.ndjson` | `--output` (`-` for stdout) |
| `sct sqlite` | `<input stem>.db` | `--output` |
| `sct parquet` | `<input stem>.parquet` | `--output` |
| `sct fst build` | `<input stem>.fst` | `--output` |
| `sct embed` | `<input stem>-embeddings.arrow` | `--output` |
| `sct markdown` | `<input stem>-concepts/` | `--output` |
| `sct trud download` | `$SCT_DATA_HOME/releases/<zip>` | `--output-dir` / `download_dir` in `[trud]` |
| `sct trud download --pipeline` build artefacts | `$SCT_DATA_HOME/data/<slug>.{ndjson,db}` | `--data-dir` / `data_dir` in `[trud]` |

Rules:

- The stem comes from the input's file stem: `paths::derived_output`. `sct ndjson` slugifies its RF2 input (lowercase, non-alphanumerics to hyphens, `.zip` stripped) via `ndjson::slugify_path`; the `sct trud` pipeline uses the same function, so a TRUD-built workspace and a hand-built one are named identically.
- **The canonical names are this rule applied to a canonical input.** `snomed.ndjson` yields `snomed.db`, `snomed.parquet`, `snomed-embeddings.arrow`. They are not a separate case, which is why `paths::CANONICAL_*` still describes what discovery looks for.
- Output is always a **bare filename in the working directory**, never beside the input, which may live somewhere read-only. `sct trud` is the exception: it writes into the data home by design.
- Input from stdin (`-`) has no name to inherit, so it falls back to the `snomed` stem.
- A derived name is printed to stderr as `Output: <path>`. A name the user did not type must never be a surprise, and the next command in the pipeline needs to know what to consume.

### FST index lookup

`sct fst search`, `sct sayt`, and `sct serve --fst` have never had an env var or config entry, and still do not. When `--index`/`--fst` is omitted they call `paths::find_fst_index(dir)`, which prefers `snomed.fst` in that directory and otherwise takes the newest `*.fst` there - the same two final steps as the `--db` chain. `dir` is the working directory for `fst search` and `sayt`, and the database's directory for `serve`. Without this, naming an index after its input would have made `sct fst build` followed by a bare `sct fst search` fail.

Two write locations remain - the working directory for one-shot runs, the data home for `sct trud` automation - and the read chain finds either.

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
    Flag, Env(&'static str), Cwd, Config, DataHomeCanonical, DataHomeNewest, CwdNewest,
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

- Every step of the chain wins in isolation (flag, env, cwd, config, data-home canonical, data-home newest, cwd newest)
- The cwd-newest step never outranks a populated data home
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
