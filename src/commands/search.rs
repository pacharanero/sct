// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct search` - one front door onto every search strategy `sct` offers.
//!
//! `sct` grew keyword, typo-tolerant, and meaning-based search as separate
//! top-level commands (`sct lexical`, `sct sayt --fuzzy`, `sct semantic`),
//! each mature and independently useful, but a user has no single place that
//! names the choice or explains the tradeoff. `sct search <mode>` is that
//! place. It does not replace the commands it fronts, and does not
//! reimplement their query logic - see [`docs/commands/search.md`] for the
//! comparison table this module exists to make discoverable.
//!
//! Three subcommands:
//!   - `lexical`  - a genuine passthrough. [`Command::Lexical`] carries the
//!     real [`super::lexical::Args`] and calls [`super::lexical::run`]
//!     directly, so `sct search lexical` and `sct lexical` are the exact same
//!     code path, byte for byte.
//!   - `semantic` - the same passthrough shape over [`super::semantic`].
//!   - `fuzzy`    - not a passthrough of an existing top-level command, but
//!     not new search logic either: it calls the same
//!     [`crate::index::query::Index::search_typeahead`] that `sct sayt
//!     --fuzzy` already uses (prefix, then multi-word, then typo-tolerant
//!     Levenshtein fallback), wrapped in the `--format`/`--ids`/batch/
//!     provenance conventions the polished commands share. `sct fst search
//!     --fuzzy <N>` remains the lower-level, single-pass primitive for
//!     development and benchmarking, matching how `sct fst build` sits below
//!     `sct sqlite`/`sct lexical` rather than duplicating them.
//!
//! `hybrid` (weighted lexical/semantic/fuzzy fusion) is deliberately not
//! here yet. Roadmap `R14`/`R15` call for evaluating a ranking combination
//! against the guideline-derived corpus (`sct bench semantic`, `R62`) before
//! committing to one; shipping an unmeasured weighting scheme under a mode
//! name a user would reasonably trust would be exactly the invariant this
//! project's own assurance programme exists to catch.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::commands::batch::{self, LineMode, ResultBudget};
use crate::format::{single_line, INACTIVE_MARKER};
use crate::index::query::{Hit, Index};
use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, ProvenanceFlags};

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Keyword (FTS5) search - identical to `sct lexical`.
    Lexical(super::lexical::Args),
    /// Semantic similarity search (requires Ollama) - identical to `sct semantic`.
    Semantic(super::semantic::Args),
    /// Typo-tolerant search over the FST index - the same engine as `sct sayt --fuzzy`.
    Fuzzy(FuzzyArgs),
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Command::Lexical(a) => super::lexical::run(a),
        Command::Semantic(a) => super::semantic::run(a),
        Command::Fuzzy(a) => run_fuzzy(a),
    }
}

// ---------------------------------------------------------------------------
// `sct search fuzzy`
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
pub struct FuzzyArgs {
    /// Search query. Pass `-` to read one query per line from stdin.
    query: String,

    /// FST index produced by `sct fst build`.
    ///
    /// Defaults to `./snomed.fst`, then the newest `*.fst` in the working
    /// directory - `sct fst build` names its index after its input, so it is
    /// usually `<release>.fst`.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    index: Option<PathBuf>,

    /// Maximum number of results.
    #[arg(long, short, default_value = "10")]
    limit: u32,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,

    /// Emit only matching SCTIDs (newline-delimited) for piping.
    #[arg(long, conflicts_with = "format")]
    ids: bool,

    #[command(flatten)]
    prov: ProvenanceFlags,
}

fn open_index(path: Option<PathBuf>) -> Result<Index> {
    let index_path = match path {
        Some(p) => p,
        None => crate::paths::find_fst_index(std::path::Path::new(".")).ok_or_else(|| {
            anyhow::anyhow!(
                "No FST index found in the current directory.\n\
                 Build one with `sct fst build --ndjson <file>`, or pass --index <path>."
            )
        })?,
    };
    Index::open(&index_path).with_context(|| {
        format!(
            "opening FST index {} - build one with `sct fst build`",
            index_path.display()
        )
    })
}

