//! `sct embed` — (Coming soon) Generate vector embeddings from a SNOMED CT NDJSON artefact.
//!
//! This subcommand will produce a LanceDB vector index suitable for semantic
//! search over SNOMED CT concepts. See roadmap.md Milestone 6.

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct Args {
    /// Input NDJSON file produced by `sct ndjson`.
    #[arg(long, short)]
    pub input: Option<PathBuf>,

    /// Embedding model name (e.g. nomic-embed-text).
    #[arg(long, default_value = "nomic-embed-text")]
    pub model: String,

    /// Output LanceDB directory.
    #[arg(long, short)]
    pub output: Option<PathBuf>,
}

pub fn run(_args: Args) -> Result<()> {
    anyhow::bail!(
        "`sct embed` is not yet implemented.\n\
         See roadmap.md Milestone 6 for the planned design (LanceDB + nomic-embed-text via Ollama)."
    )
}
