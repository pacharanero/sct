// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct ecl expand` - evaluate a SNOMED CT ECL expression against the database
//! and emit the matching concept SCTIDs.
//!
//! Composable by design (see `spec/spec.md` - "Composability"): stdout is
//! newline-delimited SCTIDs, so it pipes straight into `sct codelist add <file> -`,
//! `sct lookup`, `jq`, or anything else. `--json` emits a JSON array instead.
//! The human-readable match count goes to stderr, keeping stdout clean for pipes.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{Read, Write};
use std::path::PathBuf;

use crate::output::OutputFormat;

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    command: EclCommand,
}

#[derive(Subcommand, Debug)]
enum EclCommand {
    /// Evaluate an ECL expression and print the matching concept SCTIDs.
    Expand(ExpandArgs),

    /// Refactor a set of SCTIDs into a compact ECL expression (inverse of expand).
    Compress(CompressArgs),
}

#[derive(Parser, Debug)]
struct ExpandArgs {
    /// ECL expression, e.g. `"<<73211009"`. Pass `-` to read it from stdin.
    expr: String,

    /// SNOMED CT SQLite database. See `docs/path-resolution.md` for the
    /// discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    db: Option<PathBuf>,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,

    /// Deprecated alias for `--format json`.
    #[arg(long, hide = true)]
    json: bool,
}

#[derive(Parser, Debug)]
struct CompressArgs {
    /// SCTIDs to compress. Pass `-` (or no ids) to read newline/whitespace-
    /// delimited ids from stdin. Mutually exclusive with `--codelist`.
    ids: Vec<String>,

    /// Compress the effective members of a `.codelist` file instead of ids.
    #[arg(long, conflicts_with = "ids", value_parser = crate::paths::tilde_pathbuf)]
    codelist: Option<PathBuf>,

    /// Registry directory for bare `includes:` ids when using `--codelist`
    /// (default `./codelists`, or `$SCT_CODELISTS` / `[codelists] dir`).
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    codelists: Option<PathBuf>,

    /// Emit only subsumption/exclusion clauses; do not paper over gaps with
    /// literal `OR`/`MINUS` residuals. Exits non-zero if the result is not exact.
    #[arg(long)]
    intensional_only: bool,

    /// Maximum number of `MINUS <<x` exclusion clauses before the remainder is
    /// handed to literal residuals.
    #[arg(long, default_value_t = 32)]
    max_exclusions: usize,

    /// Break the expression across indented lines.
    #[arg(long)]
    pretty: bool,

    /// Skip the re-expansion check that verifies exactness (verification is on
    /// by default and cheap).
    #[arg(long)]
    no_verify: bool,

    /// Print a compression report (clause counts, coverage) to stderr.
    #[arg(long)]
    stats: bool,

    /// Output format. `text` prints the ECL expression; `json`/`yaml` emit a
    /// structured object (expression plus include/exclude/residual breakdown).
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,

    /// SNOMED CT SQLite database. See `docs/path-resolution.md` for discovery.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    db: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        EclCommand::Expand(a) => expand(a),
        EclCommand::Compress(a) => compress(a),
    }
}

fn expand(args: ExpandArgs) -> Result<()> {
    let expr = if args.expr == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("reading ECL expression from stdin")?;
        s.trim().to_string()
    } else {
        args.expr
    };

    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = crate::commands::open_db_readonly(&db, None)?;
    crate::ecl::warn_if_no_tct(&conn);
    let ids = crate::ecl::expand(&conn, &expr)?;

    eprintln!("{} concept(s) matched {expr:?}", ids.len());

    let format = args.format.or_json_flag(args.json);
    if !format.print(&ids)? {
        // Buffer stdout: the raw lock is a LineWriter, which issues one write
        // syscall per line - 136k syscalls for a big expansion (measured ~99%
        // of the process's syscall count). BufWriter batches them into ~8 KiB
        // writes.
        let mut out = std::io::BufWriter::new(std::io::stdout().lock());
        for id in &ids {
            writeln!(out, "{id}")?;
        }
        out.flush()?;
    }
    Ok(())
}

