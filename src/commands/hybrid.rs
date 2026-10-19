// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct search hybrid` - experimental weighted reciprocal-rank fusion of the
//! dense (meaning), lexical (keyword) and fuzzy (typo-tolerant) rankings.
//!
//! This module also owns the fusion primitives that `sct bench semantic`
//! compares against the guideline corpus, so the command and the benchmark
//! that justifies its defaults share one implementation.
//!
//! Examples:
//!   sct search hybrid "heart attack"
//!   sct search hybrid "sugar diabetes" --limit 20 --format json
//!   sct search hybrid "tummy ache" --dense-weight 2 --fuzzy-weight 0.5

use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::commands::batch::{self, LineMode, ResultBudget};
use crate::commands::semantic::{self, ScoredConcept};
use crate::format::{single_line, INACTIVE_MARKER};
use crate::index::Index;
use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, Provenance, ProvenanceFlags};
use crate::sdk::{SearchHit, Snomed};

pub(crate) const FUSION_METHOD: &str = "weighted-reciprocal-rank-fusion-v1";
const MAX_LIMIT: usize = 1_000;
const MAX_QUERIES: usize = 100;

#[derive(Parser, Debug)]
pub struct Args {
    /// Search query. Pass `-` to read one query per line from stdin (maximum
    /// 100 queries).
    pub query: String,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// FST index produced by `sct fst build` from the same release as the
    /// database. Defaults to the `.fst` beside the database with the same
    /// name, then `snomed.fst` or the newest `*.fst` in that directory or the
    /// working directory.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub index: Option<PathBuf>,

    /// Arrow IPC embeddings file produced by `sct embed`. See
    /// `docs/path-resolution.md` for the discovery order when omitted.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub embeddings: Option<PathBuf>,

    /// Supported Ollama embedding model - must match the model used by
    /// `sct embed`.
    #[arg(long, default_value = "nomic-embed-text")]
    pub model: String,

    /// Ollama API base URL.
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// Maximum number of fused results to return per query (maximum: 1000).
    #[arg(long, short, default_value = "10")]
    pub limit: usize,

    /// How many results each of the three searches contributes before they
    /// are fused (maximum: 1000). Raised to `--limit` if smaller.
    #[arg(long, default_value = "100")]
    pub depth: usize,

    /// Reciprocal-rank-fusion constant `k`. Each result earns
    /// `weight / (k + position)` points from each search that finds it, so a
    /// small `k` rewards top positions heavily and a large `k` flattens them.
    #[arg(long, default_value = "60")]
    pub rrf_k: usize,

    /// Weight of the meaning-based (Ollama embedding) ranking. 0 leaves it out.
    #[arg(long, default_value = "1.0")]
    pub dense_weight: f64,

    /// Weight of the keyword (FTS5) ranking. 0 leaves it out.
    #[arg(long, default_value = "1.0")]
    pub lexical_weight: f64,

