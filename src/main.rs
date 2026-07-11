// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use sct_rs::commands;

/// sct - SNOMED CT local-first toolchain.
///
/// Converts an RF2 Snapshot release into a canonical NDJSON artefact
/// and provides tools to load that artefact into SQLite, Parquet,
/// or per-concept Markdown, and to serve it via a local MCP server.
#[derive(Parser)]
#[command(name = "sct", author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert an RF2 Snapshot directory to a canonical NDJSON artefact.
    Ndjson(commands::ndjson::Args),

    /// Load a SNOMED CT NDJSON artefact into a SQLite database with FTS5.
    Sqlite(commands::sqlite::Args),

    /// Export a SNOMED CT NDJSON artefact to a Parquet file.
    Parquet(commands::parquet::Args),

    /// Export a SNOMED CT NDJSON artefact to per-concept Markdown files.
    Markdown(commands::markdown::Args),

    /// Build and query an FST-backed lexical index (exact/prefix/fuzzy/word search).
    Fst(commands::fst::Args),

    /// Search-as-you-type over the FST index: live interactive TUI, or a `--stdio`
    /// line protocol for embedding sct as a search backend.
    Sayt(commands::sayt::Args),

    /// Evaluate an ECL expression and emit matching concept SCTIDs (pipe-friendly).
    Ecl(commands::ecl::Args),

    /// Draw a concept's definition, ancestors, or descendants (tree/DOT/Mermaid).
    Diagram(commands::diagram::Args),

    /// Start a local MCP server over stdio backed by a SNOMED CT SQLite database.
    Mcp(commands::mcp::Args),

    /// Generate vector embeddings from a SNOMED CT NDJSON artefact (requires Ollama).
    Embed(commands::embed::Args),

    /// Inspect a sct-produced artefact (.ndjson, .db, .arrow) and print a summary.
    Info(commands::info::Args),

    /// Compare two SNOMED CT NDJSON artefacts and report what changed between releases.
    Diff(commands::diff::Args),

    /// Build, validate, and publish clinical code lists (alias: valueset).
    #[command(alias = "valueset")]
    Codelist(commands::codelist::Args),

    /// Inspect SNOMED CT simple reference sets loaded into a SQLite database.
    Refset(commands::refset::Args),

    /// Import final Read v2 maps from NHS Data Migration TRUD item 9.
    Read2(commands::read2::Args),

    /// Build a transitive closure table over the IS-A hierarchy in an existing SQLite database.
    Tct(commands::tct::Args),

    /// Map codes between terminologies (SNOMED/Read v2/CTV3/ICD-10/OPCS-4). Aliases: transcode, crosswalk.
    #[command(alias = "transcode", alias = "crosswalk")]
    Map(commands::map::Args),

    /// Download SNOMED CT RF2 releases via the NHS TRUD API.
    Trud(commands::trud::Args),

    /// Show where sct looks for databases, embeddings, and config files.
    Paths(commands::paths::Args),

    /// Look up a SNOMED CT concept by SCTID or CTV3 code.
    Lookup(commands::lookup::Args),

    /// Keyword (FTS5) search over a SNOMED CT SQLite database.
    Lexical(commands::lexical::Args),

    /// Semantic similarity search over a SNOMED CT Arrow IPC embeddings file (requires Ollama).
    Semantic(commands::semantic::Args),

    /// Print shell completion scripts (bash, zsh, fish, powershell, elvish).
    Completions(commands::completions::Args),

    /// View size of SNOMED CT concepts and their subtree distributions.
    Size(commands::size::Args),

    /// Launch an interactive terminal UI for exploring SNOMED CT.
    #[cfg(feature = "tui")]
    Tui(commands::tui::Args),

    /// Launch a browser-based UI for exploring SNOMED CT (requires --features gui).
    #[cfg(feature = "gui")]
    Gui(commands::gui::Args),

    /// Start a FHIR R4 terminology server over the SQLite database.
    #[cfg(feature = "serve")]
    Serve(commands::serve::Args),

    /// Read NHS Data Migration Workbench .mdb cross-maps (requires --features dmwb).
    #[cfg(feature = "dmwb")]
    Dmwb(commands::dmwb::Args),
}

/// Restore the default SIGPIPE disposition on Unix so that piping `sct` into
/// `head`, `less`, or `diff <(sct …) …` terminates cleanly instead of
/// panicking from `println!` on a closed stdout. Rust's runtime ignores
/// SIGPIPE by default, which turns every broken-pipe write into a panic -
/// fine for long-lived services, wrong for a CLI tool.
#[cfg(unix)]
fn reset_sigpipe() {
    // SAFETY: single FFI call with well-defined semantics, called once at startup.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}

fn main() -> Result<()> {
    reset_sigpipe();
    let cli = Cli::parse();
    match cli.command {
        Command::Ndjson(args) => commands::ndjson::run(args),
        Command::Sqlite(args) => commands::sqlite::run(args),
        Command::Parquet(args) => commands::parquet::run(args),
        Command::Markdown(args) => commands::markdown::run(args),
        Command::Fst(args) => commands::fst::run(args),
        Command::Sayt(args) => commands::sayt::run(args),
        Command::Ecl(args) => commands::ecl::run(args),
        Command::Diagram(args) => commands::diagram::run(args),
        Command::Mcp(args) => commands::mcp::run(args),
        Command::Embed(args) => commands::embed::run(args),
        Command::Info(args) => commands::info::run(args),
        Command::Diff(args) => commands::diff::run(args),
        Command::Codelist(args) => commands::codelist::run(args),
        Command::Refset(args) => commands::refset::run(args),
        Command::Read2(args) => commands::read2::run(args),
        Command::Tct(args) => commands::tct::run(args),
        Command::Map(args) => commands::map::run(args),
        Command::Trud(args) => commands::trud::run(args),
        Command::Paths(args) => commands::paths::run(args),
        Command::Lookup(args) => commands::lookup::run(args),
        Command::Lexical(args) => commands::lexical::run(args),
        Command::Semantic(args) => commands::semantic::run(args),
        Command::Completions(args) => commands::completions::run(args, Cli::command()),
        Command::Size(args) => commands::size::run(args),
        #[cfg(feature = "tui")]
        Command::Tui(args) => commands::tui::run(args),
        #[cfg(feature = "gui")]
        Command::Gui(args) => commands::gui::run(args),
        #[cfg(feature = "serve")]
        Command::Serve(args) => commands::serve::run(args),
        #[cfg(feature = "dmwb")]
        Command::Dmwb(args) => commands::dmwb::run(args),
    }
}