fn run_fuzzy(args: FuzzyArgs) -> Result<()> {
    let index = open_index(args.index.clone())?;
    let prov = index.provenance().cloned();
    let mode = if args.format.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    if args.query == "-" {
        return run_fuzzy_batch(&index, &args, prov.as_ref(), show_prov);
    }

    let hits = index.search_typeahead(&args.query, args.limit as usize, true);

    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for h in &hits {
            writeln!(out, "{}", h.concept_id)?;
        }
        return Ok(());
    }

    if hits.is_empty() && !args.format.is_structured() {
        eprintln!("No results for {:?}", args.query);
        return Ok(());
    }

    if args.format.is_structured() {
        let items: Vec<_> = hits.iter().map(Hit::to_json).collect();
        let mut value = serde_json::json!({ "results": items });
        provenance::inject_into_json(&mut value, prov.as_ref(), show_prov);
        if let Some(s) = args.format.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    for h in &hits {
        println!("{}", render_hit_line(h));
    }
    provenance::print_human_footer(prov.as_ref(), show_prov);
    Ok(())
}

fn run_fuzzy_batch(
    index: &Index,
    args: &FuzzyArgs,
    prov: Option<&provenance::Provenance>,
    show_prov: bool,
) -> Result<()> {
    let queries = batch::read_stdin(LineMode::Whole, "fuzzy search")?;

    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let mut budget = ResultBudget::new();
        for query in &queries {
            let limit = budget.query_limit(Some(args.limit)) as usize;
            let hits = index.search_typeahead(query, limit, true);
            budget.retain(hits.len(), "fuzzy search")?;
            for h in &hits {
                writeln!(out, "{}", h.concept_id)?;
            }
        }
        return Ok(());
    }

    let mut items = Vec::with_capacity(queries.len());
    let mut budget = ResultBudget::new();
    for query in queries {
        let limit = budget.query_limit(Some(args.limit)) as usize;
        let hits = index.search_typeahead(&query, limit, true);
        budget.retain(hits.len(), "fuzzy search")?;
        items.push(batch::BatchItem::new(query, hits));
    }

    if args.format.is_structured() {
        let items: Vec<_> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "input": item.input,
                    "result": item.result.iter().map(Hit::to_json).collect::<Vec<_>>(),
                })
            })
            .collect();
        let mut value = serde_json::json!({ "items": items });
        provenance::inject_into_json(&mut value, prov, show_prov);
        args.format.print(&value)?;
        return Ok(());
    }

    for item in &items {
        if item.result.is_empty() {
            eprintln!("No results for {:?}", item.input);
        }
        for h in &item.result {
            println!("{}", render_hit_line(h));
        }
    }
    provenance::print_human_footer(prov, show_prov);
    Ok(())
}

/// One text-output line: `[marker]<id>  <term> (<tag>)`. There is no FSN or
/// hierarchy name to show - the FST index carries a preferred term and a
/// semantic tag parsed from it, nothing else - so this deliberately does not
/// reuse `ConceptFields`, whose `hierarchy` field means something this index
/// cannot answer. Mirrors `sct fst search`'s existing rendering.
fn render_hit_line(hit: &Hit) -> String {
    let tag = hit
        .semantic_tag
        .as_deref()
        .map(|t| format!(" ({t})"))
        .unwrap_or_default();
    let marker = if hit.active { "" } else { INACTIVE_MARKER };
    format!(
        "{marker}{:<18}  {}{}",
        hit.concept_id,
        single_line(&hit.term),
        single_line(&tag)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_hit_line_flags_inactive_with_the_shared_marker() {
        let active = Hit {
            concept_id: 22298006,
            term: "Myocardial infarction".into(),
            matched: String::new(),
            semantic_tag: Some("disorder".into()),
            score: 0.9,
            active: true,
        };
        let inactive = Hit {
            active: false,
            ..active.clone()
        };
        assert!(!render_hit_line(&active).starts_with(INACTIVE_MARKER));
        assert!(render_hit_line(&inactive).starts_with(INACTIVE_MARKER));
        assert!(render_hit_line(&active).contains("(disorder)"));
    }

    #[test]
    fn render_hit_line_has_no_tag_suffix_when_none() {
        let hit = Hit {
            concept_id: 1,
            term: "Term".into(),
            matched: String::new(),
            semantic_tag: None,
            score: 0.1,
            active: true,
        };
        assert!(!render_hit_line(&hit).contains('('));
    }
}
