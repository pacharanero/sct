# Agent Instructions

`sct` is a local-first SNOMED CT toolchain: one Rust binary that takes raw RF2 releases and produces NDJSON, SQLite, Parquet, Markdown, FST indexes, embeddings, a FHIR server, an MCP server, and TUI/GUI front-ends. No network calls at runtime; everything runs on the user's machine.

This file is the entry point for AI coding agents. Read it before changing anything.

## Read First

- [README.md](README.md) - setup, architecture diagram, feature overview.
- [spec/roadmap.md](spec/roadmap.md) - planned and in-progress work (the `R##` identifiers are stable references used in commits and conversation).
- [spec/adding-a-command.md](spec/adding-a-command.md) - **how to add a new `sct` subcommand**. Read this before creating any new command.
- [~/code/house-style/AGENTS.md](~/code/house-style/AGENTS.md) - cross-repo standards (Rust CLI shape, commit conventions, GitHub Actions pinning, licensing, `s/` scripts).

## Core Invariants

- **The NDJSON artefact is canonical.** Everything else (SQLite, Parquet, Markdown, FST, embeddings) is derived from it and regenerable. Don't introduce a path that bypasses NDJSON for primary data.
- **Read commands open the DB read-only.** Use `crate::commands::open_db_readonly` (or `crate::commands::refset::open_db` which delegates to it). The only commands that open read-write are `sqlite` (build), `tct` (build), and `size` (interactive TCT build after prompt). Don't add new read-write openers without a good reason.
- **SPDX headers on every source file.** `SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd` + `SPDX-License-Identifier: AGPL-3.0-or-later`. REUSE compliance is enforced in CI and in the `s/version++` gate.
- **SQL uses bound parameters.** Never interpolate user input into a SQL string via `format!`. The only `format!` SQL sites interpolate compile-time constants (enum-derived keywords like `EXISTS`/`NOT EXISTS`, or `PRAGMA` names) - never user values.
- **Data on stdout, hints on stderr.** Machine-readable output (JSON, YAML, TSV) goes to stdout. Human hints, progress bars, warnings, and "not found" messages go to stderr.
- **Conventional commits.** `feat(area):`, `fix(area):`, `docs:`, `ci:`, `chore(release):`, `test(area):`. The `s/version++` script regenerates the changelog from committed history via git-cliff.

## Workflow

- `s/version++ [patch|minor|major]` - the **one release action**. Gates the tree (fmt + clippy ×3 + tests ×2 + REUSE), bumps `Cargo.toml`, regenerates `CHANGELOG.md`, commits, and pushes. CI auto-tags and publishes (binaries, crates.io, Homebrew/Scoop/AUR). Never tag locally.
- After a release has published successfully, post a plain-English reply to the [`sct` CHANGELOG and release announcements](https://openhealthhub.org/t/sct-changelog-and-release-announcements/3033) topic using `dsc topic reply openhealthhub 3033 <file>` (`dsc` is the Discourse CLI; dry-run first with `--dry-run`) - not the browser. This is a manual post-release step; `s/version++` does not publish it. Format:
  - First line is an H2 with the version, e.g. `## \`sct\` v0.22.0`.
  - User-facing highlights as a short bullet list - plain English, no internal `R##` identifiers.
  - One link to the GitHub release/changelog compare.
  - One link to the docs site's Installation section rather than separate links per channel (crates.io, Homebrew, AUR, ...) - a single link is easier to keep current as channels change: `https://pacharanero.github.io/sct/walkthrough/getting-started/#installation`.

  Example:

  ```markdown
  ## `sct` v0.22.0

  Released 2026-08-07. [Release notes](https://github.com/pacharanero/sct/releases/tag/v0.22.0) · [compare v0.21.0...v0.22.0](https://github.com/pacharanero/sct/compare/v0.21.0...v0.22.0)

  **Highlights:**

  - `sct codelist export --format ecl` - export a codelist's active members as a compact, exact ECL expression, round-tripping cleanly with `sct codelist add --ecl`.
  - New SNOMED CT primer in the docs, and a new FST/search-internals diagram.
  - `sct trud` downloads are now fsync'd before being persisted.
  - `sct serve` now warns when bound to a non-loopback address.

  **Install/upgrade:** see the [Installation guide](https://pacharanero.github.io/sct/walkthrough/getting-started/#installation).
  ```
- `s/docs` - serve the Zensical docs site locally.
- `s/install` - install local hooks (`s/lint` as pre-commit).
- There is no `s/test` or `s/lint` script; use `cargo test` and `cargo clippy` directly (see below).

## Before Every Commit

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo clippy --features serve --all-targets -- -D warnings
cargo clippy --features dmwb --all-targets -- -D warnings
cargo clippy --manifest-path python/Cargo.toml --all-targets -- -D warnings
cargo test
cargo test --features serve
reuse lint
```

CI runs the same (plus a Windows `cargo check`, a FHIR conformance gate, and a report-only coverage job). The `s/version++` pre-release gate mirrors these checks with an **isolated `SCT_DATA_HOME`** so ambient local databases can't mask environment-dependent failures.

## Adding a New Command

Read [spec/adding-a-command.md](spec/adding-a-command.md) before creating any new subcommand. It covers:

- Checking for existing similar functionality (aliases, subcommands, shared query helpers).
- The `src/commands/<name>.rs` file structure (module docs, `Args` struct, `run()` function).
- Wiring into `src/main.rs` and `src/commands/mod.rs`.
- Naming conventions, aliases (including multi-name aliases for commands known by different names), and how to fit the new name alongside existing command names.
- `--format`/`OutputFormat`, `--db`/`tilde_pathbuf`, `ProvenanceFlags`, and stdin `-` conventions.
- Shell completions (clap-generated, but you need to verify coverage).
- Help-text quality (every `#[arg]` and `#[command]` doc comment is the `--help` text).

## Assurance

- Review the diff and validation results after agent changes.
- For SQL query logic, validate against the committed synthetic RF2 fixture in `tests/fixtures/rf2/` (the `build()` helper in `tests/end_to_end.rs` builds a real DB from it).
- Agent-generated tests must not be the sole basis for accepting query-result correctness - cross-check against a known concept (e.g. 22298006 = Myocardial infarction, 46635009 = Type 1 diabetes mellitus).

## Approval Required

Ask before publishing releases (`s/version++`), deleting branches, force-pushing, changing secrets, or taking externally visible actions. Do not merge your own PRs unless explicitly instructed. The nightly Claude bot reviews PRs but does not merge them.
