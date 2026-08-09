// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct proximal-primitives` - Compute a concept's proximal primitive
//! supertypes: the most specific primitive concepts that subsume it.
//!
//! Every fully-defined concept is ultimately built on primitive ancestors
//! somewhere up the IS-A hierarchy (the root concept, 138875005, is
//! primitive by definition), so this always returns at least one concept.
//! Useful for classification and post-coordination QA, where the necessary
//! normal form of a concept is expressed in terms of its proximal primitive
//! supertypes plus refinements.
//!
//! Requires a database built with `sct sqlite` from schema v6 onward (the
//! `definition_status` column).

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use crate::output::OutputFormat;
use crate::sdk::Snomed;

#[derive(Parser, Debug)]
pub struct Args {
    /// Focus concept SCTID.
    pub concept: String,

    /// SNOMED CT SQLite database. See `docs/path-resolution.md` for discovery.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
}

pub fn run(args: Args) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let snomed = Snomed::open(&db)?;

    let supertypes = snomed.proximal_primitive_supertypes(&args.concept)?;

    if let Some(rendered) = args.format.render(&supertypes)? {
        println!("{rendered}");
        return Ok(());
    }

    for supertype in &supertypes {
        println!("{}\t{}", supertype.id, supertype.preferred_term);
    }
    eprintln!("{} proximal primitive supertype(s)", supertypes.len());
    Ok(())
}
