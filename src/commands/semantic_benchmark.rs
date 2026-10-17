// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct bench semantic` - evaluate retrieval strategies against a fixed clinical corpus.

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::commands::semantic::{self, ScoredConcept};
use crate::index::Index;
use crate::output::OutputFormat;
use crate::sdk::{SearchHit, Snomed};

const DEFAULT_CORPUS: &str = include_str!("../../benchmarks/scenarios/semantic-search.yaml");
const REPORT_SCHEMA_VERSION: u32 = 1;
const COMPARISON_REPORT_SCHEMA_VERSION: u32 = 2;
const MAX_CASES: usize = 100;

#[derive(Parser, Debug)]
pub struct Args {
    /// Arrow IPC embeddings file produced by `sct embed`. See
    /// `docs/path-resolution.md` for the discovery order when omitted.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub embeddings: Option<PathBuf>,

    /// Supported Ollama embedding model matching the embeddings artefact.
    #[arg(long, default_value = "nomic-embed-text")]
    pub model: String,

    /// Ollama API base URL.
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// YAML query corpus. The versioned built-in R15 corpus is used when omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub corpus: Option<PathBuf>,

    /// SQLite database produced from the same canonical NDJSON as the
    /// embeddings. Requires `--index` and enables lexical/fuzzy/hybrid
    /// comparison.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// FST index produced from the same canonical NDJSON as the database and
    /// embeddings. Requires `--db` and enables lexical/fuzzy/hybrid comparison.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub index: Option<PathBuf>,

    /// Reciprocal-rank-fusion constant used by the experimental hybrid strategy.
    #[arg(long, default_value = "60")]
    pub rrf_k: usize,

    /// Dense-ranking contribution to reciprocal-rank fusion.
    #[arg(long, default_value = "1.0")]
    pub dense_weight: f64,

    /// Lexical-ranking contribution to reciprocal-rank fusion.
    #[arg(long, default_value = "1.0")]
    pub lexical_weight: f64,

    /// Fuzzy-ranking contribution to reciprocal-rank fusion.
    #[arg(long, default_value = "1.0")]
    pub fuzzy_weight: f64,

    /// Ranked results retained per case and searched for expected concepts.
    #[arg(long, default_value = "1000")]
    pub limit: usize,

    /// Query-embedding warm-up requests excluded from timing. Default: 1.
    #[arg(long)]
    pub warmup: Option<usize>,

    /// Output format. Default: text.
    #[arg(long, short = 'f', value_enum)]
    pub format: Option<OutputFormat>,

    /// Write the report to a file instead of stdout.
    #[arg(long, short = 'o', value_parser = crate::paths::tilde_pathbuf)]
    pub output: Option<PathBuf>,