    /// Weight of the typo-tolerant (FST) ranking. 0 leaves it out.
    #[arg(long, default_value = "1.0")]
    pub fuzzy_weight: f64,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Emit only matching SCTIDs (newline-delimited) for piping.
    #[arg(long, conflicts_with = "format")]
    pub ids: bool,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RetrievalStrategy {
    Dense,
    Lexical,
    Fuzzy,
    Hybrid,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct FusionPolicy {
    pub method: &'static str,
    pub rrf_k: usize,
    pub dense_weight: f64,
    pub lexical_weight: f64,
    pub fuzzy_weight: f64,
}

impl FusionPolicy {
    pub(crate) fn new(
        rrf_k: usize,
        dense_weight: f64,
        lexical_weight: f64,
        fuzzy_weight: f64,
    ) -> Result<Self> {
        anyhow::ensure!(
            (1..=10_000).contains(&rrf_k),
            "--rrf-k must be between 1 and 10000"
        );
        for (name, weight) in [
            ("--dense-weight", dense_weight),
            ("--lexical-weight", lexical_weight),
            ("--fuzzy-weight", fuzzy_weight),
        ] {
            anyhow::ensure!(
                weight.is_finite() && weight >= 0.0,
                "{name} must be a finite non-negative number"
            );
        }
        anyhow::ensure!(
            dense_weight + lexical_weight + fuzzy_weight > 0.0,
            "at least one fusion weight must be greater than zero"
        );
        Ok(Self {
            method: FUSION_METHOD,
            rrf_k,
            dense_weight,
            lexical_weight,
            fuzzy_weight,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ComparisonCandidate {
    pub id: String,
    pub preferred_term: String,
    /// Source-native score. Lexical FTS exposes only rank, so this is null for
    /// lexical candidates and must not be compared across strategies.
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub source_ranks: BTreeMap<RetrievalStrategy, usize>,
    #[serde(skip)]
    pub active: bool,
}

impl ComparisonCandidate {
    pub(crate) fn dense(concept: ScoredConcept) -> Self {
        Self {
            id: concept.id,
            preferred_term: concept.preferred_term,
            score: Some(f64::from(concept.score)),
            source_ranks: BTreeMap::new(),
            active: concept.active,
        }
    }

    pub(crate) fn lexical(concept: SearchHit) -> Self {
        Self {
            id: concept.id,
            preferred_term: concept.preferred_term,
            score: None,
            source_ranks: BTreeMap::new(),
            active: concept.active,
        }
    }

    pub(crate) fn fuzzy(concept: crate::index::Hit) -> Self {
        Self {
            id: concept.concept_id.to_string(),
            preferred_term: concept.term,
            score: Some(f64::from(concept.score)),
            source_ranks: BTreeMap::new(),
            active: concept.active,
        }
    }
}

pub(crate) fn verify_same_content(
    left: Option<&Provenance>,
    right: Option<&Provenance>,
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

pub(crate) fn fuse_rankings(
    dense: &[ComparisonCandidate],
    lexical: &[ComparisonCandidate],
    fuzzy: &[ComparisonCandidate],
    limit: usize,
    policy: FusionPolicy,
) -> Vec<ComparisonCandidate> {
    struct Fused {
        preferred_term: String,
        score: f64,
        active: bool,
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
                active: candidate.active,
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
            active: candidate.active,
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

pub fn run(args: Args) -> Result<()> {
    anyhow::ensure!(
        (1..=MAX_LIMIT).contains(&args.limit),
        "--limit must be between 1 and {MAX_LIMIT}"
    );
    anyhow::ensure!(
        (1..=MAX_LIMIT).contains(&args.depth),
        "--depth must be between 1 and {MAX_LIMIT}"
    );
    let policy = FusionPolicy::new(
        args.rrf_k,
        args.dense_weight,
        args.lexical_weight,
        args.fuzzy_weight,
    )?;
    let depth = args.depth.max(args.limit);

    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let embeddings = crate::paths::resolve_embeddings(args.embeddings.as_deref())?.path;
    let index_path = resolve_index(args.index.as_deref(), &db)?;
    let snomed = Snomed::open(&db).with_context(|| format!("opening {}", db.display()))?;
    let index = Index::open(&index_path)
        .with_context(|| format!("opening FST index {}", index_path.display()))?;
    let embedding_provenance = semantic::read_arrow_provenance(&embeddings)?;
    verify_same_content(
        snomed.provenance(),
        embedding_provenance.as_ref(),
        "database",
        "embeddings artefact",
    )?;
    verify_same_content(
        snomed.provenance(),
        index.provenance(),
        "database",
        "FST index",
    )?;
    let prov = snomed.provenance().cloned();

    let batch_mode = args.query == "-";
    let queries = if batch_mode {
        batch::read_stdin_limited(LineMode::Whole, "queries", MAX_QUERIES)?
    } else {
        vec![args.query.clone()]
    };

    let dense: Vec<Vec<ComparisonCandidate>> = if policy.dense_weight > 0.0 {
        semantic::semantic_search_many(
            &embeddings,
            &args.ollama_url,
            &args.model,
            &queries,
            depth,
        )
        .context(
            "the meaning-based part of hybrid search failed; `sct search hybrid` does not fall \
             back to a weaker search - start Ollama, or pass --dense-weight 0 to leave it out",
        )?
        .into_iter()
        .map(|results| results.into_iter().map(ComparisonCandidate::dense).collect())
        .collect()
    } else {
        vec![Vec::new(); queries.len()]
    };

    let mut lexical: Vec<Vec<ComparisonCandidate>> = Vec::with_capacity(queries.len());
    for query in &queries {
        lexical.push(if policy.lexical_weight > 0.0 {
            snomed
                .search(query, depth as u32)?
                .into_iter()
                .map(ComparisonCandidate::lexical)
                .collect()
        } else {
            Vec::new()
        });
    }

    let fuzzy: Vec<Vec<ComparisonCandidate>> = queries
        .iter()
        .map(|query| {
            if policy.fuzzy_weight > 0.0 {
                index
                    .search_typeahead(query, depth, true)
                    .into_iter()
                    .map(ComparisonCandidate::fuzzy)
                    .collect()
            } else {
                Vec::new()
            }
        })
        .collect();

    let mut budget = ResultBudget::new();
    let mut fused = Vec::with_capacity(queries.len());
    for ((dense, lexical), fuzzy) in dense.iter().zip(&lexical).zip(&fuzzy) {
        let results = fuse_rankings(dense, lexical, fuzzy, args.limit, policy);
        budget.retain(results.len(), "hybrid search")?;
        fused.push(results);
    }

    write_output(&args, &queries, &fused, policy, prov.as_ref(), batch_mode)
}

fn resolve_index(arg: Option<&Path>, db: &Path) -> Result<PathBuf> {
    if let Some(path) = arg {
        return Ok(path.to_path_buf());
    }
    let sibling = db.with_extension("fst");
    if sibling.exists() {
        return Ok(sibling);
    }
    let beside_db = db
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    crate::paths::find_fst_index(beside_db)
        .or_else(|| crate::paths::find_fst_index(Path::new(".")))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No FST index found beside {} or in the current directory.\n\
                 Build one with `sct fst build --ndjson <file>`, or pass --index <path>.",
                db.display()
            )
        })
}

fn write_output(
    args: &Args,
    queries: &[String],
    fused: &[Vec<ComparisonCandidate>],
    policy: FusionPolicy,
    prov: Option<&Provenance>,
    batch_mode: bool,
) -> Result<()> {
    if args.ids {
        use std::io::Write;
        for id in fused.iter().flatten().map(|candidate| &candidate.id) {
            crate::sctid::validate_syntax(id)?;
        }
        let mut out = std::io::stdout().lock();
        for candidate in fused.iter().flatten() {
            writeln!(out, "{}", candidate.id)?;
        }
        return Ok(());
    }

    let mode = if args.format.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    if args.format.is_structured() {
        let mut value = if batch_mode {
            let items: Vec<Value> = queries
                .iter()
                .zip(fused)
                .map(|(query, results)| {
                    json!({
                        "input": query,
                        "result": results.iter().map(candidate_json).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({ "items": items })
        } else {
            json!({ "results": fused[0].iter().map(candidate_json).collect::<Vec<_>>() })
        };
        value["fusion"] = serde_json::to_value(policy)?;
        provenance::inject_into_json(&mut value, prov, show_prov);
        args.format.print(&value)?;
        return Ok(());
    }

    eprintln!(
        "note: `sct search hybrid` is experimental; equal weights and k={} follow the R62 \
         evidence, not a clinical guarantee",
        policy.rrf_k
    );
    for (query, results) in queries.iter().zip(fused) {
        if results.is_empty() {
            eprintln!("No results for {query:?}");
        }
        for candidate in results {
            println!("{}", render_line(candidate));
        }
    }
    provenance::print_human_footer(prov, show_prov);
    Ok(())
}

fn candidate_json(candidate: &ComparisonCandidate) -> Value {
    json!({
        "id": candidate.id,
        "preferred_term": candidate.preferred_term,
        "score": candidate.score,
        "active": candidate.active,
        "source_ranks": candidate.source_ranks,
    })
}

/// `[marker]<id>  <term>  [dense #3, lexical #1]` - the bracket shows which
/// searches found the concept and at what position, so a surprising result
/// can be traced to the search that surfaced it.
fn render_line(candidate: &ComparisonCandidate) -> String {
    let sources = candidate
        .source_ranks
        .iter()
        .map(|(strategy, rank)| format!("{} #{rank}", strategy.label()))
        .collect::<Vec<_>>()
        .join(", ");
    let marker = if candidate.active {
        ""
    } else {
        INACTIVE_MARKER
    };
    format!(
        "{marker}{:<18}  {}  [{sources}]",
        candidate.id,
        single_line(&candidate.preferred_term)
    )
}

impl RetrievalStrategy {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Dense => "dense",
            Self::Lexical => "lexical",
            Self::Fuzzy => "fuzzy",
            Self::Hybrid => "hybrid",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str) -> ComparisonCandidate {
        ComparisonCandidate {
            id: id.into(),
            preferred_term: id.into(),
            score: None,
            source_ranks: BTreeMap::new(),
            active: true,
        }
    }

    fn equal_policy() -> FusionPolicy {
        FusionPolicy::new(60, 1.0, 1.0, 1.0).unwrap()
    }

    #[test]
    fn weighted_rrf_rewards_candidates_found_by_multiple_strategies() {
        let dense = vec![candidate("dense-only"), candidate("shared")];
        let lexical = vec![candidate("shared"), candidate("lexical-only")];
        let fuzzy = vec![candidate("fuzzy-only"), candidate("shared")];
        let fused = fuse_rankings(&dense, &lexical, &fuzzy, 4, equal_policy());

        assert_eq!(fused[0].id, "shared");
        assert_eq!(fused[0].source_ranks.len(), 3);
        assert_eq!(fused[0].source_ranks[&RetrievalStrategy::Lexical], 1);
    }

    #[test]
    fn weighted_rrf_can_ablate_a_source() {
        let policy = FusionPolicy::new(60, 0.0, 1.0, 0.0).unwrap();
        let fused = fuse_rankings(
            &[candidate("dense")],
            &[candidate("lexical")],
            &[],
            2,
            policy,
        );

        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "lexical");
    }

    #[test]
    fn weights_move_the_ranking() {
        let dense = vec![candidate("a"), candidate("b")];
        let lexical = vec![candidate("b"), candidate("a")];
        let favour_dense = FusionPolicy::new(60, 3.0, 1.0, 0.0).unwrap();
        let favour_lexical = FusionPolicy::new(60, 1.0, 3.0, 0.0).unwrap();

        assert_eq!(
            fuse_rankings(&dense, &lexical, &[], 2, favour_dense)[0].id,
            "a"
        );
        assert_eq!(
            fuse_rankings(&dense, &lexical, &[], 2, favour_lexical)[0].id,
            "b"
        );
    }

    #[test]
    fn ties_break_by_concept_id_and_limit_truncates() {
        let fused = fuse_rankings(&[candidate("2")], &[candidate("1")], &[], 1, equal_policy());
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "1");
    }

    #[test]
    fn inactive_flag_survives_fusion() {
        let mut retired = candidate("1");
        retired.active = false;
        let fused = fuse_rankings(&[retired], &[], &[], 1, equal_policy());
        assert!(!fused[0].active);
        assert!(render_line(&fused[0]).starts_with(INACTIVE_MARKER));
    }

    #[test]
    fn policy_rejects_invalid_values() {
        assert!(FusionPolicy::new(0, 1.0, 1.0, 1.0).is_err());
        assert!(FusionPolicy::new(10_001, 1.0, 1.0, 1.0).is_err());
        assert!(FusionPolicy::new(60, -1.0, 1.0, 1.0).is_err());
        assert!(FusionPolicy::new(60, f64::NAN, 1.0, 1.0).is_err());
        assert!(FusionPolicy::new(60, f64::INFINITY, 1.0, 1.0).is_err());
        assert!(FusionPolicy::new(60, 0.0, 0.0, 0.0).is_err());
        assert!(FusionPolicy::new(60, 0.0, 0.0, 0.5).is_ok());
    }

    #[test]
    fn render_line_shows_which_searches_found_it() {
        let mut found = candidate("22298006");
        found.source_ranks = BTreeMap::from([
            (RetrievalStrategy::Dense, 3),
            (RetrievalStrategy::Lexical, 1),
        ]);
        let line = render_line(&found);
        assert!(line.contains("22298006"));
        assert!(line.ends_with("[dense #3, lexical #1]"));
    }

    #[test]
    fn index_resolution_prefers_the_database_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("release.db");
        std::fs::write(&db, b"").unwrap();
        std::fs::write(dir.path().join("snomed.fst"), b"").unwrap();
        std::fs::write(dir.path().join("release.fst"), b"").unwrap();

        assert_eq!(
            resolve_index(None, &db).unwrap(),
            dir.path().join("release.fst")
        );
        let explicit = dir.path().join("other.fst");
        assert_eq!(resolve_index(Some(&explicit), &db).unwrap(), explicit);
    }
}
