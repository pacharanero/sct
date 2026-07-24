# Adding a new `sct` subcommand

This is the detailed guide for creating a new subcommand. It is referenced from [AGENTS.md](../AGENTS.md) and should be read before writing any new command code.

The patterns below are drawn from the living codebase. When in doubt, copy from the closest existing command - `refset.rs` is a good reference for a command with subcommands, `lookup.rs` for a flat read command, `map.rs` for a command with multiple aliases.

## 1. Check for existing functionality first

Before creating a new command, check whether the capability already exists under a different name:

```sh
# Search for existing command names, aliases, and subcommands
rg -n 'enum Command|Subcommand|alias' src/main.rs src/commands/*.rs

# Search for shared query helpers that already do what you need
rg -n 'pub.*fn ' src/commands/*.rs src/*.rs | rg -i '<your-keyword>'

# Check the README command list and docs
rg -n 'sct ' README.md docs/commands/*.md
```

Many `sct` commands share query helpers. For example, `list_refsets` and `list_refset_members` in `src/commands/refset.rs` are reused by `src/commands/mcp.rs` so the CLI and MCP surfaces return identical data. If a helper already exists, use it rather than duplicating the query.

If the feature you're adding is a natural subcommand of an existing command (e.g. `sct refset compare` was added as a subcommand of `refset`, not as a top-level `sct compare`), prefer that over a new top-level command.

## 2. File structure

Create `src/commands/<name>.rs`. Every source file starts with the SPDX header:

```rust
// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct <name>` - <one-line summary>.
//!
//! <longer description of what the command does, any subcommands, and
//! any shared helpers other modules should know about.>

use anyhow::Result;
use clap::Parser;
// ... other imports

/// <help text - this is what `sct --help` and `sct <name> --help` show>
#[derive(Parser, Debug)]
pub struct Args {
    // ...
}

pub fn run(args: Args) -> Result<()> {
    // ...
}
```

If the command has subcommands, use a `Command` enum:

```rust
#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// <help text for subcommand>
    List(ListArgs),

    /// <help text for another subcommand>
    Info(InfoArgs),
}
```

See `src/commands/refset.rs` for a full worked example with subcommands, shared helpers, and the `run()` dispatch pattern.

## 3. Wire into main.rs and commands/mod.rs

**`src/commands/mod.rs`** - add the module declaration:

```rust
pub mod <name>;
```

If the command is feature-gated:

```rust
#[cfg(feature = "serve")]
pub mod serve;
```

**`src/main.rs`** - add the variant to the `Command` enum and the dispatch arm:

```rust
/// <help text - every variant's doc comment is the --help text>
<Name>(commands::<name>::Args),
```

```rust
Command::<Name>(args) => commands::<name>::run(args),
```

Follow the ordering convention: build/pipeline commands first (`ndjson`, `sqlite`, `parquet`, `markdown`, `fst`), then query commands (`ecl`, `lookup`, `lexical`, `refset`, `map`, `diagram`), then utility (`info`, `diff`, `completions`, `paths`), then interactive (`tui`, `gui`, `sayt`, `serve`, `mcp`). Feature-gated commands go at the end of the enum.

## 4. Naming conventions

### Command name

- Single word, lowercase, no hyphens: `lookup`, `refset`, `lexical`, `semantic`.
- If the name is two words, consider whether it's really a subcommand of an existing command (e.g. `refset compare`, not `compare-refset`).
- Fit it alongside the existing names - read `src/main.rs` and make sure the new name doesn't clash phonetically or conceptually. For example, `map` was chosen over `crossmap` or `translate` because it's shorter and sits naturally next to `lookup` and `lexical`.

### Aliases

If users might refer to the command by a different name, add aliases:

```rust
/// Map codes between terminologies (SNOMED/Read v2/CTV3/ICD-10/OPCS-4). Aliases: transcode, crosswalk.
#[command(alias = "transcode", alias = "crosswalk")]
Map(commands::map::Args),
```

Multiple aliases for the same concept (the command is known by different names in different communities) is explicitly encouraged - `map`/`transcode`/`crosswalk` all point to the same implementation. Similarly `codelist`/`valueset`.

When choosing aliases, think about what a user would type from memory. If the function has an established name in the SNOMED/FHIR community that differs from the shortest name, include it as an alias.

### Argument names

- `--db` for the SQLite database path (use `value_parser = crate::paths::tilde_pathbuf` so `~` is expanded).
- `--format` / `-f` for output format (use the shared `OutputFormat` enum, not a command-local one).
- `--output` / `-o` for output file paths.
- `--limit` for row caps (type: `Option<usize>`; `None` means unlimited).
- `--input` for input file paths (or positional if the command takes exactly one input).
- stdin: accept `-` as a positional argument or `--input -` where piping is natural.

## 5. The shared conventions every read command should follow

These are the established patterns that keep the CLI surface consistent. Not every command needs all of them, but omitting one should be a conscious choice, not an oversight:

### `--db` with tilde expansion

```rust
#[arg(long, value_parser = crate::paths::tilde_pathbuf)]
pub db: Option<PathBuf>,
```

When `--db` is omitted, use `crate::paths::resolve_db` which discovers the DB in a standard order (see `docs/path-resolution.md`). Open it read-only:

```rust
let db = crate::paths::resolve_db(args.db.as_deref())?.path;
let conn = crate::commands::open_db_readonly(&db, None)?;
```

### `--format` / `OutputFormat`

Use the shared `crate::output::OutputFormat` enum (text/json/yaml), not a command-local format enum. Include the deprecated `--json` hidden alias for backwards compatibility where it existed:

```rust
#[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
pub format: OutputFormat,