fn compress(args: CompressArgs) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = crate::commands::open_db_readonly(&db, None)?;
    crate::ecl::warn_if_no_tct(&conn);

    // Gather the requested ids from --codelist, positional args, or stdin.
    let requested: Vec<String> = if let Some(path) = &args.codelist {
        let cl = crate::commands::codelist::read_codelist(path)?;
        let registry = crate::paths::codelist_registry(args.codelists.as_deref());
        crate::commands::codelist::effective_members_of(&cl, path, &registry, false)?
            .into_iter()
            .map(|m| m.id)
            .collect()
    } else {
        let mut raw: Vec<String> = args.ids.iter().filter(|s| *s != "-").cloned().collect();
        if args.ids.iter().any(|s| s == "-") || args.ids.is_empty() {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .context("reading SCTIDs from stdin")?;
            raw.extend(s.split_whitespace().map(str::to_string));
        }
        raw.iter().filter_map(|t| parse_sctid(t)).collect()
    };

    anyhow::ensure!(
        !requested.is_empty(),
        "no SCTIDs to compress (pass ids as arguments, on stdin, or via --codelist)"
    );

    // Keep only ids that exist and are active; report the rest. An id that
    // does not parse as u64 cannot be a valid SCTID, so it drops with the
    // unknowns rather than erroring the whole run.
    let mut target = crate::ecl::eval::IdSet::new();
    let mut dropped = Vec::new();
    {
        let mut stmt =
            conn.prepare_cached("SELECT 1 FROM concepts WHERE id = ?1 AND active = 1")?;
        for id in &requested {
            match id.parse::<u64>() {
                Ok(v) if stmt.query_row([id], |_| Ok(())).is_ok() => {
                    target.insert(v);
                }
                _ => dropped.push(id.clone()),
            }
        }
    }
    if !dropped.is_empty() {
        eprintln!(
            "note: dropped {} id(s) that are unknown or inactive: {}",
            dropped.len(),
            preview(&dropped)
        );
    }
    anyhow::ensure!(
        !target.is_empty(),
        "none of the requested ids are active concepts in this database"
    );

    let result = crate::ecl::compress::compress(
        &conn,
        &target,
        args.max_exclusions,
        !args.intensional_only,
        !args.no_verify,
    )?;

    if args.stats {
        let residuals = result.missing.len() + result.extra.len();
        eprint!(
            "— input {} id(s) → {} include + {} exclude clause(s)",
            target.len(),
            result.includes.len(),
            result.excludes.len(),
        );
        if residuals > 0 {
            eprint!(" (+{residuals} literal residual(s))");
        }
        if result.dropped_exclusions > 0 {
            eprint!(
                "; {} clean exclusion(s) dropped at --max-exclusions",
                result.dropped_exclusions
            );
        }
        eprintln!("; intensional coverage {:.1}%", result.coverage);
    }

    if args.format.is_structured() {
        let structured = serde_json::json!({
            "ecl": result.expr,
            "includes": result.includes,
            "excludes": result.excludes,
            "missing": result.missing,
            "extra": result.extra,
            "coverage": result.coverage,
            "exact": result.exact,
        });
        args.format.print(&structured)?;
    } else {
        let out_expr = if args.pretty {
            crate::ecl::compress::prettify(&result.expr)
        } else {
            result.expr.clone()
        };
        println!("{out_expr}");
    }

    // In intensional-only mode a non-exact result is a hard failure: the emitted
    // expression does not reproduce the input, and the user asked us not to fix it.
    if args.intensional_only && !result.exact {
        eprintln!(
            "warning: intensional form is not exact — missing {} id(s){}, extra {} id(s){}",
            result.missing.len(),
            suffix(&result.missing),
            result.extra.len(),
            suffix(&result.extra),
        );
        std::process::exit(1);
    }
    Ok(())
}

/// Extract a leading SCTID from an input word, tolerating a trailing `|term|`
/// or other trailing text (`73211009 |Diabetes|` → `73211009`). Returns `None`
/// for words that do not start with digits.
fn parse_sctid(word: &str) -> Option<String> {
    let digits: String = word
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// First few ids of a list, with an ellipsis when there are more.
fn preview(ids: &[String]) -> String {
    const N: usize = 8;
    if ids.len() <= N {
        ids.join(", ")
    } else {
        format!("{}, … ({} more)", ids[..N].join(", "), ids.len() - N)
    }
}

fn suffix(ids: &[String]) -> String {
    if ids.is_empty() {
        String::new()
    } else {
        format!(" [{}]", preview(ids))
    }
}
