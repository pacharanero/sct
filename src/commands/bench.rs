// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct bench` - self-benchmark the local artefacts through the SDK and CLI
//! boundaries.
//!
//! One command, no repository clone, no Docker, no `oha`: it times the
//! operations a user actually performs (lookup, lexical search, children,
//! ancestors, subsumption, ECL expansion, FST prefix search) against their own
//! database, and renders the result for a terminal, a forum post, a standalone
//! HTML file, or a machine.
//!
//! Three profiles (see `spec/commands/bench.md` §3.1):
//!
//! - `sdk` - in-process through [`crate::sdk::Snomed`], warm cache.
//! - `cli` - the same operation through a subprocess of the *running* binary,
//!   so the measurement includes process spawn, argument parsing, and database
//!   open. The `sdk`/`cli` difference is the startup cost, which is what an
//!   in-process benchmark cannot see.
//! - `artefact` - static inspection of artefact sizes and presence. Not timed.
//!
//! `--pipeline <RF2>` additionally times a full build (`sct ndjson`, then
//! `sct sqlite`, then `sct fst build`) into a temporary directory that is
//! removed afterwards.
//!
//! Cases are embedded here rather than read from `benchmarks/scenarios/`: the
//! user has no repository. When the typed scenario corpus lands with `R48`
//! these definitions become its shipped subset. Every case declares the
//! concepts and artefacts it needs and is **skipped and reported as skipped**
//! when they are absent - never silently dropped, never timed against a
//! missing row.
//!
//! Privacy: no output in any format carries an absolute path, a hostname, a
//! username, or a credential. The database is identified by file name only,
//! errors are reduced to fixed classification strings rather than messages
//! that might embed a path, and nothing is uploaded anywhere.

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::humanize::{fmt_count, human_bytes};
use crate::sdk::Snomed;

/// Version of the shared result schema emitted by `--format json`. Bump when a
/// field changes meaning; additive fields do not need a bump.
pub const RESULT_SCHEMA_VERSION: u32 = 1;

/// Percentage band inside which a `--baseline` delta is called noise rather
/// than a regression or an improvement.
const NOISE_BAND_PCT: f64 = 15.0;

/// Absolute floor below which a `--baseline` delta is called noise whatever the
/// percentage says. A relative band alone misreads fast operations: an in-process
/// lookup moving 0.045 ms -> 0.114 ms is +152%, but both figures are close enough
/// to timer granularity and scheduler jitter that the percentage is meaningless.
/// Reporting that as "slower" in output designed to be pasted into a bug report
/// would send someone chasing a regression that does not exist.
const NOISE_FLOOR_NS: u64 = 500_000; // 0.5 ms

/// Sampling defaults from `spec/commands/bench.md` §3.3.
const DEFAULT_WARMUP: usize = 3;
const DEFAULT_SAMPLES: usize = 10;
/// `--full` scales the sampling up rather than changing the method.
const FULL_WARMUP: usize = 5;
const FULL_SAMPLES: usize = 30;

// ─── CLI surface ──────────────────────────────────────────────────────────────

/// Output format for the benchmark report.
///
/// Deliberately *not* the shared [`crate::output::OutputFormat`]: this command
/// has no useful YAML rendering and does have Markdown and standalone HTML
/// renderings, so it carries its own enum in the same way `sct diagram` does
/// for its tree/DOT/Mermaid targets.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum BenchFormat {
    /// Human-readable terminal report (default).
    Text,
    /// Tables plus a fenced environment block, for a GitHub issue or a forum post.
    Markdown,
    /// Canonical machine form: the shared result schema, with raw samples.
    Json,
    /// Standalone HTML file with inline CSS and no external requests.
    Html,
}

/// A measurement boundary.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Profile {
    /// In-process through the SDK, warm cache.
    Sdk,
    /// Through a subprocess of this binary: spawn, parse, open, query, print.
    Cli,
    /// Static inspection of artefact sizes and presence. Not timed.
    Artefact,
}

impl Profile {
    fn as_str(self) -> &'static str {
        match self {
            Profile::Sdk => "sdk",
            Profile::Cli => "cli",
            Profile::Artefact => "artefact",
        }
    }
}

#[derive(Parser, Debug)]
pub struct Args {
    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Comma-separated measurement profiles to run.
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        value_name = "LIST",
        default_values_t = [Profile::Sdk, Profile::Cli, Profile::Artefact],
    )]
    pub profiles: Vec<Profile>,

    /// Longer run: more samples, plus deeper hierarchy and ECL cases.
    #[arg(long)]
    pub full: bool,

    /// Also time a full build from this RF2 zip or directory. Runs
    /// `sct ndjson`, `sct sqlite`, and `sct fst build` once each into a
    /// temporary directory, which is removed afterwards.
    #[arg(long, value_name = "RF2", value_parser = crate::paths::tilde_pathbuf)]
    pub pipeline: Option<PathBuf>,

    /// Override the per-case sample count.
    #[arg(long, value_name = "N")]
    pub samples: Option<usize>,

    /// Override the per-case warm-up count.
    #[arg(long, value_name = "N")]
    pub warmup: Option<usize>,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = BenchFormat::Text, value_name = "FMT")]
    pub format: BenchFormat,

    /// Write the report to a file instead of stdout. Required for `--format
    /// html` unless stdout is redirected.
    #[arg(long, short = 'o', value_parser = crate::paths::tilde_pathbuf)]
    pub output: Option<PathBuf>,

    /// Compare against a previous `--format json` result and show per-case
    /// deltas, flagging changes outside a ±15% noise band.
    #[arg(long, value_name = "PATH", value_parser = crate::paths::tilde_pathbuf)]
    pub baseline: Option<PathBuf>,

    /// Omit dataset release identity (edition, release date, release id) for
    /// users who consider their edition licensing sensitive. Concept count and
    /// schema version are still shown; they identify no release.
    #[arg(long)]
    pub no_provenance: bool,
}

