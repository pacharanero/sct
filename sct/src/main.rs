mod builder;
mod commands;
mod rf2;
mod schema;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

/// sct — SNOMED CT local-first toolchain.
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

    /// Start a local MCP server over stdio backed by a SNOMED CT SQLite database.
    Mcp(commands::mcp::Args),

    /// Generate vector embeddings from a SNOMED CT NDJSON artefact (requires Ollama).
    Embed(commands::embed::Args),

    /// Inspect a sct-produced artefact (.ndjson, .db, .arrow) and print a summary.
    Info(commands::info::Args),

    /// Compare two SNOMED CT NDJSON artefacts and report what changed between releases.
    Diff(commands::diff::Args),

    /// Keyword (FTS5) search over a SNOMED CT SQLite database.
    Lexical(commands::lexical::Args),

    /// Semantic similarity search over a SNOMED CT Arrow IPC embeddings file (requires Ollama).
    Semantic(commands::semantic::Args),

    /// Print shell completion scripts (bash, zsh, fish, powershell, elvish).
    Completions(commands::completions::Args),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ndjson(args) => commands::ndjson::run(args),
        Command::Sqlite(args) => commands::sqlite::run(args),
        Command::Parquet(args) => commands::parquet::run(args),
        Command::Markdown(args) => commands::markdown::run(args),
        Command::Mcp(args) => commands::mcp::run(args),
        Command::Embed(args) => commands::embed::run(args),
        Command::Info(args) => commands::info::run(args),
        Command::Diff(args) => commands::diff::run(args),
        Command::Lexical(args) => commands::lexical::run(args),
        Command::Semantic(args) => commands::semantic::run(args),
        Command::Completions(args) => commands::completions::run(args, Cli::command()),
    }
}
