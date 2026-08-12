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

use crate::humanize::{human_bytes, plural_count};
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
    ///
    /// Defaults to the input's name with a `.fst` extension
    /// (`uk-monolith-42.ndjson` → `uk-monolith-42.fst`), written to the working
    /// directory. Reading from stdin gives `snomed.fst`.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    output: Option<PathBuf>,

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
    ///
    /// Defaults to `./snomed.fst`, then the newest `*.fst` in the working
    /// directory - `sct fst build` names its index after its input, so it is
    /// usually `<release>.fst`.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    index: Option<PathBuf>,

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
    let output = crate::commands::resolve_output(
        args.output.as_deref(),
        &args.input,
        crate::paths::suffix::FST,
    );

    let (reader, pb) = crate::progress::ndjson_reader(&args.input)?;
    pb.set_message("Building FST index...");

    let mut out =
        std::fs::File::create(&output).with_context(|| format!("creating {}", output.display()))?;

    let opts = index::BuildOptions {
        include_terms: !args.no_terms,
    };

    let started = Instant::now();
    let stats = index::build_with_options(reader, &mut out, &opts)?;
    drop(out);
    let elapsed = started.elapsed();
    pb.finish_and_clear();

    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);

    eprintln!(
        "Built {} in {:.2}s",
        output.display(),
        elapsed.as_secs_f64()
    );
    eprintln!(
        "  {}, {} → {}, {}, {}",
        plural_count(stats.concepts as u64, "concept"),
        plural_count(stats.terms as u64, "term"),
        plural_count(stats.distinct_keys as u64, "distinct key"),
        plural_count(stats.distinct_words as u64, "word token"),
        plural_count(stats.semantic_tags as u64, "semantic tag")
    );
    let labels = if stats.terms_included {
        "with labels"
    } else {
        "no labels (--no-terms)"
    };
    eprintln!(
        "  {} on disk ({}), {labels}",
        human_bytes(size),
        plural_count(size, "byte")
    );
    Ok(())
}

fn search(args: SearchArgs) -> Result<()> {
    let index_path = match args.index {
        Some(p) => p,
        None => crate::paths::find_fst_index(std::path::Path::new(".")).ok_or_else(|| {
            anyhow::anyhow!(
                "No FST index found in the current directory.\n\
                 Build one with `sct fst build --ndjson <file>`, or pass --index <path>."
            )
        })?,
    };
    let idx = Index::open(&index_path)?;

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
            "{} in {:.3} ms",
            plural_count(hits.len() as u64, "result"),
            elapsed.as_secs_f64() * 1000.0
        );
        return Ok(());
    }

    if hits.is_empty() {
        eprintln!("No results for {:?}", args.query);
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
        // Same fixed marker as `sct lexical`; there is no `--template` here to
        // strip it from, but the prefix (not suffix) placement is kept
        // consistent so a retired concept always reads the same way.
        let marker = if h.active {
            ""
        } else {
            crate::format::INACTIVE_MARKER
        };
        println!("{marker}{:<18}  {}{}", h.concept_id, h.term, tag);
    }
    eprintln!(
        "\n{} in {:.3} ms",
        plural_count(hits.len() as u64, "result"),
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}