/// Deprecated alias for `--format json`.
#[arg(long, hide = true)]
pub json: bool,
```

Resolve with: `let format = args.format.or_json_flag(args.json);`

### `ProvenanceFlags`

Read commands that display concept data should flatten provenance flags so the user can show/suppress the release footer:

```rust
#[command(flatten)]
pub prov: ProvenanceFlags,
```

See `src/commands/lookup.rs` and `src/commands/refset.rs` for the full provenance handling pattern (`provenance::read_sqlite`, `provenance::should_show`, `provenance::inject_into_json`, `provenance::print_human_footer`).

### stdin `-`

Where piping is natural (e.g. `sct codelist add list.codelist -` reads SCTIDs from stdin), accept `-` as a sentinel and read from `std::io::stdin()`. Check the existing stdin sites in `codelist.rs`, `ecl.rs`, and `map.rs` for the pattern.

### Data on stdout, hints on stderr

Machine-readable output (JSON, YAML, TSV, plain concept IDs) goes to stdout. Human-facing hints ("Concept X not found", "No results", progress bars) go to stderr.

## 6. Shell completions

Completions are clap-generated via `clap_complete`. The `sct completions` command generates scripts for bash, zsh, fish, powershell, and elvish. Because completions are derived from the clap `Command` structure at build time, adding a new subcommand **automatically** adds its completions - there is no manual step.

However, verify:

1. Every `#[arg]` and `#[command]` has a doc comment - these become the completion descriptions shown in the shell.
2. Aliases are included (`#[command(alias = "...")]` - clap generates completions for aliases too).
3. `sct completions bash` (or your shell) produces output that includes the new command name and its subcommands/arguments. A quick smoke test:

```sh
cargo build && target/debug/sct completions bash | grep '<your-command-name>'
```

## 7. Help-text quality

Every doc comment on a `#[derive(Parser)]` struct, `#[derive(Subcommand)]` enum variant, and `#[arg]` field **is** the `--help` text. There is no separate help string to fill in. Rules:

- The top-level command doc comment in `src/main.rs` is one line, shown in the main `sct --help` listing.
- Each `#[arg]` doc comment should explain what the flag does, not just restate its name. For `--db`: "SNOMED CT SQLite database. See `docs/path-resolution.md` for the discovery order when this flag is omitted."
- For flags with a default value, clap automatically shows `[default: <value>]` - don't repeat it in the doc comment unless the default needs explanation.
- Hidden flags (`#[arg(long, hide = true)]`) don't appear in help - use for deprecated aliases.
- If a flag has an `alias`, mention it in the doc comment so users discover it: "Aliases: transcode, crosswalk."

## 8. Tests

- Unit tests for query logic go in a `#[cfg(test)] mod tests` block at the bottom of the command file, using an in-memory SQLite DB (`Connection::open_in_memory()`) with a hand-built schema. See `src/commands/refset.rs` for the pattern.
- For end-to-end tests that exercise the real binary, use `assert_cmd` in `tests/`. See `tests/cli.rs` and `tests/end_to_end.rs` for patterns.
- For tests that need a real DB built from the committed fixture, use the `build()` helper in `tests/end_to_end.rs`.
- Cross-check concept queries against known concepts: 22298006 = Myocardial infarction, 46635009 = Type 1 diabetes mellitus, 73211009 = Diabetes mellitus.

## 9. Documentation

- Add a docs page: `docs/commands/<name>.md`. See existing pages for the format (header, usage examples, output samples).
- Update `README.md` if the command is a user-facing feature that belongs in the overview or the Mermaid diagram.
- Update `spec/roadmap.md` if there's a roadmap item being closed - mark it `[x]` and add a "Shipped:" summary.

## Checklist

- [ ] SPDX header on the new file
- [ ] Module doc comment (the `//!` block)
- [ ] `Args` struct with `#[derive(Parser, Debug)]`
- [ ] Every `#[arg]` and `#[command]` has a doc comment (it's the --help text)
- [ ] `--db` uses `tilde_pathbuf` and `resolve_db` discovery
- [ ] `--format` uses the shared `OutputFormat` (not a local enum)
- [ ] DB opened read-only via `open_db_readonly`
- [ ] Data on stdout, hints on stderr
- [ ] SQL uses bound parameters (no user values in `format!`)
- [ ] `run()` returns `anyhow::Result<()>`
- [ ] Wired into `src/main.rs` (enum variant + dispatch arm)
- [ ] Wired into `src/commands/mod.rs` (`pub mod <name>;`)
- [ ] Completions verified (`sct completions bash | grep <name>`)
- [ ] Docs page created (`docs/commands/<name>.md`)
- [ ] Unit tests for query logic
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test` passes