    /// Withhold release identity and content fingerprint from the report.
    #[arg(long)]
    pub no_provenance: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    version: u32,
    name: String,
    cases: Vec<CorpusCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusCase {
    id: String,
    query: String,
    expected_ids: Vec<String>,
    classes: Vec<QueryClass>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum QueryClass {
    CategoryDrift,
    ColloquialLanguage,
    Idiom,
    Misspelling,
    SynonymDilution,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    run: RunInfo,
    host: crate::commands::bench::HostInfo,
    corpus: CorpusInfo,
    artifact: ArtifactInfo,
    ollama: crate::commands::ollama::ModelInfo,
    policy: Policy,
    timings: Timings,
    metrics: Metrics,
    metrics_by_class: BTreeMap<QueryClass, Metrics>,
    cases: Vec<CaseResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    comparison: Option<ComparisonReport>,
}

#[derive(Debug, Serialize)]
struct RunInfo {
    run_id: String,
    started_at: String,
    tool: &'static str,
    sct_version: &'static str,
    git_commit: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct CorpusInfo {
    name: String,
    version: u32,
    sha256: String,
    source: String,
    case_count: usize,
}

#[derive(Debug, Serialize)]
struct ArtifactInfo {
    file: String,
    bytes: u64,
    vector_count: usize,
    dimensions: usize,
    model: Option<String>,
    model_digest: Option<String>,
    model_digest_verified: bool,
    profile: Option<String>,
    text_scheme: Option<String>,
    edition: Option<String>,
    release_date: Option<String>,
    release_id: Option<String>,
    content_fingerprint: Option<String>,
    built_by_sct: Option<String>,
    provenance_suppressed: bool,
}

#[derive(Debug, Serialize)]
struct Policy {
    strategy: &'static str,
    query_model: String,
    query_profile: String,
    result_cutoff: usize,
    query_batch_size: usize,
    ollama_warmup_requests: usize,
    active_preference: bool,
}

#[derive(Debug, Serialize)]
struct Timings {
    query_embedding_ns: u64,
    arrow_scan_batch_ns: u64,
    total_batch_ns: u64,
    model_identity_check_ns: u64,
    amortized_query_embedding_ns_per_query: u64,
    amortized_arrow_scan_ns_per_query: u64,
    arrow_scan_samples: usize,
    cache_state: &'static str,
    build_time_ns: Option<u64>,
    peak_model_memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct Metrics {
    cases: usize,
    top_1_rate: f64,
    top_5_rate: f64,
    top_10_rate: f64,
    mean_reciprocal_rank_at_cutoff: f64,
    mean_retrieved_expected_score: Option<f64>,
    retrieved_expected_scores: usize,
}

#[derive(Debug, Serialize)]
struct CaseResult {
    id: String,
    query: String,
    expected_ids: Vec<String>,
    classes: Vec<QueryClass>,
    rank: Option<usize>,
    matched_id: Option<String>,
    expected_score: Option<f32>,
    results: Vec<ScoredConcept>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
enum RetrievalStrategy {
    Dense,
    Lexical,
    Fuzzy,
    Hybrid,
}

#[derive(Debug, Serialize)]
struct ComparisonReport {
    artifacts: ComparisonArtifacts,
    policy: FusionPolicy,
    timings_ns: BTreeMap<RetrievalStrategy, u64>,
    strategies: BTreeMap<RetrievalStrategy, StrategyEvaluation>,
}

#[derive(Debug, Serialize)]
struct ComparisonArtifacts {
    database_file: String,
    database_bytes: u64,
    fst_index_file: String,
    fst_index_bytes: u64,
    release_id_verified: bool,
    content_fingerprint_verified: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct FusionPolicy {
    method: &'static str,
    rrf_k: usize,
    dense_weight: f64,
    lexical_weight: f64,
    fuzzy_weight: f64,
}

#[derive(Debug, Serialize)]
struct StrategyEvaluation {
    metrics: RankingMetrics,
    metrics_by_class: BTreeMap<QueryClass, RankingMetrics>,
    cases: Vec<StrategyCaseResult>,
}

#[derive(Debug, Serialize)]
struct RankingMetrics {
    cases: usize,
    top_1_rate: f64,
    top_5_rate: f64,
    top_10_rate: f64,
    mean_reciprocal_rank_at_cutoff: f64,
}

#[derive(Debug, Serialize)]
struct StrategyCaseResult {
    id: String,
    query: String,
    expected_ids: Vec<String>,
    classes: Vec<QueryClass>,
    rank: Option<usize>,
    matched_id: Option<String>,
    results: Vec<ComparisonCandidate>,
}

#[derive(Debug, Clone, Serialize)]
struct ComparisonCandidate {
    id: String,
    preferred_term: String,
    /// Source-native score. Lexical FTS exposes only rank, so this is null for
    /// lexical candidates and must not be compared across strategies.
    score: Option<f64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    source_ranks: BTreeMap<RetrievalStrategy, usize>,
}

pub fn run(args: Args) -> Result<()> {
    anyhow::ensure!(
        (1..=1000).contains(&args.limit),
        "--limit must be between 1 and 1000"
    );
    let warmup = args.warmup.unwrap_or(1);
    let format = args.format.unwrap_or(OutputFormat::Text);
    anyhow::ensure!(warmup <= 100, "--warmup cannot exceed 100 requests");
    anyhow::ensure!(
        args.db.is_some() == args.index.is_some(),
        "--db and --index must be supplied together to compare retrieval strategies"
    );
    anyhow::ensure!(
        (1..=10_000).contains(&args.rrf_k),
        "--rrf-k must be between 1 and 10000"
    );
    for (name, weight) in [
        ("--dense-weight", args.dense_weight),
        ("--lexical-weight", args.lexical_weight),
        ("--fuzzy-weight", args.fuzzy_weight),
    ] {
        anyhow::ensure!(
            weight.is_finite() && weight >= 0.0,
            "{name} must be a finite non-negative number"
        );
    }
    anyhow::ensure!(
        args.dense_weight + args.lexical_weight + args.fuzzy_weight > 0.0,
        "at least one fusion weight must be greater than zero"
    );
    let resolved = crate::paths::resolve_embeddings(args.embeddings.as_deref())?;
    let embeddings = resolved.path;
    ensure_distinct_output(
        args.output.as_deref(),
        &embeddings,
        args.corpus.as_deref(),
        args.db.as_deref(),
        args.index.as_deref(),
    )?;
    let query_profile = crate::commands::embedding_profile::resolve(&args.model)?.id;
    let run_id = crate::commands::bench::run_id();
    let started_at = chrono::Utc::now().to_rfc3339();
    let (corpus, corpus_text, corpus_source) = read_corpus(args.corpus.as_deref())?;
    validate_corpus(&corpus)?;
    let metadata = semantic::read_arrow_metadata(&embeddings)?;
    let artifact_model_digest = metadata.get("sct.embedding_model_digest").cloned();
    let artifact_provenance = crate::provenance::from_arrow_metadata(&metadata);

    let queries: Vec<String> = corpus.cases.iter().map(|case| case.query.clone()).collect();
    let required_ids: Vec<String> = corpus
        .cases
        .iter()
        .flat_map(|case| case.expected_ids.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    for _ in 0..warmup {
        semantic::embed_queries_many(&args.ollama_url, &args.model, &queries)
            .context("warming up the semantic query model")?;
    }

    let embedding_started = Instant::now();
    let query_vectors = semantic::embed_queries_many(&args.ollama_url, &args.model, &queries)?;
    let query_embedding_ns = elapsed_ns(embedding_started);
    let identity_started = Instant::now();
    let ollama = crate::commands::ollama::inspect(&args.ollama_url, &args.model);
    let model_identity_check_ns = elapsed_ns(identity_started);
    let model_digest_verified = match (&artifact_model_digest, &ollama.model_digest) {
        (Some(built), Some(current)) => {
            anyhow::ensure!(
                built == current,
                "embeddings file {} was built with Ollama model digest {built}, but the current model digest is {current}",
                embeddings.display()
            );
            true
        }
        (Some(_), None) => anyhow::bail!(
            "embeddings file {} records an Ollama model digest, but the configured Ollama endpoint did not expose the current digest",
            embeddings.display()
        ),
        (None, _) => false,
    };

    let scan_started = Instant::now();
    let batch = semantic::search_embedding_vectors(
        &embeddings,
        &args.model,
        &query_vectors,
        args.limit,
        true,
        &required_ids,
    )?;
    let arrow_scan_batch_ns = elapsed_ns(scan_started);
    let total_batch_ns = query_embedding_ns.saturating_add(arrow_scan_batch_ns);

    anyhow::ensure!(
        batch.results.len() == corpus.cases.len(),
        "semantic search returned {} result sets for {} corpus cases",
        batch.results.len(),
        corpus.cases.len()
    );
    anyhow::ensure!(
        batch.missing_required_ids.is_empty(),
        "benchmark expected concepts are absent from {}: {}",
        embeddings.display(),
        batch.missing_required_ids.join(", ")
    );

    let comparison = match (&args.db, &args.index) {
        (Some(db), Some(index)) => Some(build_comparison(
            db,
            index,
            artifact_provenance.as_ref(),
            &corpus.cases,
            &batch.results,
            args.limit,
            total_batch_ns,
            FusionPolicy {
                method: "weighted-reciprocal-rank-fusion-v1",
                rrf_k: args.rrf_k,
                dense_weight: args.dense_weight,
                lexical_weight: args.lexical_weight,
                fuzzy_weight: args.fuzzy_weight,
            },
        )?),
        (None, None) => None,
        _ => unreachable!("paired comparison inputs validated above"),
    };
    let case_results = evaluate_cases(&corpus.cases, &batch.results);
    let metrics = calculate_metrics(case_results.iter());
    let metrics_by_class = metrics_by_class(&case_results);
    let provenance = (!args.no_provenance)
        .then_some(artifact_provenance.as_ref())
        .flatten();
    let case_count = case_results.len();

    let report = Report {
        schema_version: if comparison.is_some() {
            COMPARISON_REPORT_SCHEMA_VERSION
        } else {
            REPORT_SCHEMA_VERSION
        },
        run: RunInfo {
            run_id,
            started_at,
            tool: "sct bench semantic",
            sct_version: env!("CARGO_PKG_VERSION"),
            git_commit: option_env!("SCT_GIT_COMMIT"),
        },
        host: crate::commands::bench::host_info(),
        corpus: CorpusInfo {
            name: corpus.name,
            version: corpus.version,
            sha256: sha256(&corpus_text),
            source: corpus_source,
            case_count,
        },
        artifact: ArtifactInfo {
            file: file_name(&embeddings),
            bytes: std::fs::metadata(&embeddings)
                .with_context(|| format!("reading metadata for {}", embeddings.display()))?
                .len(),
            vector_count: batch.vector_count,
            dimensions: batch.dimensions,
            model: metadata.get("sct.embedding_model").cloned(),
            model_digest: artifact_model_digest,
            model_digest_verified,
            profile: metadata.get("sct.embedding_profile").cloned(),
            text_scheme: metadata.get("sct.embed_text_scheme").cloned(),
            edition: provenance.as_ref().map(|value| value.edition_label.clone()),
            release_date: provenance.as_ref().map(|value| value.release_date.clone()),
            release_id: provenance.as_ref().map(|value| value.release_id.clone()),
            content_fingerprint: provenance
                .as_ref()
                .and_then(|value| value.content_fingerprint.clone()),
            built_by_sct: metadata.get("sct.sct_version").cloned(),
            provenance_suppressed: args.no_provenance,
        },
        ollama,
        policy: Policy {
            strategy: "dense-concept-v1",
            query_model: args.model,
            query_profile: query_profile.to_string(),
            result_cutoff: args.limit,
            query_batch_size: case_count,
            ollama_warmup_requests: warmup,
            active_preference: false,
        },
        timings: Timings {
            query_embedding_ns,
            arrow_scan_batch_ns,
            total_batch_ns,
            model_identity_check_ns,
            amortized_query_embedding_ns_per_query: query_embedding_ns / case_count as u64,
            amortized_arrow_scan_ns_per_query: arrow_scan_batch_ns / case_count as u64,
            arrow_scan_samples: 1,
            cache_state: "uncontrolled",
            build_time_ns: None,
            peak_model_memory_bytes: None,
        },
        metrics,
        metrics_by_class,
        cases: case_results,
        comparison,
    };

    let rendered = match format {
        OutputFormat::Text => render_text(&report),
        OutputFormat::Json => serde_json::to_string_pretty(&report)? + "\n",
        OutputFormat::Yaml => serde_yaml_ng::to_string(&report)?,
    };
    match args.output {
        Some(path) => {
            std::fs::write(&path, rendered)
                .with_context(|| format!("writing report to {}", path.display()))?;
            eprintln!("Wrote {}", path.display());
        }
        None => print!("{rendered}"),
    }
    Ok(())
}

fn read_corpus(path: Option<&Path>) -> Result<(Corpus, String, String)> {
    let (text, source) = match path {
        Some(path) => (
            std::fs::read_to_string(path)
                .with_context(|| format!("reading semantic benchmark corpus {}", path.display()))?,
            file_name(path),
        ),
        None => (DEFAULT_CORPUS.to_string(), "built-in".to_string()),
    };
    let corpus =
        serde_yaml_ng::from_str(&text).context("parsing semantic benchmark corpus YAML")?;
    Ok((corpus, text, source))
}

fn ensure_distinct_output(
    output: Option<&Path>,
    embeddings: &Path,
    corpus: Option<&Path>,
    db: Option<&Path>,
    index: Option<&Path>,
) -> Result<()> {
    let Some(output) = output else {
        return Ok(());
    };
    let Ok(output) = output.canonicalize() else {
        return Ok(());
    };
    for (kind, input) in [
        ("embeddings artefact", Some(embeddings)),
        ("corpus", corpus),
        ("database", db),
        ("FST index", index),
    ] {
        if let Some(input) = input {
            anyhow::ensure!(
                input.canonicalize().ok().as_ref() != Some(&output),
                "--output must not overwrite the semantic benchmark {kind}"
            );
        }
    }
    Ok(())
}

fn validate_corpus(corpus: &Corpus) -> Result<()> {
    anyhow::ensure!(
        corpus.version == 1,
        "unsupported semantic corpus version {}",
        corpus.version
    );
    anyhow::ensure!(
        !corpus.name.trim().is_empty(),
        "semantic corpus name cannot be empty"
    );
    anyhow::ensure!(
        !corpus.cases.is_empty() && corpus.cases.len() <= MAX_CASES,
        "semantic corpus must contain between 1 and {MAX_CASES} cases"
    );
    let mut ids = BTreeSet::new();
    for case in &corpus.cases {
        anyhow::ensure!(
            !case.id.trim().is_empty(),
            "semantic case ID cannot be empty"
        );
        anyhow::ensure!(
            ids.insert(&case.id),
            "duplicate semantic case ID {:?}",
            case.id
        );
        anyhow::ensure!(
            !case.query.trim().is_empty(),
            "semantic case {:?} has an empty query",
            case.id
        );
        anyhow::ensure!(
            !case.expected_ids.is_empty(),
            "semantic case {:?} has no expected SCTIDs",
            case.id
        );
        anyhow::ensure!(
            !case.classes.is_empty(),
            "semantic case {:?} has no query classes",
            case.id
        );
        for expected in &case.expected_ids {
            anyhow::ensure!(
                crate::sctid::is_valid_sctid(expected),
                "semantic case {:?} contains invalid SCTID {:?}",
                case.id,
                expected
            );
        }
    }
    Ok(())
}

fn evaluate_cases(cases: &[CorpusCase], results: &[Vec<ScoredConcept>]) -> Vec<CaseResult> {
    cases
        .iter()
        .zip(results.iter())
        .map(|(case, results)| {
            let matched = results
                .iter()
                .enumerate()
                .find(|(_, result)| case.expected_ids.contains(&result.id));
            CaseResult {
                id: case.id.clone(),
                query: case.query.clone(),
                expected_ids: case.expected_ids.clone(),
                classes: case.classes.clone(),
                rank: matched.map(|(index, _)| index + 1),
                matched_id: matched.map(|(_, result)| result.id.clone()),
                expected_score: matched.map(|(_, result)| result.score),
                results: results.clone(),
            }
        })
        .collect()
}

fn calculate_metrics<'a>(cases: impl Iterator<Item = &'a CaseResult>) -> Metrics {
    let cases: Vec<_> = cases.collect();
    let count = cases.len();
    let rate = |cutoff| {
        cases
            .iter()
            .filter(|case| case.rank.is_some_and(|rank| rank <= cutoff))
            .count() as f64
            / count as f64
    };
    let scores: Vec<f64> = cases
        .iter()
        .filter_map(|case| case.expected_score.map(f64::from))
        .collect();
    Metrics {
        cases: count,
        top_1_rate: rate(1),
        top_5_rate: rate(5),
        top_10_rate: rate(10),
        mean_reciprocal_rank_at_cutoff: cases
            .iter()
            .filter_map(|case| case.rank.map(|rank| 1.0 / rank as f64))
            .fold(0.0, |sum, reciprocal_rank| sum + reciprocal_rank)
            / count as f64,
        mean_retrieved_expected_score: (!scores.is_empty())
            .then(|| scores.iter().sum::<f64>() / scores.len() as f64),
        retrieved_expected_scores: scores.len(),
    }
}

fn metrics_by_class(cases: &[CaseResult]) -> BTreeMap<QueryClass, Metrics> {
    let classes: BTreeSet<QueryClass> = cases
        .iter()
        .flat_map(|case| case.classes.iter().copied())
        .collect();
    classes
        .into_iter()
        .map(|class| {
            (
                class,
                calculate_metrics(cases.iter().filter(|case| case.classes.contains(&class))),
            )
        })
        .collect()
}

fn render_text(report: &Report) -> String {
    let mut output = format!(
        "sct bench semantic {}\n\n  Corpus     {} v{} ({} cases)\n  Artefact   {}, {} vectors x {} dimensions\n  Model      {}\n  Strategy   {}\n  Cutoff     {}\n\n",
        report.run.sct_version,
        report.corpus.name,
        report.corpus.version,
        report.corpus.case_count,
        report.artifact.file,
        report.artifact.vector_count,
        report.artifact.dimensions,
        report.policy.query_model,
        report.policy.strategy,
        report.policy.result_cutoff,
    );
    output.push_str(&format!(
        "  Profile    {} (artefact: {})\n  Ollama     {}{}, digest {}\n  Machine    {}, {} cores, {} ({}/{})\n",
        report.policy.query_profile,
        report
            .artifact
            .profile
            .as_deref()
            .unwrap_or("legacy/unrecorded"),
        report
            .ollama
            .version
            .as_deref()
            .unwrap_or("version unrecorded"),
        report
            .ollama
            .resolved_model
            .as_deref()
            .map(|model| format!(", {model}"))
            .unwrap_or_default(),
        if report.artifact.model_digest_verified {
            "verified against artefact"
        } else {
            "unverified (legacy artefact)"
        },
        report.host.cpu,
        report.host.logical_cores,
        report
            .host
            .memory_bytes
            .map(crate::humanize::human_bytes)
            .unwrap_or_else(|| "unknown RAM".to_string()),
        report.host.os,
        report.host.architecture,
    ));
    if report.artifact.provenance_suppressed {
        output.push_str("  Release    identity withheld\n");
    } else if let Some(edition) = &report.artifact.edition {
        output.push_str(&format!(
            "  Release    {} ({})\n",
            edition,
            report
                .artifact
                .release_date
                .as_deref()
                .unwrap_or("date unrecorded")
        ));
    }
    output.push('\n');
    output.push_str(&format!(
        "  Top 1      {:>6.1}%\n  Top 5      {:>6.1}%\n  Top 10     {:>6.1}%\n  MRR@{:<4}  {:>7.4}\n\n",
        report.metrics.top_1_rate * 100.0,
        report.metrics.top_5_rate * 100.0,
        report.metrics.top_10_rate * 100.0,
        report.policy.result_cutoff,
        report.metrics.mean_reciprocal_rank_at_cutoff,
    ));
    if let Some(comparison) = &report.comparison {
        output.push_str("\n  Retrieval comparison\n");
        output.push_str("    strategy      top 1    top 5   top 10      MRR\n");
        for strategy in [
            RetrievalStrategy::Dense,
            RetrievalStrategy::Lexical,
            RetrievalStrategy::Fuzzy,
            RetrievalStrategy::Hybrid,
        ] {
            let metrics = &comparison.strategies[&strategy].metrics;
            output.push_str(&format!(
                "    {:<10} {:>7.1}% {:>7.1}% {:>7.1}% {:>8.4}\n",
                strategy.label(),
                metrics.top_1_rate * 100.0,
                metrics.top_5_rate * 100.0,
                metrics.top_10_rate * 100.0,
                metrics.mean_reciprocal_rank_at_cutoff,
            ));
        }
        output.push_str(&format!(
            "\n    Hybrid uses weighted RRF (k={}; dense={}, lexical={}, fuzzy={})\n",
            comparison.policy.rrf_k,
            comparison.policy.dense_weight,
            comparison.policy.lexical_weight,
            comparison.policy.fuzzy_weight,
        ));
    }
    output.push_str("  Cases\n");
    for case in &report.cases {
        let rank = case
            .rank
            .map(|rank| format!("#{rank}"))
            .unwrap_or_else(|| format!(">{}", report.policy.result_cutoff));
        output.push_str(&format!(
            "    {:<28} {:>6}  {}\n",
            case.id, rank, case.query
        ));
    }
    output.push_str(&format!(
        "\n  Query embedding  {:.3} ms total\n  Arrow scan       {:.3} ms one-shot batch (uncontrolled cache)\n\nFull ranked evidence: --format json\n",
        report.timings.query_embedding_ns as f64 / 1_000_000.0,
        report.timings.arrow_scan_batch_ns as f64 / 1_000_000.0,
    ));
    output
}

impl RetrievalStrategy {
    fn label(self) -> &'static str {
        match self {
            Self::Dense => "dense",
            Self::Lexical => "lexical",
            Self::Fuzzy => "fuzzy",
            Self::Hybrid => "hybrid",
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_comparison(
    db: &Path,
    index_path: &Path,
    embedding_provenance: Option<&crate::provenance::Provenance>,
    cases: &[CorpusCase],
    dense_results: &[Vec<ScoredConcept>],
    limit: usize,
    dense_batch_ns: u64,
    policy: FusionPolicy,
) -> Result<ComparisonReport> {
    let snomed = Snomed::open(db).with_context(|| format!("opening {}", db.display()))?;
    let index = Index::open(index_path)
        .with_context(|| format!("opening FST index {}", index_path.display()))?;
    verify_same_content(
        snomed.provenance(),
        embedding_provenance,
        "database",
        "embeddings artefact",
    )?;
    verify_same_content(
        snomed.provenance(),
        index.provenance(),
        "database",
        "FST index",
    )?;

    let mut missing_db_ids = Vec::new();
    for id in cases
        .iter()
        .flat_map(|case| case.expected_ids.iter())
        .collect::<BTreeSet<_>>()
    {
        if snomed.concept(id)?.is_none() {
            missing_db_ids.push(id.clone());
        }
    }
    anyhow::ensure!(
        missing_db_ids.is_empty(),
        "benchmark expected concepts are absent from {}: {}",
        db.display(),
        missing_db_ids.join(", ")
    );

    let lexical_started = Instant::now();
    let lexical_results: Vec<Vec<ComparisonCandidate>> = cases
        .iter()
        .map(|case| {
            snomed
                .search(&case.query, limit as u32)
                .map(|hits| hits.into_iter().map(ComparisonCandidate::lexical).collect())
        })
        .collect::<Result<_, _>>()?;
    let lexical_batch_ns = elapsed_ns(lexical_started);

    let fuzzy_started = Instant::now();
    let fuzzy_results: Vec<Vec<ComparisonCandidate>> = cases
        .iter()
        .map(|case| {
            index
                .search_typeahead(&case.query, limit, true)
                .into_iter()
                .map(ComparisonCandidate::fuzzy)
                .collect()
        })
        .collect();
    let fuzzy_batch_ns = elapsed_ns(fuzzy_started);

    let dense_candidates: Vec<Vec<ComparisonCandidate>> = dense_results
        .iter()
        .map(|results| {
            results
                .iter()
                .cloned()
                .map(ComparisonCandidate::dense)
                .collect()
        })
        .collect();
    let fusion_started = Instant::now();
    let hybrid_results: Vec<Vec<ComparisonCandidate>> = dense_candidates
        .iter()
        .zip(&lexical_results)
        .zip(&fuzzy_results)
        .map(|((dense, lexical), fuzzy)| fuse_rankings(dense, lexical, fuzzy, limit, policy))
        .collect();
    let hybrid_batch_ns = elapsed_ns(fusion_started);

    let mut strategies = BTreeMap::new();
    for (strategy, results) in [
        (RetrievalStrategy::Dense, dense_candidates),
        (RetrievalStrategy::Lexical, lexical_results),
        (RetrievalStrategy::Fuzzy, fuzzy_results),
        (RetrievalStrategy::Hybrid, hybrid_results),
    ] {
        strategies.insert(strategy, evaluate_strategy(cases, results));
    }

    Ok(ComparisonReport {
        artifacts: ComparisonArtifacts {
            database_file: file_name(db),
            database_bytes: file_size(db)?,
            fst_index_file: file_name(index_path),
            fst_index_bytes: file_size(index_path)?,
            release_id_verified: true,
            content_fingerprint_verified: true,
        },
        policy,
        timings_ns: BTreeMap::from([
            (RetrievalStrategy::Dense, dense_batch_ns),
            (RetrievalStrategy::Lexical, lexical_batch_ns),
            (RetrievalStrategy::Fuzzy, fuzzy_batch_ns),
            (RetrievalStrategy::Hybrid, hybrid_batch_ns),
        ]),
        strategies,
    })
}

fn verify_same_content(
    left: Option<&crate::provenance::Provenance>,
    right: Option<&crate::provenance::Provenance>,
    left_name: &str,
    right_name: &str,
) -> Result<()> {
    let left = left.with_context(|| format!("{left_name} has no release provenance"))?;
    let right = right.with_context(|| format!("{right_name} has no release provenance"))?;
    anyhow::ensure!(
        !left.release_id.is_empty() && !right.release_id.is_empty(),
        "{left_name} and {right_name} must record release identifiers"
    );
    anyhow::ensure!(
        left.release_id == right.release_id,
        "{left_name} release {:?} does not match {right_name} release {:?}",
        left.release_id,
        right.release_id
    );
    let left_fingerprint = left
        .content_fingerprint
        .as_deref()
        .with_context(|| format!("{left_name} has no content fingerprint"))?;
    let right_fingerprint = right
        .content_fingerprint
        .as_deref()
        .with_context(|| format!("{right_name} has no content fingerprint"))?;
    anyhow::ensure!(
        left_fingerprint == right_fingerprint,
        "{left_name} content fingerprint does not match {right_name}"
    );
    Ok(())
}

impl ComparisonCandidate {
    fn dense(concept: ScoredConcept) -> Self {
        Self {
            id: concept.id,
            preferred_term: concept.preferred_term,
            score: Some(f64::from(concept.score)),
            source_ranks: BTreeMap::new(),
        }
    }

    fn lexical(concept: SearchHit) -> Self {
        Self {
            id: concept.id,
            preferred_term: concept.preferred_term,
            score: None,
            source_ranks: BTreeMap::new(),
        }
    }

    fn fuzzy(concept: crate::index::Hit) -> Self {
        Self {
            id: concept.concept_id.to_string(),
            preferred_term: concept.term,
            score: Some(f64::from(concept.score)),
            source_ranks: BTreeMap::new(),
        }
    }
}

fn fuse_rankings(
    dense: &[ComparisonCandidate],
    lexical: &[ComparisonCandidate],
    fuzzy: &[ComparisonCandidate],
    limit: usize,
    policy: FusionPolicy,
) -> Vec<ComparisonCandidate> {
    struct Fused {
        preferred_term: String,
        score: f64,
        source_ranks: BTreeMap<RetrievalStrategy, usize>,
    }

    let mut fused: HashMap<String, Fused> = HashMap::new();
    for (strategy, weight, ranking) in [
        (RetrievalStrategy::Dense, policy.dense_weight, dense),
        (RetrievalStrategy::Lexical, policy.lexical_weight, lexical),
        (RetrievalStrategy::Fuzzy, policy.fuzzy_weight, fuzzy),
    ] {
        if weight == 0.0 {
            continue;
        }
        for (index, candidate) in ranking.iter().enumerate() {
            let rank = index + 1;
            let entry = fused.entry(candidate.id.clone()).or_insert_with(|| Fused {
                preferred_term: candidate.preferred_term.clone(),
                score: 0.0,
                source_ranks: BTreeMap::new(),
            });
            entry.score += weight / (policy.rrf_k + rank) as f64;
            entry.source_ranks.insert(strategy, rank);
        }
    }

    let mut results: Vec<_> = fused
        .into_iter()
        .map(|(id, candidate)| ComparisonCandidate {
            id,
            preferred_term: candidate.preferred_term,
            score: Some(candidate.score),
            source_ranks: candidate.source_ranks,
        })
        .collect();
    results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    results.truncate(limit);
    results
}

fn evaluate_strategy(
    cases: &[CorpusCase],
    results: Vec<Vec<ComparisonCandidate>>,
) -> StrategyEvaluation {
    let cases: Vec<_> = cases
        .iter()
        .zip(results)
        .map(|(case, results)| {
            let matched = results
                .iter()
                .enumerate()
                .find(|(_, result)| case.expected_ids.contains(&result.id));
            StrategyCaseResult {
                id: case.id.clone(),
                query: case.query.clone(),
                expected_ids: case.expected_ids.clone(),
                classes: case.classes.clone(),
                rank: matched.map(|(index, _)| index + 1),
                matched_id: matched.map(|(_, result)| result.id.clone()),
                results,
            }
        })
        .collect();
    let metrics = calculate_ranking_metrics(cases.iter());
    let classes: BTreeSet<_> = cases
        .iter()
        .flat_map(|case| case.classes.iter().copied())
        .collect();
    let metrics_by_class = classes
        .into_iter()
        .map(|class| {
            (
                class,
                calculate_ranking_metrics(
                    cases.iter().filter(|case| case.classes.contains(&class)),
                ),
            )
        })
        .collect();
    StrategyEvaluation {
        metrics,
        metrics_by_class,
        cases,
    }
}

fn calculate_ranking_metrics<'a>(
    cases: impl Iterator<Item = &'a StrategyCaseResult>,
) -> RankingMetrics {
    let cases: Vec<_> = cases.collect();
    let count = cases.len();
    let rate = |cutoff| {
        cases
            .iter()
            .filter(|case| case.rank.is_some_and(|rank| rank <= cutoff))
            .count() as f64
            / count as f64
    };
    RankingMetrics {
        cases: count,
        top_1_rate: rate(1),
        top_5_rate: rate(5),
        top_10_rate: rate(10),
        mean_reciprocal_rank_at_cutoff: cases
            .iter()
            .filter_map(|case| case.rank.map(|rank| 1.0 / rank as f64))
            .sum::<f64>()
            / count as f64,
    }
}

fn elapsed_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
}

fn sha256(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "artifact".to_string())
}

fn file_size(path: &Path) -> Result<u64> {
    std::fs::metadata(path)
        .with_context(|| format!("reading metadata for {}", path.display()))
        .map(|metadata| metadata.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(rank: Option<usize>, score: Option<f32>, classes: Vec<QueryClass>) -> CaseResult {
        CaseResult {
            id: "case".into(),
            query: "query".into(),
            expected_ids: vec!["22298006".into()],
            classes,
            rank,
            matched_id: rank.map(|_| "22298006".into()),
            expected_score: score,
            results: Vec::new(),
        }
    }

    #[test]
    fn built_in_corpus_is_valid() {
        let corpus: Corpus = serde_yaml_ng::from_str(DEFAULT_CORPUS).unwrap();
        validate_corpus(&corpus).unwrap();
    }

    #[test]
    fn corpus_rejects_duplicate_case_ids() {
        let mut corpus: Corpus = serde_yaml_ng::from_str(DEFAULT_CORPUS).unwrap();
        corpus.cases[1].id = corpus.cases[0].id.clone();
        assert!(validate_corpus(&corpus).is_err());
    }

    #[test]
    fn report_output_cannot_replace_an_input() {
        let embeddings = tempfile::NamedTempFile::new().unwrap();
        let corpus = tempfile::NamedTempFile::new().unwrap();
        assert!(ensure_distinct_output(
            Some(embeddings.path()),
            embeddings.path(),
            None,
            None,
            None,
        )
        .is_err());
        assert!(ensure_distinct_output(
            Some(corpus.path()),
            embeddings.path(),
            Some(corpus.path()),
            None,
            None,
        )
        .is_err());
    }

    #[test]
    fn weighted_rrf_rewards_candidates_found_by_multiple_strategies() {
        let candidate = |id: &str| ComparisonCandidate {
            id: id.into(),
            preferred_term: id.into(),
            score: None,
            source_ranks: BTreeMap::new(),
        };
        let dense = vec![candidate("dense-only"), candidate("shared")];
        let lexical = vec![candidate("shared"), candidate("lexical-only")];
        let fuzzy = vec![candidate("fuzzy-only"), candidate("shared")];
        let fused = fuse_rankings(
            &dense,
            &lexical,
            &fuzzy,
            4,
            FusionPolicy {
                method: "weighted-reciprocal-rank-fusion-v1",
                rrf_k: 60,
                dense_weight: 1.0,
                lexical_weight: 1.0,
                fuzzy_weight: 1.0,
            },
        );

        assert_eq!(fused[0].id, "shared");
        assert_eq!(fused[0].source_ranks.len(), 3);
        assert_eq!(fused[0].source_ranks[&RetrievalStrategy::Lexical], 1);
    }

    #[test]
    fn weighted_rrf_can_ablate_a_source() {
        let candidate = |id: &str| ComparisonCandidate {
            id: id.into(),
            preferred_term: id.into(),
            score: None,
            source_ranks: BTreeMap::new(),
        };
        let fused = fuse_rankings(
            &[candidate("dense")],
            &[candidate("lexical")],
            &[],
            2,
            FusionPolicy {
                method: "weighted-reciprocal-rank-fusion-v1",
                rrf_k: 60,
                dense_weight: 0.0,
                lexical_weight: 1.0,
                fuzzy_weight: 0.0,
            },
        );

        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "lexical");
    }

    #[test]
    fn metrics_count_censored_ranks_as_misses() {
        let cases = [
            result(Some(1), Some(0.9), vec![QueryClass::SynonymDilution]),
            result(Some(6), Some(0.5), vec![QueryClass::Misspelling]),
            result(None, None, vec![QueryClass::Idiom]),
        ];
        let metrics = calculate_metrics(cases.iter());
        assert_eq!(metrics.top_1_rate, 1.0 / 3.0);
        assert_eq!(metrics.top_5_rate, 1.0 / 3.0);
        assert_eq!(metrics.top_10_rate, 2.0 / 3.0);
        assert!(
            (metrics.mean_reciprocal_rank_at_cutoff - (1.0 + 1.0 / 6.0) / 3.0).abs() < f64::EPSILON
        );
        assert_eq!(metrics.retrieved_expected_scores, 2);
    }
}
