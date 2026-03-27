mod builder;
mod commands;
mod rf2;
mod schema;

use anyhow::Result;
use clap::{Parser, Subcommand};

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

    /// (Coming soon) Generate vector embeddings from a SNOMED CT NDJSON artefact.
    Embed(commands::embed::Args),
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
    }
}