pub fn run(args: Args) -> Result<()> {
    let samples = args.samples.unwrap_or(if args.full {
        FULL_SAMPLES
    } else {
        DEFAULT_SAMPLES
    });
    let warmup = args.warmup.unwrap_or(if args.full {
        FULL_WARMUP
    } else {
        DEFAULT_WARMUP
    });
    if samples == 0 {
        bail!("--samples must be at least 1: a benchmark with no samples measures nothing");
    }

    if args.format == BenchFormat::Html && args.output.is_none() {
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() {
            bail!(
                "--format html writes a whole HTML document; give --output <PATH> \
                 or redirect stdout to a file"
            );
        }
    }

    let db_path = crate::paths::resolve_db(args.db.as_deref())?.path;
    let db_dir = containing_dir(&db_path);
    let mut snomed = Snomed::open(&db_path)?;

    // An index next door is only usable if the SDK accepts it as belonging to
    // this database (matching release and content fingerprint). One that is
    // absent, unreadable, or paired with a different release is not fatal: the
    // FST cases report themselves as skipped, with the reason.
    let fst_file = crate::paths::find_fst_index(&db_dir).filter(|p| p.is_file());
    let mut fst_usable: Option<PathBuf> = None;
    let mut fst_skip_reason =
        "no FST index alongside this database (build one with `sct fst build`)".to_string();
    if let Some(path) = &fst_file {
        match snomed.attach_fst(path) {
            Ok(()) => fst_usable = Some(path.clone()),
            Err(_) => {
                fst_skip_reason =
                    "the FST index beside this database was built from a different release or \
                     content and cannot be measured against it"
                        .to_string()
            }
        }
    }

    let mut profiles = args.profiles.clone();
    profiles.sort_unstable();
    profiles.dedup();

    let exe =
        std::env::current_exe().context("locating the running sct binary for the cli profile")?;

    let runner = Runner {
        snomed: &snomed,
        exe,
        db: db_path.clone(),
        fst: fst_usable,
        fst_skip_reason,
        profiles: profiles.clone(),
        sampling: Sampling { warmup, samples },
    };

    // `artefact` is static inspection, so with neither `sdk` nor `cli` selected
    // there is nothing to time and the scenario set is not worth walking.
    let timed = profiles
        .iter()
        .any(|p| matches!(p, Profile::Sdk | Profile::Cli));
    let cases = if timed {
        default_cases(args.full)
    } else {
        Vec::new()
    };
    // Progress is a hint, not data: only draw it on an interactive stderr, so a
    // redirected or piped run stays clean.
    let progress = {
        use std::io::IsTerminal;
        std::io::stderr().is_terminal()
    };
    let mut results = Vec::with_capacity(cases.len());
    for case in &cases {
        if progress {
            eprint!("\r  running {:<32}", case.label);
        }
        results.push(runner.run_case(case));
    }
    if progress {
        eprint!("\r{:<42}\r", "");
    }

    let artefacts = if profiles.contains(&Profile::Artefact) {
        Some(inspect_artefacts(&db_path, &db_dir, fst_file.as_deref()))
    } else {
        None
    };

    let dataset = dataset_info(&snomed, &db_path, artefacts, args.no_provenance)?;

    let pipeline = match &args.pipeline {
        Some(rf2) => Some(run_pipeline(&runner.exe, rf2, progress)?),
        None => None,
    };

    let mut report = Report {
        schema_version: RESULT_SCHEMA_VERSION,
        run: RunInfo {
            run_id: run_id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            tool: "sct bench".to_string(),
            sct_version: env!("CARGO_PKG_VERSION").to_string(),
            git_commit: option_env!("SCT_GIT_COMMIT").map(str::to_string),
        },
        host: host_info(),
        dataset,
        policy: PolicyInfo {
            profiles: profiles.iter().map(|p| p.as_str().to_string()).collect(),
            warmup,
            samples,
            full: args.full,
            cache_mode: "warm".to_string(),
            concurrency: 1,
            timer: "monotonic (std::time::Instant)".to_string(),
            noise_band_pct: NOISE_BAND_PCT,
        },
        cases: results,
        pipeline,
        baseline: Vec::new(),
    };

    if let Some(path) = &args.baseline {
        report.baseline = compare_baseline(path, &report)?;
    }

    let rendered = match args.format {
        BenchFormat::Text => render_text(&report),
        BenchFormat::Markdown => render_markdown(&report),
        BenchFormat::Json => serde_json::to_string_pretty(&report)? + "\n",
        BenchFormat::Html => render_html(&report),
    };

    match &args.output {
        Some(path) => {
            std::fs::write(path, rendered)
                .with_context(|| format!("writing report to {}", path.display()))?;
            eprintln!("Wrote {}", path.display());
        }
        None => print!("{rendered}"),
    }
    Ok(())
}

// ─── Cases ────────────────────────────────────────────────────────────────────

/// What a case actually does, at both boundaries.
#[derive(Debug, Clone)]
enum CaseKind {
    Lookup { id: String },
    Search { query: String, limit: u32 },
    Children { id: String, limit: u32 },
    Ancestors { id: String },
    Subsumes { left: String, right: String },
    Ecl { expr: String },
    FstPrefix { prefix: String, limit: usize },
}

impl CaseKind {
    fn operation(&self) -> &'static str {
        match self {
            CaseKind::Lookup { .. } => "lookup",
            CaseKind::Search { .. } => "lexical_search",
            CaseKind::Children { .. } => "children",
            CaseKind::Ancestors { .. } => "ancestors",
            CaseKind::Subsumes { .. } => "subsumption",
            CaseKind::Ecl { .. } => "ecl_expand",
            CaseKind::FstPrefix { .. } => "fst_prefix",
        }
    }

    /// A short, path-free description of the input, for the result file.
    fn input_summary(&self) -> String {
        match self {
            CaseKind::Lookup { id } => id.clone(),
            CaseKind::Search { query, limit } => format!("\"{query}\", limit {limit}"),
            CaseKind::Children { id, limit } => format!("{id}, limit {limit}"),
            CaseKind::Ancestors { id } => id.clone(),
            CaseKind::Subsumes { left, right } => format!("{left} ⊒ {right}"),
            CaseKind::Ecl { expr } => expr.clone(),
            CaseKind::FstPrefix { prefix, limit } => format!("\"{prefix}\", limit {limit}"),
        }
    }
}

/// One embedded scenario: what to run, what it needs, and what to call it.
#[derive(Debug, Clone)]
struct Case {
    id: &'static str,
    label: String,
    kind: CaseKind,
    /// Concepts that must exist in the database for the case to be honest.
    requires_concepts: Vec<String>,
    /// Whether the case needs an FST index alongside the database.
    requires_fst: bool,
}

impl Case {
    fn new(id: &'static str, label: impl Into<String>, kind: CaseKind) -> Self {
        Self {
            id,
            label: label.into(),
            kind,
            requires_concepts: Vec::new(),
            requires_fst: false,
        }
    }

    fn needs(mut self, ids: &[&str]) -> Self {
        self.requires_concepts = ids.iter().map(|s| (*s).to_string()).collect();
        self
    }

    fn needs_fst(mut self) -> Self {
        self.requires_fst = true;
        self
    }
}

/// The shipped scenario set.
///
/// Concepts are chosen to be present in every SNOMED CT edition (and in the
/// committed synthetic fixture): `138875005` is the root, `22298006`
/// (Myocardial infarction), `73211009` (Diabetes mellitus), `46635009`
/// (Type 1 diabetes mellitus), and `404684003` (Clinical finding) are
/// International core.
fn default_cases(full: bool) -> Vec<Case> {
    let mut cases = vec![
        Case::new(
            "lookup_sctid",
            "lookup by SCTID",
            CaseKind::Lookup {
                id: "22298006".into(),
            },
        )
        .needs(&["22298006"]),
        Case::new(
            "lexical_search",
            "lexical search \"heart\"",
            CaseKind::Search {
                query: "heart".into(),
                limit: 10,
            },
        ),
        Case::new(
            "children",
            "children",
            CaseKind::Children {
                id: "73211009".into(),
                limit: 100,
            },
        )
        .needs(&["73211009"]),
        Case::new(
            "ancestors",
            "ancestors",
            CaseKind::Ancestors {
                id: "22298006".into(),
            },
        )
        .needs(&["22298006"]),
        Case::new(
            "subsumption",
            "subsumption test",
            CaseKind::Subsumes {
                left: "73211009".into(),
                right: "46635009".into(),
            },
        )
        .needs(&["73211009", "46635009"]),
        Case::new(
            "ecl_descendants",
            "ECL <<73211009",
            CaseKind::Ecl {
                expr: "<<73211009".into(),
            },
        )
        .needs(&["73211009"]),
        Case::new(
            "fst_prefix",
            "FST prefix \"myoca\"",
            CaseKind::FstPrefix {
                prefix: "myoca".into(),
                limit: 10,
            },
        )
        .needs_fst(),
    ];

    if full {
        cases.push(
            Case::new(
                "ecl_broad",
                "ECL <<404684003 (deep)",
                CaseKind::Ecl {
                    expr: "<<404684003".into(),
                },
            )
            .needs(&["404684003"]),
        );
        cases.push(
            Case::new(
                "children_high_fanout",
                "children (high fan-out)",
                CaseKind::Children {
                    id: "404684003".into(),
                    limit: 1000,
                },
            )
            .needs(&["404684003"]),
        );
    }

    cases
}

// ─── Running ──────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug)]
struct Sampling {
    warmup: usize,
    samples: usize,
}

struct Runner<'a> {
    snomed: &'a Snomed,
    exe: PathBuf,
    db: PathBuf,
    /// The FST index the SDK accepted for this database, if any.
    fst: Option<PathBuf>,
    /// Why there is no usable index, reported on the cases that need one.
    fst_skip_reason: String,
    profiles: Vec<Profile>,
    sampling: Sampling,
}

