// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct cite` - Cite a terminology source, local derived artefact, and software.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};

use crate::citation::Citation;

/// Citation output format.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum CitationFormat {
    /// Human-readable citation and data-availability statement.
    #[default]
    Text,
    /// Three BibTeX entries for the source, artefact, and software.
    Bibtex,
    /// Three CSL-JSON bibliography entries. Aliases: csl-json, json.
    #[value(name = "csljson", alias = "csl-json", alias = "json")]
    CslJson,
}

/// Cite the terminology source, exact local artefact identity, and producing software.
#[derive(Parser, Debug)]
pub struct Args {
    /// SNOMED CT SQLite database. See `docs/path-resolution.md` for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Citation output format.
    #[arg(long, short = 'f', value_enum, default_value_t = CitationFormat::Text)]
    pub format: CitationFormat,
}

pub fn run(args: Args) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = crate::commands::open_db_readonly(&db, None)?;
    let provenance = crate::provenance::read_sqlite(&conn)?.with_context(|| {
        "database has no usable citation provenance; rebuild it from known RF2 inputs with `sct ndjson` then `sct sqlite`"
    })?;
    let citation = Citation::from_provenance(&provenance)?;

    let rendered = match args.format {
        CitationFormat::Text => citation.render_text(),
        CitationFormat::Bibtex => citation.render_bibtex(),
        CitationFormat::CslJson => citation.render_csl_json()?,
    };
    let mut stdout = io::stdout().lock();
    stdout.write_all(rendered.as_bytes())?;
    if !rendered.ends_with('\n') {
        stdout.write_all(b"\n")?;
    }
    Ok(())
}
