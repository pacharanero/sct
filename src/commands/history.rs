// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct history` - Show the current historical status of a SNOMED CT concept.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, ProvenanceFlags};
use crate::sdk::{ConceptHistory, Snomed};

#[derive(Parser, Debug)]
pub struct Args {
    /// SNOMED CT concept identifier (SCTID).
    pub id: String,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

pub fn run(args: Args) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let snomed = Snomed::open(&db)?;
    let history = snomed
        .concept_history(args.id.trim())?
        .with_context(|| format!("Concept {} not found.", args.id.trim()))?;
    let mode = if args.format.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    if args.format.is_structured() {
        let mut value = serde_json::to_value(history)?;
        provenance::inject_into_json(&mut value, snomed.provenance(), show_prov);
        args.format.print(&value)?;
        return Ok(());
    }

    print_history(&history);
    provenance::print_human_footer(snomed.provenance(), show_prov);
    Ok(())
}

fn print_history(history: &ConceptHistory) {
    println!("  [{}] {}", history.id, history.preferred_term);
    if history.active {
        println!("  ACTIVE");
    } else {
        match &history.inactivation_reason {
            Some(reason) => println!("  INACTIVE - {}", reason.label),
            None => println!("  INACTIVE - reason unavailable"),
        }
        for association in &history.historical_associations {
            let display = association.target_display.as_deref().unwrap_or("?");
            println!(
                "    {}: [{}] {display}",
                humanize_association(&association.association),
                association.target
            );
        }
    }
    println!("  Snapshot effective: {}", history.effective_time);
    println!("  Note: chronological history requires Full RF2 (R25).");
}

fn humanize_association(association: &str) -> String {
    let mut label = association.replace('_', " ");
    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    label
}

#[cfg(test)]
mod tests {
    use super::humanize_association;

    #[test]
    fn humanizes_known_and_future_association_names() {
        assert_eq!(humanize_association("replaced_by"), "Replaced by");
        assert_eq!(
            humanize_association("future_association"),
            "Future association"
        );
    }
}