impl Runner<'_> {
    fn run_case(&self, case: &Case) -> CaseResult {
        let mut result = CaseResult {
            id: case.id.to_string(),
            label: case.label.clone(),
            operation: case.kind.operation().to_string(),
            input: case.kind.input_summary(),
            status: "ok".to_string(),
            skipped_reason: None,
            profiles: BTreeMap::new(),
        };

        // Degrade honestly: a case whose concepts (or index) are absent is
        // skipped and reported, never timed against a missing row.
        for id in &case.requires_concepts {
            let present = matches!(self.snomed.concept(id), Ok(Some(_)));
            if !present {
                result.status = "skipped".into();
                result.skipped_reason =
                    Some(format!("concept {id} is not present in this database"));
                return result;
            }
        }
        if case.requires_fst && self.fst.is_none() {
            result.status = "skipped".into();
            result.skipped_reason = Some(self.fst_skip_reason.clone());
            return result;
        }

        if self.profiles.contains(&Profile::Sdk) {
            result
                .profiles
                .insert("sdk".to_string(), self.measure_sdk(case));
        }
        if self.profiles.contains(&Profile::Cli) {
            result
                .profiles
                .insert("cli".to_string(), self.measure_cli(case));
        }
        result
    }

    fn measure_sdk(&self, case: &Case) -> ProfileResult {
        // Validate once outside the timed region: a failing operation must not
        // be reported as a fast one.
        if run_sdk_case(self.snomed, &case.kind).is_err() {
            return ProfileResult::unavailable(format!(
                "the {} operation failed against this database",
                case.kind.operation()
            ));
        }
        let samples = measure(self.sampling, || {
            run_sdk_case(self.snomed, &case.kind)
                .err()
                .map(|_| "sdk_error")
        });
        ProfileResult::measured(samples)
    }

    fn measure_cli(&self, case: &Case) -> ProfileResult {
        let Some(argv) = cli_args(&case.kind, &self.db, self.fst.as_deref()) else {
            return ProfileResult::unavailable(
                "no equivalent single CLI invocation for this operation".into(),
            );
        };
        if !spawn_ok(&self.exe, &argv) {
            return ProfileResult::unavailable(format!(
                "`sct {}` exited non-zero against this database",
                case.kind.operation()
            ));
        }
        let samples = measure(self.sampling, || {
            if spawn_ok(&self.exe, &argv) {
                None
            } else {
                Some("nonzero_exit")
            }
        });
        ProfileResult::measured(samples)
    }
}

fn run_sdk_case(snomed: &Snomed, kind: &CaseKind) -> Result<()> {
    match kind {
        CaseKind::Lookup { id } => {
            snomed.concept(id)?;
        }
        CaseKind::Search { query, limit } => {
            snomed.search(query, *limit)?;
        }
        CaseKind::Children { id, limit } => {
            snomed.children(id, *limit)?;
        }
        CaseKind::Ancestors { id } => {
            snomed.ancestors(id)?;
        }
        CaseKind::Subsumes { left, right } => {
            snomed.subsumes(left, right)?;
        }
        CaseKind::Ecl { expr } => {
            snomed.expand(expr)?;
        }
        CaseKind::FstPrefix { prefix, limit } => {
            snomed.fst_prefix(prefix, *limit)?;
        }
    }
    Ok(())
}

/// The single CLI invocation equivalent to `kind`, or `None` when the operation
/// has no one-command CLI form.
///
/// `children`, `ancestors`, and `subsumption` go through `sct ecl expand`,
/// which is the CLI's expression of exactly those hierarchy relations (`<!`,
/// `>`, and a `<<left AND right` conjunction that is non-empty precisely when
/// `left` subsumes `right`).
fn cli_args(kind: &CaseKind, db: &Path, fst: Option<&Path>) -> Option<Vec<OsString>> {
    let db_flag = |args: &mut Vec<OsString>| {
        args.push("--db".into());
        args.push(db.as_os_str().to_os_string());
    };
    let mut args: Vec<OsString> = Vec::new();
    match kind {
        CaseKind::Lookup { id } => {
            args.push("lookup".into());
            args.push(id.into());
            db_flag(&mut args);
        }
        CaseKind::Search { query, limit } => {
            args.push("lexical".into());
            args.push(query.into());
            args.push("--limit".into());
            args.push(limit.to_string().into());
            db_flag(&mut args);
        }
        CaseKind::Children { id, .. } => {
            args.push("ecl".into());
            args.push("expand".into());
            args.push(format!("<!{id}").into());
            db_flag(&mut args);
        }
        CaseKind::Ancestors { id } => {
            args.push("ecl".into());
            args.push("expand".into());
            args.push(format!(">{id}").into());
            db_flag(&mut args);
        }
        CaseKind::Subsumes { left, right } => {
            args.push("ecl".into());
            args.push("expand".into());
            args.push(format!("<<{left} AND {right}").into());
            db_flag(&mut args);
        }
        CaseKind::Ecl { expr } => {
            args.push("ecl".into());
            args.push("expand".into());
            args.push(expr.into());
            db_flag(&mut args);
        }
        CaseKind::FstPrefix { prefix, limit } => {
            let index = fst?;
            args.push("fst".into());
            args.push("search".into());
            args.push(prefix.into());
            args.push("--prefix".into());
            args.push("--limit".into());
            args.push(limit.to_string().into());
            args.push("--index".into());
            args.push(index.as_os_str().to_os_string());
        }
    }
    Some(args)
}

/// Run one subprocess of the current binary, discarding its output, and report
/// only whether it succeeded. Output is discarded because writing to a pipe
/// nobody reads would otherwise be part of what is measured.
fn spawn_ok(exe: &Path, argv: &[OsString]) -> bool {
    Command::new(exe)
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Warm up, then take `samples` timed runs. `op` returns `None` on success or a
/// fixed error class on failure - never a message, which could embed a path.
fn measure(sampling: Sampling, mut op: impl FnMut() -> Option<&'static str>) -> Vec<Sample> {
    for _ in 0..sampling.warmup {
        let _ = op();
    }
    let mut out = Vec::with_capacity(sampling.samples);
    for _ in 0..sampling.samples {
        let started = Instant::now();
        let failure = op();
        let elapsed_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        out.push(Sample {
            elapsed_ns,
            ok: failure.is_none(),
            error_class: failure.map(str::to_string),
        });
    }
    out
}

// ─── Summary arithmetic ───────────────────────────────────────────────────────

/// Aggregate raw samples. Written once and shared by every profile so the
/// numbers in a `sdk` row and a `cli` row cannot be computed differently.
///
/// Only successful samples contribute to the timings; the failure rate is
/// reported separately rather than being averaged into them. Returns `None`
/// when no sample succeeded.
fn summarize(samples: &[Sample]) -> Option<Summary> {
    let total = samples.len();
    if total == 0 {
        return None;
    }
    let mut ok: Vec<u64> = samples
        .iter()
        .filter(|s| s.ok)
        .map(|s| s.elapsed_ns)
        .collect();
    if ok.is_empty() {
        return None;
    }
    ok.sort_unstable();

    let n = ok.len();
    let mean = ok.iter().map(|v| *v as f64).sum::<f64>() / n as f64;
    let variance = if n > 1 {
        ok.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / (n - 1) as f64
    } else {
        0.0
    };

    Some(Summary {
        samples: n,
        median_ns: median(&ok),
        mean_ns: mean.round() as u64,
        std_dev_ns: variance.sqrt().round() as u64,
        min_ns: ok[0],
        max_ns: ok[n - 1],
        p95_ns: percentile(&ok, 95.0),
        p99_ns: percentile(&ok, 99.0),
        error_rate: (total - n) as f64 / total as f64,
    })
}

/// Median of a sorted, non-empty slice. Even lengths average the two middles.
fn median(sorted: &[u64]) -> u64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2
    }
}

