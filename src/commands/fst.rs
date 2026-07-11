// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct fst` - build and query the FST-backed lexical index.
//!
//! Two subcommands:
//!   - `sct fst build  --ndjson snomed.ndjson --output snomed.fst`
//!   - `sct fst search --index snomed.fst <query> [--prefix | --fuzzy N | --words]`
//!
//! `build` mirrors `sct sqlite` / `sct parquet`: it consumes the canonical
//! NDJSON and emits a single artefact (default `snomed.fst`). `search` is here
//! so the prefix/fuzzy/word capabilities can be exercised from the CLI; the
//! benchmark (`benchmarks/fst_bench.rs`) drives the same query paths in-process.
//!
//! See `spec/commands/fst.md`.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Instant;

use crate::index::{self, Index};

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    command: FstCommand,
}

#[derive(Subcommand, Debug)]
enum FstCommand {
    /// Build a `snomed.fst` index from a SNOMED CT NDJSON artefact.
    Build(BuildArgs),
    /// Query an existing `snomed.fst` index.
    Search(SearchArgs),
}

#[derive(Parser, Debug)]
struct BuildArgs {
    /// NDJSON artefact produced by `sct ndjson`. Use `-` for stdin.
    #[arg(
        long = "ndjson",
        alias = "input",
        short = 'i',
        value_hint = clap::ValueHint::FilePath,
        value_name = "NDJSON",
        value_parser = crate::paths::tilde_pathbuf
    )]
    input: PathBuf,

    /// Output index file.
    #[arg(long, short, default_value = "snomed.fst", value_parser = crate::paths::tilde_pathbuf)]
    output: PathBuf,

    /// Omit the display side-tables (preferred-term labels). Produces a smaller
    /// index for use alongside SQLite, where labels are resolved from the DB.
    /// `sct fst search` on such an index returns SCTIDs without labels.
    #[arg(long)]
    no_terms: bool,
}

#[derive(Parser, Debug)]
struct SearchArgs {
    /// The query term or words.
    query: String,

    /// Index file produced by `sct fst build`.
    #[arg(long, default_value = "snomed.fst", value_parser = crate::paths::tilde_pathbuf)]
    index: PathBuf,

    /// Prefix (autocomplete) search instead of exact match.
    #[arg(long, conflicts_with_all = ["fuzzy", "words"])]
    prefix: bool,

    /// Fuzzy search up to N edits (Levenshtein distance 1 or 2).
    #[arg(long, value_name = "N", conflicts_with_all = ["prefix", "words"])]
    fuzzy: Option<u32>,

    /// Word-intersection search: whitespace-split the query, return concepts
    /// whose terms contain every word.
    #[arg(long, conflicts_with_all = ["prefix", "fuzzy"])]
    words: bool,

    /// Maximum number of results.
    #[arg(long, short, default_value = "10")]
    limit: usize,

    /// Emit only matching SCTIDs (newline-delimited) for piping.
    #[arg(long)]
    ids: bool,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        FstCommand::Build(a) => build(a),
        FstCommand::Search(a) => search(a),
    }
}

fn build(args: BuildArgs) -> Result<()> {
    let (reader, pb) = crate::progress::ndjson_reader(&args.input)?;
    pb.set_message("Building FST index...");

    let mut out = std::fs::File::create(&args.output)
        .with_context(|| format!("creating {}", args.output.display()))?;

    let opts = index::BuildOptions {
        include_terms: !args.no_terms,
    };

    let started = Instant::now();
    let stats = index::build_with_options(reader, &mut out, &opts)?;
    drop(out);
    let elapsed = started.elapsed();
    pb.finish_and_clear();

    let size = std::fs::metadata(&args.output)
        .map(|m| m.len())
        .unwrap_or(0);

    eprintln!(
        "Built {} in {:.2}s",
        args.output.display(),
        elapsed.as_secs_f64()
    );
    eprintln!(
        "  {} concepts, {} terms → {} distinct keys, {} word tokens, {} semantic tags",
        stats.concepts, stats.terms, stats.distinct_keys, stats.distinct_words, stats.semantic_tags
    );
    let labels = if stats.terms_included {
        "with labels"
    } else {
        "no labels (--no-terms)"
    };
    eprintln!("  {} on disk ({} bytes), {labels}", human_bytes(size), size);
    Ok(())
}

fn search(args: SearchArgs) -> Result<()> {
    let idx = Index::open(&args.index)?;

    let started = Instant::now();
    let hits = if args.words {
        let words: Vec<&str> = args.query.split_whitespace().collect();
        idx.lookup_words(&words, args.limit)
    } else if let Some(dist) = args.fuzzy {
        idx.lookup_fuzzy(&args.query, dist, args.limit)?
    } else if args.prefix {
        idx.lookup_prefix(&args.query, args.limit)?
    } else {
        idx.lookup_exact(&args.query)
    };
    let elapsed = started.elapsed();

    // `--ids`: machine output for pipes - SCTIDs on stdout, timing on stderr.
    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for h in &hits {
            writeln!(out, "{}", h.concept_id)?;
        }
        eprintln!(
            "{} result(s) in {:.3} ms",
            hits.len(),
            elapsed.as_secs_f64() * 1000.0
        );
        return Ok(());
    }

    if hits.is_empty() {
        println!("No results for {:?}", args.query);
        return Ok(());
    }

    if !idx.has_terms() {
        eprintln!("note: index built with --no-terms; results have no labels");
    }

    for h in &hits {
        let tag = h
            .semantic_tag
            .as_deref()
            .map(|t| format!(" ({t})"))
            .unwrap_or_default();
        println!("{:<18}  {}{}", h.concept_id, h.term, tag);
    }
    eprintln!(
        "\n{} result(s) in {:.3} ms",
        hits.len(),
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}
