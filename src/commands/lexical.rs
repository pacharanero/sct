// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct lexical` - Full-text keyword search over a SNOMED CT SQLite database.
//!
//! Uses the FTS5 virtual table built by `sct sqlite`. Supports any FTS5 query
//! syntax: phrase search, prefix search, column filters, boolean operators.
//!
//! Examples:
//!   sct lexical --db snomed.db "heart attack"
//!   sct lexical --db snomed.db "myocardial infarct*"
//!   sct lexical --db snomed.db "heart attack" --hierarchy "Clinical finding"
//!   sct lexical --db snomed.db "heart attack" --limit 20

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use serde_json::json;

use crate::commands::batch::{self, BatchItem, LineMode};
use crate::format::{ConceptFields, ConceptFormat};
use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, ProvenanceFlags};
use crate::sdk::{SearchOptions, Snomed};

#[derive(Parser, Debug)]
pub struct Args {
    /// Search query (FTS5 syntax: phrases, prefix*, boolean AND/OR/NOT). Pass
    /// `-` to read one query per line from stdin.
    pub query: String,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Restrict results to a specific top-level hierarchy (e.g. "Clinical finding").
    #[arg(long)]
    pub hierarchy: Option<String>,

    /// Maximum number of results to return.
    #[arg(long, short, default_value = "10")]
    pub limit: u32,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Emit only matching SCTIDs (newline-delimited) for piping, e.g.
    /// `sct lexical "asthma" --ids | sct codelist add list.codelist -`.
    #[arg(long, conflicts_with = "format")]
    pub ids: bool,

    /// Override the per-concept line template (text output only). See
    /// `docs/commands/refset.md` for the variable list.
    #[arg(long)]
    pub template: Option<String>,

    /// Override the FSN suffix template (rendered only when FSN differs from PT).
    #[arg(long)]
    pub template_fsn_suffix: Option<String>,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

pub fn run(args: Args) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let snomed = Snomed::open(&db)?;
    let prov = snomed.provenance().cloned();
    let out = args.format;
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    if args.query == "-" {
        return run_batch(&snomed, &args, prov.as_ref(), show_prov);
    }

    let mut options = SearchOptions::new(&args.query, args.limit);
    if let Some(hierarchy) = args.hierarchy.as_deref() {
        options = options.hierarchy(hierarchy);
    }

    // `--ids`: machine output for pipes - just SCTIDs on stdout, nothing else.
    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for id in snomed.search_ids_with(options)? {
            writeln!(out, "{id}")?;
        }
        return Ok(());
    }
    let results = snomed.search_with(options)?;

    if results.is_empty() && !out.is_structured() {
        eprintln!("No results for {:?}", args.query);
        return Ok(());
    }

    if out.is_structured() {
        let items = serde_json::to_value(&results)?;
        let value = if show_prov {
            let mut v = json!({ "results": items });
            provenance::inject_into_json(&mut v, prov.as_ref(), true);
            v
        } else {
            items
        };
        if let Some(s) = out.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    let format = ConceptFormat::load().with_overrides(args.template, args.template_fsn_suffix);
    for hit in &results {
        println!(
            "{}",
            format.render(&ConceptFields {
                id: &hit.id,
                pt: &hit.preferred_term,
                fsn: &hit.fsn,
                hierarchy: &hit.hierarchy,
                ..Default::default()
            })
        );
    }

    provenance::print_human_footer(prov.as_ref(), show_prov);

    Ok(())
}

fn run_batch(
    snomed: &Snomed,
    args: &Args,
    prov: Option<&provenance::Provenance>,
    show_prov: bool,
) -> Result<()> {
    let queries = batch::read_stdin(LineMode::Whole, "queries")?;
    if args.ids {
        let mut result_ids = Vec::new();
        let mut budget = batch::ResultBudget::new();
        for query in queries {
            let mut options = SearchOptions::new(&query, budget.query_limit(Some(args.limit)));
            if let Some(hierarchy) = args.hierarchy.as_deref() {
                options = options.hierarchy(hierarchy);
            }
            let ids = snomed.search_ids_with(options)?;
            budget.retain(ids.len(), "lexical search")?;
            result_ids.extend(ids);
        }
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for id in result_ids {
            writeln!(out, "{id}")?;
        }
        return Ok(());
    }

    let mut items = Vec::with_capacity(queries.len());
    let mut budget = batch::ResultBudget::new();
    for query in queries {
        let mut options = SearchOptions::new(&query, budget.query_limit(Some(args.limit)));
        if let Some(hierarchy) = args.hierarchy.as_deref() {
            options = options.hierarchy(hierarchy);
        }
        let results = snomed.search_with(options)?;
        budget.retain(results.len(), "lexical search")?;
        items.push(BatchItem::new(query, results));
    }

    if args.format.is_structured() {
        let mut value = serde_json::json!({ "items": items });
        provenance::inject_into_json(&mut value, prov, show_prov);
        args.format.print(&value)?;
        return Ok(());
    }

    let format = ConceptFormat::load()
        .with_overrides(args.template.clone(), args.template_fsn_suffix.clone());
    for item in &items {
        if item.result.is_empty() {
            eprintln!("No results for {:?}", item.input);
        }
        for hit in &item.result {
            println!(
                "{}",
                format.render(&ConceptFields {
                    id: &hit.id,
                    pt: &hit.preferred_term,
                    fsn: &hit.fsn,
                    hierarchy: &hit.hierarchy,
                    ..Default::default()
                })
            );
        }
    }
    provenance::print_human_footer(prov, show_prov);
    Ok(())
}