/// Nearest-rank percentile of a sorted, non-empty slice.
fn percentile(sorted: &[u64], pct: f64) -> u64 {
    let rank = (pct / 100.0 * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

// ─── Host, dataset, artefacts ─────────────────────────────────────────────────

/// Best-effort machine description, without adding a dependency.
///
/// `/proc` on Linux, `sysctl` on macOS, `"unknown"` everywhere else. Nothing
/// here identifies a person or a filesystem: no hostname, no user, no paths.
/// Extending it for another platform means adding one arm to each helper.
fn host_info() -> HostInfo {
    HostInfo {
        os: std::env::consts::OS.to_string(),
        architecture: std::env::consts::ARCH.to_string(),
        cpu: cpu_model().unwrap_or_else(|| "unknown".to_string()),
        logical_cores: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        memory_bytes: total_memory_bytes(),
    }
}

fn cpu_model() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        for line in text.lines() {
            // "model name" on x86, "Model" / "Hardware" on many Arm boards.
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            if key == "model name" || key == "Model" || key == "Hardware" {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        sysctl("machdep.cpu.brand_string")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn total_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        sysctl("hw.memsize")?.trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn sysctl(key: &str) -> Option<String> {
    let out = Command::new("sysctl").args(["-n", key]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// The directory holding `path`. A bare relative filename (`--db fixture.db`)
/// has an empty parent, which is not a readable directory, so it becomes `.`.
fn containing_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Sizes and presence of the artefacts sitting next to the database.
fn inspect_artefacts(db: &Path, db_dir: &Path, fst: Option<&Path>) -> Artefacts {
    Artefacts {
        database_bytes: file_bytes(db).unwrap_or(0),
        fst_bytes: fst.and_then(file_bytes),
        embeddings_bytes: find_embeddings(db, db_dir).as_deref().and_then(file_bytes),
    }
}

fn file_bytes(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// The embeddings file `sct embed` would have written beside this database.
fn find_embeddings(db: &Path, dir: &Path) -> Option<PathBuf> {
    let stem = db.file_stem()?.to_string_lossy().into_owned();
    for name in [
        format!("{stem}{}", crate::paths::suffix::EMBEDDINGS),
        format!("snomed{}", crate::paths::suffix::EMBEDDINGS),
    ] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn dataset_info(
    snomed: &Snomed,
    db: &Path,
    artefacts: Option<Artefacts>,
    no_provenance: bool,
) -> Result<DatasetInfo> {
    let conn = snomed.connection();
    let concept_count: Option<u64> = conn
        .query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get::<_, i64>(0))
        .ok()
        .map(|n| n as u64);
    let schema_version: Option<u32> = conn
        .query_row("SELECT schema_version FROM concepts LIMIT 1", [], |r| {
            r.get::<_, i64>(0)
        })
        .ok()
        .map(|v| v as u32);

    // Only the *name* of the database is ever reported: an absolute path leaks
    // the user's home directory and often their username.
    let database_file = db
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "database".to_string());

    let prov = snomed.provenance().filter(|_| !no_provenance);
    Ok(DatasetInfo {
        database_file,
        concept_count,
        schema_version,
        edition: prov
            .map(|p| p.edition_label.clone())
            .filter(|s| !s.is_empty()),
        release_date: prov
            .map(|p| p.release_date.clone())
            .filter(|s| !s.is_empty()),
        release_id: prov.map(|p| p.release_id.clone()).filter(|s| !s.is_empty()),
        provenance_suppressed: no_provenance,
        transitive_closure: snomed.has_transitive_closure(),
        artefacts,
    })
}

fn run_id() -> String {
    format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        std::process::id()
    )
}

// ─── Pipeline ─────────────────────────────────────────────────────────────────

/// Time a full build from an RF2 input, once per stage, into a scoped temporary
/// directory that is removed when it drops.
///
/// Not sampled: a build takes minutes on a real release, and the interesting
/// number is the wall clock a user would actually wait, not its distribution.
fn run_pipeline(exe: &Path, rf2: &Path, progress: bool) -> Result<PipelineResult> {
    let tmp = tempfile::TempDir::new().context("creating a temporary directory for --pipeline")?;
    let ndjson = tmp.path().join("pipeline.ndjson");
    let db = tmp.path().join("pipeline.db");
    let fst = tmp.path().join("pipeline.fst");

    let stages: Vec<(&str, Vec<OsString>)> = vec![
        (
            "ndjson",
            vec![
                "ndjson".into(),
                "--rf2".into(),
                rf2.as_os_str().to_os_string(),
                "--output".into(),
                ndjson.as_os_str().to_os_string(),
            ],
        ),
        (
            "sqlite",
            vec![
                "sqlite".into(),
                "--ndjson".into(),
                ndjson.as_os_str().to_os_string(),
                "--output".into(),
                db.as_os_str().to_os_string(),
            ],
        ),
        (
            "fst build",
            vec![
                "fst".into(),
                "build".into(),
                "--ndjson".into(),
                ndjson.as_os_str().to_os_string(),
                "--output".into(),
                fst.as_os_str().to_os_string(),
            ],
        ),
    ];

    let mut out = Vec::with_capacity(stages.len());
    for (name, argv) in stages {
        if progress {
            eprint!("\r  pipeline: {name:<24}");
        }
        let started = Instant::now();
        let ok = spawn_ok(exe, &argv);
        let elapsed_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        out.push(PipelineStage {
            stage: name.to_string(),
            elapsed_ns,
            ok,
        });
        if !ok {
            // A failed stage cannot be timed as a success, and every later
            // stage consumes its output. Stop and report what happened.
            eprintln!();
            bail!("`sct {name}` failed during --pipeline; the remaining stages were not run");
        }
    }
    if progress {
        eprint!("\r{:<36}\r", "");
    }

    Ok(PipelineResult {
        // The input is identified by name only, never by path.
        source: rf2
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "rf2 input".to_string()),
        stages: out,
    })
}

// ─── Baseline comparison ──────────────────────────────────────────────────────

/// Compare medians against a previous `--format json` run, per case and profile.
///
/// Anything inside ±[`NOISE_BAND_PCT`], *or* moving by less than
/// [`NOISE_FLOOR_NS`] in absolute terms, is called `noise`: a single run on an
/// uncontrolled machine cannot distinguish a 4% change from the weather, and a
/// large percentage swing on a sub-millisecond operation is jitter rather than
/// a real change.
fn compare_baseline(path: &Path, current: &Report) -> Result<Vec<BaselineDelta>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading baseline result {}", path.display()))?;
    let baseline: Report = serde_json::from_str(&text).with_context(|| {
        format!(
            "parsing {} as an `sct bench --format json` result",
            path.display()
        )
    })?;

    let mut deltas = Vec::new();
    for case in &current.cases {
        let Some(previous) = baseline.cases.iter().find(|c| c.id == case.id) else {
            continue;
        };
        for (profile, now) in &case.profiles {
            let (Some(before), Some(after)) = (
                previous.profiles.get(profile).and_then(|p| p.summary),
                now.summary,
            ) else {
                continue;
            };
            if before.median_ns == 0 {
                continue;
            }
            let change_pct = (after.median_ns as f64 - before.median_ns as f64)
                / before.median_ns as f64
                * 100.0;
            let absolute_delta_ns = after.median_ns.abs_diff(before.median_ns);
            let verdict =
                if change_pct.abs() <= NOISE_BAND_PCT || absolute_delta_ns < NOISE_FLOOR_NS {
                    "noise"
                } else if change_pct > 0.0 {
                    "slower"
                } else {
                    "faster"
                };
            deltas.push(BaselineDelta {
                case_id: case.id.clone(),
                label: case.label.clone(),
                profile: profile.clone(),
                baseline_median_ns: before.median_ns,
                current_median_ns: after.median_ns,
                change_pct,
                verdict: verdict.to_string(),
            });
        }
    }
    Ok(deltas)
}

// ─── Result model ─────────────────────────────────────────────────────────────
//
// The shape follows the "Result model" section of `spec/benchmark-runner.md`:
// schema version, run metadata, host, dataset, policy, and per-case raw samples
// plus summaries, so a chart is never the source of truth.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub run: RunInfo,
    pub host: HostInfo,
    pub dataset: DatasetInfo,
    pub policy: PolicyInfo,
    pub cases: Vec<CaseResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<PipelineResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baseline: Vec<BaselineDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub run_id: String,
    pub started_at: String,
    pub tool: String,
    pub sct_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub os: String,
    pub architecture: String,
    pub cpu: String,
    pub logical_cores: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetInfo {
    /// File **name** of the database. Never a path.
    pub database_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concept_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_id: Option<String>,
    /// Whether release identity was withheld with `--no-provenance`, so a
    /// reader can tell "not disclosed" from "not recorded".
    #[serde(default)]
    pub provenance_suppressed: bool,
    #[serde(default)]
    pub transitive_closure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artefacts: Option<Artefacts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artefacts {
    pub database_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fst_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embeddings_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyInfo {
    pub profiles: Vec<String>,
    pub warmup: usize,
    pub samples: usize,
    pub full: bool,
    pub cache_mode: String,
    pub concurrency: usize,
    pub timer: String,
    pub noise_band_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub id: String,
    pub label: String,
    pub operation: String,
    pub input: String,
    /// `ok` or `skipped`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileResult {
    /// `ok` or `unavailable`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    #[serde(default)]
    pub samples: Vec<Sample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
}

impl ProfileResult {
    fn unavailable(reason: String) -> Self {
        Self {
            status: "unavailable".to_string(),
            unavailable_reason: Some(reason),
            samples: Vec::new(),
            summary: None,
        }
    }

    fn measured(samples: Vec<Sample>) -> Self {
        let summary = summarize(&samples);
        Self {
            status: if summary.is_some() {
                "ok".to_string()
            } else {
                "unavailable".to_string()
            },
            unavailable_reason: summary
                .is_none()
                .then(|| "every timed run failed".to_string()),
            samples,
            summary,
        }
    }

    fn median_ns(&self) -> Option<u64> {
        self.summary.map(|s| s.median_ns)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub elapsed_ns: u64,
    pub ok: bool,
    /// A fixed class such as `nonzero_exit`, never a message (a message could
    /// carry a filesystem path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
}

/// Aggregates over the successful samples. `median_ns` is the p50.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Summary {
    pub samples: usize,
    pub median_ns: u64,
    pub mean_ns: u64,
    pub std_dev_ns: u64,
    pub min_ns: u64,
    pub max_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub error_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineResult {
    /// File **name** of the RF2 input. Never a path.
    pub source: String,
    pub stages: Vec<PipelineStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStage {
    pub stage: String,
    pub elapsed_ns: u64,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineDelta {
    pub case_id: String,
    pub label: String,
    pub profile: String,
    pub baseline_median_ns: u64,
    pub current_median_ns: u64,
    pub change_pct: f64,
    /// `faster`, `slower`, or `noise` (inside the band).
    pub verdict: String,
}

// ─── Shared rendering pieces ──────────────────────────────────────────────────

fn fmt_ms(ns: u64) -> String {
    format!("{:.3} ms", ns as f64 / 1_000_000.0)
}

fn fmt_wall(ns: u64) -> String {
    let secs = ns as f64 / 1_000_000_000.0;
    if secs >= 1.0 {
        format!("{secs:.2} s")
    } else {
        fmt_ms(ns)
    }
}

fn machine_line(host: &HostInfo) -> String {
    let memory = host
        .memory_bytes
        .map(human_bytes)
        .unwrap_or_else(|| "unknown RAM".to_string());
    format!(
        "{}, {} cores, {memory} ({}/{})",
        host.cpu, host.logical_cores, host.os, host.architecture
    )
}

fn database_line(dataset: &DatasetInfo) -> String {
    let mut parts = vec![dataset.database_file.clone()];
    if let Some(n) = dataset.concept_count {
        parts.push(format!("{} concepts", fmt_count(n)));
    }
    match (&dataset.edition, &dataset.release_date) {
        (Some(edition), Some(date)) => parts.push(format!("{edition} ({date})")),
        (Some(edition), None) => parts.push(edition.clone()),
        _ if dataset.provenance_suppressed => parts.push("release identity withheld".to_string()),
        _ => {}
    }
    if let Some(v) = dataset.schema_version {
        parts.push(format!("schema v{v}"));
    }
    parts.join(", ")
}

fn artefacts_line(artefacts: &Artefacts, has_tct: bool) -> String {
    let optional = |bytes: Option<u64>| match bytes {
        Some(b) => human_bytes(b),
        None => "absent".to_string(),
    };
    format!(
        "db {}, fst {}, tct {}, embeddings {}",
        human_bytes(artefacts.database_bytes),
        optional(artefacts.fst_bytes),
        if has_tct { "present" } else { "absent" },
        optional(artefacts.embeddings_bytes),
    )
}

/// A rendered timing table, built once and formatted three ways.
struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn timing_table(report: &Report) -> Table {
    let has_sdk = report.policy.profiles.iter().any(|p| p == "sdk");
    let has_cli = report.policy.profiles.iter().any(|p| p == "cli");
    if !has_sdk && !has_cli {
        // Nothing was timed: a column of labels is not a timing table.
        return Table {
            headers: Vec::new(),
            rows: Vec::new(),
        };
    }

    let mut headers = vec!["Operation".to_string()];
    if has_sdk {
        headers.push("SDK (median)".to_string());
    }
    if has_cli {
        headers.push("CLI (median)".to_string());
    }
    if has_sdk && has_cli {
        headers.push("startup cost".to_string());
    }

    let mut rows = Vec::new();
    for case in report.cases.iter().filter(|c| c.status == "ok") {
        let sdk = case.profiles.get("sdk").and_then(ProfileResult::median_ns);
        let cli = case.profiles.get("cli").and_then(ProfileResult::median_ns);
        let mut row = vec![case.label.clone()];
        let cell = |v: Option<u64>| v.map(fmt_ms).unwrap_or_else(|| "n/a".to_string());
        if has_sdk {
            row.push(cell(sdk));
        }
        if has_cli {
            row.push(cell(cli));
        }
        if has_sdk && has_cli {
            row.push(cell(match (sdk, cli) {
                (Some(s), Some(c)) => Some(c.saturating_sub(s)),
                _ => None,
            }));
        }
        rows.push(row);
    }
    Table { headers, rows }
}

/// The sampling disclosure and the single-run caveat, which every rendering
/// must carry. Empty when the run timed nothing (an `artefact`-only run has no
/// sampling policy worth quoting).
fn caveat_lines(report: &Report) -> Vec<String> {
    if report
        .cases
        .iter()
        .all(|case| case.profiles.values().all(|p| p.summary.is_none()))
    {
        return Vec::new();
    }
    vec![
        format!(
            "{} samples per case after {} warm-up runs; medians shown, p95 in --format json.",
            report.policy.samples, report.policy.warmup
        ),
        "Single run on an uncontrolled machine - treat as an order of magnitude.".to_string(),
    ]
}

/// Reasons a case or a profile produced no timing, as `(label, reason)` pairs.
fn unavailability(report: &Report) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for case in &report.cases {
        if case.status == "skipped" {
            out.push((
                case.label.clone(),
                case.skipped_reason
                    .clone()
                    .unwrap_or_else(|| "skipped".to_string()),
            ));
            continue;
        }
        for (profile, result) in &case.profiles {
            if let Some(reason) = &result.unavailable_reason {
                out.push((format!("{} ({profile})", case.label), reason.clone()));
            }
        }
    }
    out
}

// ─── Text ─────────────────────────────────────────────────────────────────────

fn render_text(report: &Report) -> String {
    let mut s = String::new();
    s.push_str(&format!("sct bench {}\n\n", report.run.sct_version));
    s.push_str(&format!("  Machine     {}\n", machine_line(&report.host)));
    s.push_str(&format!(
        "  Database    {}\n",
        database_line(&report.dataset)
    ));
    if let Some(artefacts) = &report.dataset.artefacts {
        s.push_str(&format!(
            "  Artefacts   {}\n",
            artefacts_line(artefacts, report.dataset.transitive_closure)
        ));
    }

    let table = timing_table(report);
    if !table.rows.is_empty() {
        s.push('\n');
        let label_width = table
            .rows
            .iter()
            .map(|r| r[0].chars().count())
            .chain(std::iter::once(table.headers[0].chars().count()))
            .max()
            .unwrap_or(20)
            .max(20);
        let value_width = 16;

        s.push_str("  ");
        s.push_str(&format!("{:<label_width$}", table.headers[0]));
        for header in &table.headers[1..] {
            s.push_str(&format!("{header:>value_width$}"));
        }
        s.push('\n');
        for row in &table.rows {
            s.push_str("  ");
            s.push_str(&format!("{:<label_width$}", row[0]));
            for cell in &row[1..] {
                s.push_str(&format!("{cell:>value_width$}"));
            }
            s.push('\n');
        }
    }

    let skipped = unavailability(report);
    if !skipped.is_empty() {
        s.push_str("\n  Not measured\n");
        let width = skipped
            .iter()
            .map(|(label, _)| label.chars().count())
            .max()
            .unwrap_or(20);
        for (label, reason) in &skipped {
            s.push_str(&format!("    {label:<width$}  {reason}\n"));
        }
    }

    if let Some(pipeline) = &report.pipeline {
        s.push_str(&format!(
            "\n  Pipeline (single run, from {})\n",
            pipeline.source
        ));
        for stage in &pipeline.stages {
            s.push_str(&format!(
                "    {:<20}{:>12}\n",
                stage.stage,
                fmt_wall(stage.elapsed_ns)
            ));
        }
    }

    if !report.baseline.is_empty() {
        s.push_str(&format!(
            "\n  Baseline comparison (noise band ±{:.0}%)\n",
            report.policy.noise_band_pct
        ));
        for delta in &report.baseline {
            s.push_str(&format!(
                "    {:<24}{:<6}{:>12} → {:>12}{:>+9.1}%  {}\n",
                delta.label,
                delta.profile,
                fmt_ms(delta.baseline_median_ns),
                fmt_ms(delta.current_median_ns),
                delta.change_pct,
                delta.verdict,
            ));
        }
    }

    let caveats = caveat_lines(report);
    if !caveats.is_empty() {
        s.push('\n');
        for line in caveats {
            s.push_str(&format!("  {line}\n"));
        }
    }
    s.push_str("\n  Share:  sct bench --format markdown | pbcopy\n");
    s
}

// ─── Markdown ─────────────────────────────────────────────────────────────────

/// Make one cell safe inside a Markdown table: `|` would end the cell, and `<`
/// is swallowed by the raw-HTML pass GitHub and Discourse both run - which
/// matters here because half the labels contain an ECL operator.
fn md_cell(value: &str) -> String {
    value.replace('|', r"\|").replace('<', "&lt;")
}

fn markdown_table(table: &Table) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "| {} |\n",
        table
            .headers
            .iter()
            .map(|h| md_cell(h))
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    s.push_str(&format!(
        "|{}|\n",
        table
            .headers
            .iter()
            .enumerate()
            .map(|(i, _)| if i == 0 { "---" } else { "---:" })
            .collect::<Vec<_>>()
            .join("|")
    ));
    for row in &table.rows {
        s.push_str(&format!(
            "| {} |\n",
            row.iter()
                .map(|c| md_cell(c))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    s
}

fn render_markdown(report: &Report) -> String {
    let mut s = String::new();
    s.push_str(&format!("### `sct bench` {}\n\n", report.run.sct_version));

    // The environment block is fenced so a forum or issue renderer leaves the
    // alignment alone.
    s.push_str("```text\n");
    s.push_str(&format!("Machine     {}\n", machine_line(&report.host)));
    s.push_str(&format!("Database    {}\n", database_line(&report.dataset)));
    if let Some(artefacts) = &report.dataset.artefacts {
        s.push_str(&format!(
            "Artefacts   {}\n",
            artefacts_line(artefacts, report.dataset.transitive_closure)
        ));
    }
    s.push_str("```\n");

    let table = timing_table(report);
    if !table.rows.is_empty() {
        s.push('\n');
        s.push_str(&markdown_table(&table));
    }

    let skipped = unavailability(report);
    if !skipped.is_empty() {
        s.push_str("\n**Not measured**\n\n");
        for (label, reason) in &skipped {
            s.push_str(&format!("- {} - {}\n", md_cell(label), md_cell(reason)));
        }
    }

    if let Some(pipeline) = &report.pipeline {
        s.push_str(&format!(
            "\n**Pipeline** (single run, from `{}`)\n\n",
            pipeline.source
        ));
        s.push_str("| Stage | Elapsed |\n|---|---:|\n");
        for stage in &pipeline.stages {
            s.push_str(&format!(
                "| {} | {} |\n",
                md_cell(&stage.stage),
                fmt_wall(stage.elapsed_ns)
            ));
        }
    }

    if !report.baseline.is_empty() {
        s.push_str(&format!(
            "\n**Baseline comparison** (noise band ±{:.0}%)\n\n",
            report.policy.noise_band_pct
        ));
        s.push_str(
            "| Operation | Profile | Baseline | Now | Change | |\n|---|---|---:|---:|---:|---|\n",
        );
        for delta in &report.baseline {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {:+.1}% | {} |\n",
                md_cell(&delta.label),
                delta.profile,
                fmt_ms(delta.baseline_median_ns),
                fmt_ms(delta.current_median_ns),
                delta.change_pct,
                delta.verdict,
            ));
        }
    }

    s.push('\n');
    for line in caveat_lines(report) {
        s.push_str(&format!("_{line}_\n\n"));
    }
    s
}

// ─── HTML ─────────────────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn html_table(table: &Table) -> String {
    let mut s = String::from("<table>\n<thead><tr>");
    for (i, header) in table.headers.iter().enumerate() {
        let class = if i == 0 { "" } else { " class=\"num\"" };
        s.push_str(&format!("<th{class}>{}</th>", esc(header)));
    }
    s.push_str("</tr></thead>\n<tbody>\n");
    for row in &table.rows {
        s.push_str("<tr>");
        for (i, cell) in row.iter().enumerate() {
            let class = if i == 0 { "" } else { " class=\"num\"" };
            s.push_str(&format!("<td{class}>{}</td>", esc(cell)));
        }
        s.push_str("</tr>\n");
    }
    s.push_str("</tbody>\n</table>\n");
    s
}

/// A standalone document: inline CSS, no scripts, no fonts, no images, and so
/// no network requests of any kind. Consistent with the project's no-CDN rule.
fn render_html(report: &Report) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "<h1>sct bench <span class=\"v\">{}</span></h1>\n",
        esc(&report.run.sct_version)
    ));

    body.push_str("<dl class=\"env\">\n");
    body.push_str(&format!(
        "<dt>Machine</dt><dd>{}</dd>\n",
        esc(&machine_line(&report.host))
    ));
    body.push_str(&format!(
        "<dt>Database</dt><dd>{}</dd>\n",
        esc(&database_line(&report.dataset))
    ));
    if let Some(artefacts) = &report.dataset.artefacts {
        body.push_str(&format!(
            "<dt>Artefacts</dt><dd>{}</dd>\n",
            esc(&artefacts_line(
                artefacts,
                report.dataset.transitive_closure
            ))
        ));
    }
    body.push_str(&format!(
        "<dt>Run</dt><dd>{}</dd>\n",
        esc(&report.run.started_at)
    ));
    body.push_str("</dl>\n");

    let table = timing_table(report);
    if !table.rows.is_empty() {
        body.push_str(&html_table(&table));
    }

    let skipped = unavailability(report);
    if !skipped.is_empty() {
        body.push_str("<h2>Not measured</h2>\n<ul>\n");
        for (label, reason) in &skipped {
            body.push_str(&format!(
                "<li><b>{}</b> - {}</li>\n",
                esc(label),
                esc(reason)
            ));
        }
        body.push_str("</ul>\n");
    }

    if let Some(pipeline) = &report.pipeline {
        body.push_str(&format!(
            "<h2>Pipeline</h2>\n<p>Single run, from <code>{}</code>.</p>\n",
            esc(&pipeline.source)
        ));
        let stage_table = Table {
            headers: vec!["Stage".into(), "Elapsed".into()],
            rows: pipeline
                .stages
                .iter()
                .map(|s| vec![s.stage.clone(), fmt_wall(s.elapsed_ns)])
                .collect(),
        };
        body.push_str(&html_table(&stage_table));
    }

    if !report.baseline.is_empty() {
        body.push_str(&format!(
            "<h2>Baseline comparison</h2>\n<p>Noise band ±{:.0}%.</p>\n",
            report.policy.noise_band_pct
        ));
        let delta_table = Table {
            headers: vec![
                "Operation".into(),
                "Profile".into(),
                "Baseline".into(),
                "Now".into(),
                "Change".into(),
                "Verdict".into(),
            ],
            rows: report
                .baseline
                .iter()
                .map(|d| {
                    vec![
                        d.label.clone(),
                        d.profile.clone(),
                        fmt_ms(d.baseline_median_ns),
                        fmt_ms(d.current_median_ns),
                        format!("{:+.1}%", d.change_pct),
                        d.verdict.clone(),
                    ]
                })
                .collect(),
        };
        body.push_str(&html_table(&delta_table));
    }

    body.push_str("<footer>\n");
    for line in caveat_lines(report) {
        body.push_str(&format!("<p>{}</p>\n", esc(&line)));
    }
    body.push_str("</footer>\n");

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>sct bench - {}</title>\n<style>\n{CSS}</style>\n</head>\n<body>\n<main>\n{body}</main>\n</body>\n</html>\n",
        esc(&report.dataset.database_file)
    )
}

const CSS: &str = "\
:root { color-scheme: light dark; }
body { margin: 0; padding: 2rem 1rem; font: 16px/1.5 ui-sans-serif, system-ui, sans-serif; }
main { max-width: 52rem; margin: 0 auto; }
h1 { font-size: 1.5rem; margin: 0 0 1rem; }
h1 .v { font-weight: 400; opacity: 0.6; }
h2 { font-size: 1.1rem; margin: 2rem 0 0.5rem; }
dl.env { display: grid; grid-template-columns: max-content 1fr; gap: 0.25rem 1rem; margin: 0 0 1.5rem; }
dl.env dt { font-weight: 600; }
dl.env dd { margin: 0; }
table { border-collapse: collapse; width: 100%; margin: 0 0 1rem; }
th, td { padding: 0.35rem 0.6rem; border-bottom: 1px solid rgba(128,128,128,0.35); text-align: left; }
th { font-weight: 600; }
th.num, td.num { text-align: right; font-variant-numeric: tabular-nums; }
ul { padding-left: 1.2rem; }
footer { margin-top: 2rem; font-size: 0.9rem; opacity: 0.75; }
footer p { margin: 0.25rem 0; }
code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
";

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(values: &[u64]) -> Vec<Sample> {
        values
            .iter()
            .map(|v| Sample {
                elapsed_ns: *v,
                ok: true,
                error_class: None,
            })
            .collect()
    }

    #[test]
    fn summary_statistics_are_computed_from_raw_samples() {
        let s = summarize(&samples(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 100])).expect("summary");
        assert_eq!(s.samples, 10);
        assert_eq!(s.median_ns, 55); // even length: mean of 50 and 60
        assert_eq!(s.mean_ns, 55);
        assert_eq!(s.min_ns, 10);
        assert_eq!(s.max_ns, 100);
        assert_eq!(s.p95_ns, 100); // nearest rank: ceil(0.95 * 10) = 10
        assert_eq!(s.error_rate, 0.0);
    }

    #[test]
    fn odd_length_median_is_the_middle_sample() {
        let s = summarize(&samples(&[5, 1, 3])).expect("summary");
        assert_eq!(s.median_ns, 3);
        assert_eq!(s.min_ns, 1);
        assert_eq!(s.max_ns, 5);
    }

    #[test]
    fn failed_samples_do_not_contribute_timings() {
        let mut values = samples(&[10, 20, 30]);
        values.push(Sample {
            elapsed_ns: 999_999,
            ok: false,
            error_class: Some("nonzero_exit".into()),
        });
        let s = summarize(&values).expect("summary");
        assert_eq!(s.samples, 3);
        assert_eq!(s.max_ns, 30);
        assert!((s.error_rate - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn all_failed_samples_yield_no_summary() {
        let values = vec![Sample {
            elapsed_ns: 5,
            ok: false,
            error_class: Some("sdk_error".into()),
        }];
        assert!(summarize(&values).is_none());
        let result = ProfileResult::measured(values);
        assert_eq!(result.status, "unavailable");
        assert!(result.summary.is_none());
    }

    #[test]
    fn measure_runs_warmups_outside_the_timed_samples() {
        let mut calls = 0usize;
        let taken = measure(
            Sampling {
                warmup: 4,
                samples: 3,
            },
            || {
                calls += 1;
                None
            },
        );
        assert_eq!(taken.len(), 3);
        assert_eq!(calls, 7);
        assert!(taken.iter().all(|s| s.ok));
    }

    #[test]
    fn cli_args_never_omit_the_database_and_map_hierarchy_to_ecl() {
        let db = Path::new("/tmp/example.db");
        let args = cli_args(
            &CaseKind::Children {
                id: "73211009".into(),
                limit: 10,
            },
            db,
            None,
        )
        .expect("children has a cli form");
        let joined: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(joined[0], "ecl");
        assert_eq!(joined[2], "<!73211009");
        assert!(joined.contains(&"--db".to_string()));

        // Subsumption: `<<left AND right` is non-empty exactly when left subsumes right.
        let args = cli_args(
            &CaseKind::Subsumes {
                left: "73211009".into(),
                right: "46635009".into(),
            },
            db,
            None,
        )
        .expect("subsumption has a cli form");
        assert_eq!(args[2].to_string_lossy(), "<<73211009 AND 46635009");
    }

    #[test]
    fn fst_case_has_no_cli_form_without_an_index() {
        let kind = CaseKind::FstPrefix {
            prefix: "myoca".into(),
            limit: 5,
        };
        assert!(cli_args(&kind, Path::new("/tmp/x.db"), None).is_none());
        assert!(cli_args(&kind, Path::new("/tmp/x.db"), Some(Path::new("/tmp/x.fst"))).is_some());
    }

    #[test]
    fn full_run_adds_deeper_cases() {
        let default = default_cases(false);
        let full = default_cases(true);
        assert!(full.len() > default.len());
        assert!(full.iter().any(|c| c.id == "ecl_broad"));
        assert!(!default.iter().any(|c| c.id == "ecl_broad"));
    }

    #[test]
    fn every_case_declares_what_it_needs() {
        for case in default_cases(true) {
            let declares = !case.requires_concepts.is_empty() || case.requires_fst;
            // Lexical search is the one case with no dataset requirement: it
            // matches whatever the database happens to contain.
            assert!(
                declares || case.id == "lexical_search",
                "case {} declares no requirements",
                case.id
            );
        }
    }

    fn report_fixture() -> Report {
        Report {
            schema_version: RESULT_SCHEMA_VERSION,
            run: RunInfo {
                run_id: "20260101T000000Z-1".into(),
                started_at: "2026-01-01T00:00:00Z".into(),
                tool: "sct bench".into(),
                sct_version: "0.0.0".into(),
                git_commit: None,
            },
            host: HostInfo {
                os: "linux".into(),
                architecture: "x86_64".into(),
                cpu: "Example CPU".into(),
                logical_cores: 4,
                memory_bytes: Some(8 * 1024 * 1024 * 1024),
            },
            dataset: DatasetInfo {
                database_file: "fixture.db".into(),
                concept_count: Some(23),
                schema_version: Some(9),
                edition: Some("Synthetic".into()),
                release_date: Some("2026-01-01".into()),
                release_id: Some("SnomedCT_SyntheticTest".into()),
                provenance_suppressed: false,
                transitive_closure: false,
                artefacts: Some(Artefacts {
                    database_bytes: 1024,
                    fst_bytes: None,
                    embeddings_bytes: None,
                }),
            },
            policy: PolicyInfo {
                profiles: vec!["sdk".into(), "cli".into(), "artefact".into()],
                warmup: 1,
                samples: 2,
                full: false,
                cache_mode: "warm".into(),
                concurrency: 1,
                timer: "monotonic (std::time::Instant)".into(),
                noise_band_pct: NOISE_BAND_PCT,
            },
            cases: vec![
                CaseResult {
                    id: "lookup_sctid".into(),
                    label: "lookup by SCTID".into(),
                    operation: "lookup".into(),
                    input: "22298006".into(),
                    status: "ok".into(),
                    skipped_reason: None,
                    profiles: BTreeMap::from([
                        (
                            "sdk".to_string(),
                            ProfileResult::measured(samples(&[100_000, 120_000])),
                        ),
                        (
                            "cli".to_string(),
                            ProfileResult::measured(samples(&[8_000_000, 8_400_000])),
                        ),
                    ]),
                },
                CaseResult {
                    id: "fst_prefix".into(),
                    label: "FST prefix \"myoca\"".into(),
                    operation: "fst_prefix".into(),
                    input: "\"myoca\", limit 10".into(),
                    status: "skipped".into(),
                    skipped_reason: Some("no FST index alongside this database".into()),
                    profiles: BTreeMap::new(),
                },
            ],
            pipeline: None,
            baseline: Vec::new(),
        }
    }

    #[test]
    fn skipped_cases_never_appear_as_timing_rows() {
        let report = report_fixture();
        let table = timing_table(&report);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0][0], "lookup by SCTID");
        // ... but they are reported.
        let text = render_text(&report);
        assert!(text.contains("Not measured"));
        assert!(text.contains("no FST index alongside this database"));
    }

    #[test]
    fn text_output_states_sampling_policy_and_the_caveat() {
        let text = render_text(&report_fixture());
        assert!(text.contains("2 samples per case after 1 warm-up runs"));
        assert!(text.contains("Single run on an uncontrolled machine"));
        assert!(text.contains("Share:"));
        assert!(text.contains("startup cost"));
    }

    #[test]
    fn startup_cost_is_the_cli_minus_sdk_median() {
        let table = timing_table(&report_fixture());
        // sdk median 110_000 ns, cli median 8_200_000 ns.
        assert_eq!(table.rows[0][1], "0.110 ms");
        assert_eq!(table.rows[0][2], "8.200 ms");
        assert_eq!(table.rows[0][3], "8.090 ms");
    }

    #[test]
    fn markdown_output_is_a_pasteable_table() {
        let md = render_markdown(&report_fixture());
        assert!(md.contains("| Operation | SDK (median) | CLI (median) | startup cost |"));
        assert!(md.contains("|---|---:|---:|---:|"));
        assert!(md.contains("```text"));
    }

    #[test]
    fn markdown_escapes_ecl_operators_so_the_cell_survives_a_renderer() {
        let mut report = report_fixture();
        report.cases[0].label = "ECL <<73211009 | deep".into();
        let md = render_markdown(&report);
        // `<` would be eaten by the raw-HTML pass, `|` would end the cell.
        assert!(md.contains(r"| ECL &lt;&lt;73211009 \| deep |"));
        assert!(!md.contains("| ECL <<73211009"));
    }

    #[test]
    fn html_output_is_standalone_and_has_no_external_references() {
        let html = render_html(&report_fixture());
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<style>"));
        for forbidden in ["http://", "https://", "<script", "src=", "@import"] {
            assert!(
                !html.contains(forbidden),
                "standalone HTML must not contain {forbidden}"
            );
        }
    }

    #[test]
    fn html_escapes_values_that_could_close_a_tag() {
        let mut report = report_fixture();
        report.dataset.database_file = "<script>x</script>.db".into();
        let html = render_html(&report);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn no_provenance_withholds_release_identity_but_keeps_the_shape() {
        let mut report = report_fixture();
        report.dataset.edition = None;
        report.dataset.release_date = None;
        report.dataset.release_id = None;
        report.dataset.provenance_suppressed = true;
        let line = database_line(&report.dataset);
        assert!(line.contains("fixture.db"));
        assert!(line.contains("23 concepts"));
        assert!(line.contains("schema v9"));
        assert!(line.contains("release identity withheld"));
        assert!(!line.contains("Synthetic"));
    }

    #[test]
    fn baseline_deltas_separate_noise_from_regressions() {
        let baseline = report_fixture();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("baseline.json");
        std::fs::write(&path, serde_json::to_string(&baseline).expect("serialise")).expect("write");

        let mut current = report_fixture();
        // sdk: 110_000 -> 115_000 ns (+4.5%, inside the band).
        // cli: 8_200_000 -> 12_000_000 ns (+46%, outside it).
        current.cases[0].profiles.insert(
            "sdk".into(),
            ProfileResult::measured(samples(&[115_000, 115_000])),
        );
        current.cases[0].profiles.insert(
            "cli".into(),
            ProfileResult::measured(samples(&[12_000_000, 12_000_000])),
        );

        let deltas = compare_baseline(&path, &current).expect("compare");
        let sdk = deltas.iter().find(|d| d.profile == "sdk").expect("sdk");
        let cli = deltas.iter().find(|d| d.profile == "cli").expect("cli");
        assert_eq!(sdk.verdict, "noise");
        assert_eq!(cli.verdict, "slower");
        assert!(cli.change_pct > NOISE_BAND_PCT);
    }

    #[test]
    fn a_large_percentage_swing_on_a_fast_operation_is_still_noise() {
        // The failure this guards against: an in-process operation moving
        // 0.045 ms -> 0.114 ms is +152%, which a purely relative band calls a
        // regression. Both figures are near timer granularity, so the verdict
        // would send a reader chasing a regression that does not exist.
        let baseline = report_fixture();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("baseline.json");
        std::fs::write(&path, serde_json::to_string(&baseline).expect("serialise")).expect("write");

        let mut current = report_fixture();
        current.cases[0].profiles.insert(
            "sdk".into(),
            ProfileResult::measured(samples(&[280_000, 280_000])),
        );
        let deltas = compare_baseline(&path, &current).expect("compare");
        let sdk = deltas.iter().find(|d| d.profile == "sdk").expect("sdk");

        assert!(
            sdk.change_pct > NOISE_BAND_PCT,
            "the percentage is genuinely outside the band: {}",
            sdk.change_pct
        );
        assert_eq!(
            sdk.verdict, "noise",
            "but the absolute move is under the floor, so it is not a regression"
        );

        // A move of the same shape but past the floor is still reported.
        current.cases[0].profiles.insert(
            "sdk".into(),
            ProfileResult::measured(samples(&[900_000, 900_000])),
        );
        let deltas = compare_baseline(&path, &current).expect("compare");
        let sdk = deltas.iter().find(|d| d.profile == "sdk").expect("sdk");
        assert_eq!(sdk.verdict, "slower");
    }

    #[test]
    fn report_round_trips_through_json() {
        let report = report_fixture();
        let json = serde_json::to_string(&report).expect("serialise");
        let parsed: Report = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(parsed.schema_version, RESULT_SCHEMA_VERSION);
        assert_eq!(parsed.cases.len(), report.cases.len());
        assert_eq!(
            parsed.cases[0].profiles["sdk"].samples.len(),
            report.cases[0].profiles["sdk"].samples.len()
        );
    }

    #[test]
    fn host_info_carries_no_identity() {
        let host = host_info();
        assert!(!host.os.is_empty());
        assert!(!host.architecture.is_empty());
        // A hostname would be the obvious accidental leak here.
        let hostname = std::env::var("HOSTNAME").unwrap_or_default();
        if !hostname.is_empty() && hostname.len() > 3 {
            assert!(!host.cpu.contains(&hostname));
        }
    }
}
