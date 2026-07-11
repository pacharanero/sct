# Path resolution

`sct` looks for databases, embeddings files, and configuration in a small set of well-known places. Every read-side command - `sct lookup`, `sct lexical`, `sct refset`, `sct codelist`, `sct mcp`, `sct semantic`, `sct tui`, `sct gui` - uses the same chain, so once a release has been built (e.g. by `sct trud download --pipeline`) every other command finds it automatically.

If you ever wonder *which* file the next command will pick, run `sct paths` - it prints every resolved location and the rule that won.

> Full specification: [`spec/path-resolution.md`](https://github.com/pacharanero/sct/blob/main/spec/path-resolution.md).

**Tilde (`~`) in path arguments.** Every path flag and positional (`--db`, `--ndjson`, `--rf2`, `--output`, `--codelists`, an input file, and so on) expands a leading `~/` to your home directory as it is parsed, so `--ndjson=~/data/snomed.ndjson` and `sct info ~/data/snomed.ndjson` work no matter how your shell treats `~` after an `=` or inside quotes - not just the unquoted, space-separated form the shell expands for you.

---

## Base directories

| Variable | Default | Purpose |
|---|---|---|
| `$SCT_DATA_HOME` | `$XDG_DATA_HOME/sct` → `~/.local/share/sct` | Data root: built artefacts and downloaded RF2 releases. |
| `$SCT_CONFIG_HOME` | `$XDG_CONFIG_HOME/sct` → `~/.config/sct` | Config root: `config.toml`. |

Directory layout under `$SCT_DATA_HOME`:

```
~/.local/share/sct/
├── releases/    downloaded RF2 zips from TRUD
└── data/        built artefacts (.ndjson, .db, .parquet, .arrow)
```

---

## Database resolution (`--db`)

When `--db` is not supplied, `sct` walks this chain and uses the first match:

1. **`$SCT_DB`** environment variable
2. **`./snomed.db`** in the current directory
3. **`[paths] db = "..."`** in the config file
4. **`$SCT_DATA_HOME/data/snomed.db`**
5. **Newest `*.db`** in `$SCT_DATA_HOME/data/`

If `$SCT_DB` is set but points at a missing file, `sct` errors out rather than silently falling through - that almost always means a typo.

Step 5 is the one that makes `sct trud download --pipeline` followed by `sct tui` (or `sct lookup`, or any other read-side command) Just Work.

## Embeddings resolution (`--embeddings`)

Same five-step chain, with `SCT_EMBEDDINGS` and `snomed-embeddings.arrow` substituted for their database equivalents.

## Config file resolution

The config file location is resolved as:

1. **`$SCT_CONFIG`** environment variable
2. **`./sct.toml`** in the current directory (project-local override)
3. **`$SCT_CONFIG_HOME/config.toml`**

If none exist, all sections default to empty.

---

## Config file format

```toml
# Default paths used when --db / --embeddings are omitted. Slot into the
# resolution chain between $SCT_DB / cwd and the $SCT_DATA_HOME data dir.
[paths]
db = "~/snomed/uk-monolith-2026-05.db"
embeddings = "~/snomed/embeddings.arrow"

# Existing sections - documented in their respective command pages.
[trud]
api_key = "..."
download_dir = "~/.local/share/sct/releases"
data_dir = "~/.local/share/sct/data"

[format]
concept = "{id} | {pt} ({hierarchy})"
concept_fsn_suffix = " - FSN: {fsn}"
```

A leading `~/` in any path is expanded to `$HOME`.

---

## When discovery fails

Every read-side command emits the same diagnostic if nothing matches:

```
No SNOMED CT database found. Searched (in order):
  --db <path>                              (not supplied)
  $SCT_DB                                  (not set)
  ./snomed.db                              (not present)
  config [paths]                           (unset)
  ~/.local/share/sct/data/snomed.db        (not present)
  ~/.local/share/sct/data/*.db (newest)    (no matches)

Build one with:
  sct trud download --edition uk_monolith --pipeline
  sct sqlite --ndjson snomed.ndjson
```

The message lists every step that was tried - so it's always obvious whether to set an env var, drop a file in cwd, or run `sct trud`.

---

## `sct paths`

`sct paths` prints the currently resolved locations:

```
$ sct paths
data home:       ~/.local/share/sct                                           default
config home:     ~/.config/sct                                                default
config file:     ~/.config/sct/config.toml                                    exists

database:        ~/.local/share/sct/data/uk_sct2mo_42.1.0_20260506000001z.db  data home, newest
embeddings:      ─                                                            not found

trud releases:   ~/.local/share/sct/releases                                  3 files
trud data:       ~/.local/share/sct/data                                      5 files
```

The right-hand column says exactly *which* resolution rule matched. Useful when a discovered DB isn't the one you expected.

---

## Write paths (unchanged)

This convention covers **read** discovery only. Commands that *write* files keep their existing defaults:

| Command | Default output |
|---|---|
| `sct sqlite` | `./snomed.db` |
| `sct ndjson` | Slugified name of the first `--rf2` input (e.g. `snomedct-monolithrf2-production-...ndjson`); use `-o` to pick your own |
| `sct parquet` | `./snomed.parquet` |
| `sct embed` | `./snomed-embeddings.arrow` |
| `sct trud download` | `$SCT_DATA_HOME/releases/<zip>` |
| `sct trud download --pipeline` artefacts | `$SCT_DATA_HOME/data/<name>.db` etc. |

In other words, one-shot interactive runs default to the current directory; the `sct trud` automation pipeline defaults to the data home. Either way, the read chain finds the result.
