// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct semantic` - Semantic similarity search over a SNOMED CT Arrow IPC embeddings file.
//!
//! Embeds the query text via Ollama, then performs cosine similarity against
//! every concept embedding in the Arrow IPC file produced by `sct embed`.
//! Returns the top-N most semantically similar concepts.
//!
//! Examples:
//!   sct semantic --embeddings snomed-embeddings.arrow "heart attack"
//!   sct semantic --embeddings snomed-embeddings.arrow "difficulty breathing" --limit 20
//!   sct semantic --embeddings snomed-embeddings.arrow "beta blocker" --model nomic-embed-text

use anyhow::{Context, Result};
use arrow::array::{AsArray, StringArray};
use arrow::datatypes::Float32Type;
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use clap::Parser;
use serde::Serialize;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::commands::batch::{self, BatchItem, LineMode};
use crate::commands::embedding_profile;
use crate::format::{ConceptFields, ConceptFormat};
use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, Provenance, ProvenanceFlags};

#[derive(Parser, Debug)]
pub struct Args {
    /// Natural-language search query. Pass `-` to read one query per line from
    /// stdin.
    pub query: String,

    /// Arrow IPC embeddings file produced by `sct embed`.
    /// See `docs/path-resolution.md` for the discovery order when omitted.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub embeddings: Option<PathBuf>,

    /// Supported Ollama embedding model - must match the model used by
    /// `sct embed`: nomic-embed-text, nomic-embed-text:v1.5,
    /// nomic-embed-text-v2-moe, qwen3-embedding:0.6b, or embeddinggemma.
    #[arg(long, default_value = "nomic-embed-text")]
    pub model: String,

    /// Ollama API base URL.
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// Maximum number of results to return per query (maximum: 1000).
    #[arg(long, short, default_value = "10")]
    pub limit: usize,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Emit only matching SCTIDs (newline-delimited) for piping.
    #[arg(long, conflicts_with = "format")]
    pub ids: bool,

    /// Override the per-result line template (text output only).
    /// Default: `{score} | {id} | {pt}`. See `docs/commands/refset.md`.
    #[arg(long)]
    pub template: Option<String>,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ScoredConcept {
    pub score: f32,
    pub id: String,
    pub preferred_term: String,
    /// False when SNOMED International has retired this concept. Always
    /// `true` for an embeddings file written before this field existed, or
    /// one built without `sct ndjson --include-inactive`.
    pub active: bool,
}

pub(crate) struct SemanticSearchBatch {
    pub results: Vec<Vec<ScoredConcept>>,
    pub vector_count: usize,
    pub dimensions: usize,
    pub missing_required_ids: Vec<String>,
}

const MAX_RESULTS: usize = 1_000;
const MAX_BATCH_QUERIES: usize = 100;

#[derive(Debug)]
struct RankedConcept(ScoredConcept);

impl PartialEq for RankedConcept {
    fn eq(&self, other: &Self) -> bool {
        rank_cmp(&self.0, &other.0).is_eq()
    }
}

impl Eq for RankedConcept {}

impl PartialOrd for RankedConcept {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedConcept {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        rank_cmp(&self.0, &other.0)
    }
}

// ---------------------------------------------------------------------------
// Ollama request/response
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(serde::Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(args: Args) -> Result<()> {
    validate_limit(args.limit)?;
    let embeddings = crate::paths::resolve_embeddings(args.embeddings.as_deref())?.path;
    let prov = read_arrow_provenance(&embeddings).unwrap_or(None);
    let out = args.format;
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    if args.query == "-" {
        return run_batch(&embeddings, &args, prov.as_ref(), show_prov);
    }

    // `--ids`: machine output for pipes - just SCTIDs on stdout.
    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for id in semantic_search_ids(
            &embeddings,
            &args.ollama_url,
            &args.model,
            &args.query,
            args.limit,
        )? {
            writeln!(out, "{id}")?;
        }
        return Ok(());
    }

    let results = semantic_search(
        &embeddings,
        &args.ollama_url,
        &args.model,
        &args.query,
        args.limit,
    )?;

    if results.is_empty() && !out.is_structured() {
        eprintln!("No embeddings found in {}", embeddings.display());
        return Ok(());
    }

    if out.is_structured() {
        let items: Vec<Value> = results
            .iter()
            .map(|c| {
                json!({
                    "score": c.score, "id": c.id, "preferred_term": c.preferred_term,
                    "active": c.active,
                })
            })
            .collect();
        let value = if show_prov {
            let mut v = json!({ "results": items });
            provenance::inject_into_json(&mut v, prov.as_ref(), true);
            v
        } else {
            Value::Array(items)
        };
        if let Some(s) = out.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    let format = ConceptFormat {
        line: "{score} | {id} | {pt}".into(),
        fsn_suffix: String::new(),
    }
    .with_overrides(args.template, Some(String::new()));

    for ScoredConcept {
        score,
        id,
        preferred_term,
        active,
    } in &results
    {
        println!(
            "{}",
            format.render(&ConceptFields {
                id,
                pt: preferred_term,
                score: Some(*score as f64),
                inactive: !active,
                ..Default::default()
            })
        );
    }

    provenance::print_human_footer(prov.as_ref(), show_prov);

    Ok(())
}

fn run_batch(
    embeddings: &Path,
    args: &Args,
    prov: Option<&Provenance>,
    show_prov: bool,
) -> Result<()> {
    let queries = batch::read_stdin_limited(LineMode::Whole, "queries", MAX_BATCH_QUERIES)?;
    if args.ids {
        let result_sets = semantic_search_ids_many(
            embeddings,
            &args.ollama_url,
            &args.model,
            &queries,
            args.limit,
        )?;
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for ids in result_sets {
            for id in ids {
                writeln!(out, "{id}")?;
            }
        }
        return Ok(());
    }

    let result_sets = semantic_search_many(
        embeddings,
        &args.ollama_url,
        &args.model,
        &queries,
        args.limit,
    )?;
    let items: Vec<_> = queries
        .into_iter()
        .zip(result_sets)
        .map(|(query, results)| BatchItem::new(query, results))
        .collect();

    if args.format.is_structured() {
        let mut value = json!({ "items": items });
        provenance::inject_into_json(&mut value, prov, show_prov);
        args.format.print(&value)?;
        return Ok(());
    }

    let format = ConceptFormat {
        line: "{score} | {id} | {pt}".into(),
        fsn_suffix: String::new(),
    }
    .with_overrides(args.template.clone(), Some(String::new()));
    for item in &items {
        if item.result.is_empty() {
            eprintln!("No embeddings found in {}", embeddings.display());
        }
        for concept in &item.result {
            println!(
                "{}",
                format.render(&ConceptFields {
                    id: &concept.id,
                    pt: &concept.preferred_term,
                    score: Some(concept.score as f64),
                    inactive: !concept.active,
                    ..Default::default()
                })
            );
        }
    }
    provenance::print_human_footer(prov, show_prov);
    Ok(())
}

/// Open the embeddings file just to read its schema-level metadata.
/// Cheap because Arrow IPC stores the schema in the footer; we don't have
/// to scan any record batches.
pub fn read_arrow_provenance(path: &Path) -> Result<Option<Provenance>> {
    Ok(provenance::from_arrow_metadata(&read_arrow_metadata(path)?))
}

/// Read schema-level metadata without scanning embedding record batches.
pub(crate) fn read_arrow_metadata(path: &Path) -> Result<HashMap<String, String>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = FileReader::try_new(file, None).context("reading Arrow IPC file")?;
    Ok(reader.schema().metadata().clone())
}

// ---------------------------------------------------------------------------
// Core search logic (shared with `sct mcp`)
// ---------------------------------------------------------------------------

/// Embed `query` via Ollama and return the top-`limit` concepts by cosine
/// similarity from the Arrow IPC file at `embeddings`.
pub fn semantic_search(
    embeddings: &Path,
    ollama_url: &str,
    model: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<ScoredConcept>> {
    let mut results =
        semantic_search_many(embeddings, ollama_url, model, &[query.to_string()], limit)?;
    Ok(results.pop().unwrap_or_default())
}

/// Embed all queries in one request and scan the Arrow file once, preserving
/// query order while bounding result memory to `queries.len() * limit`.
pub fn semantic_search_many(
    embeddings: &Path,
    ollama_url: &str,
    model: &str,
    queries: &[String],
    limit: usize,
) -> Result<Vec<Vec<ScoredConcept>>> {
    semantic_search_many_inner(embeddings, ollama_url, model, queries, limit, true)
}

fn semantic_search_ids(
    embeddings: &Path,
    ollama_url: &str,
    model: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<String>> {
    let mut results =
        semantic_search_ids_many(embeddings, ollama_url, model, &[query.to_string()], limit)?;
    Ok(results.pop().unwrap_or_default())
}

fn semantic_search_ids_many(
    embeddings: &Path,
    ollama_url: &str,
    model: &str,
    queries: &[String],
    limit: usize,
) -> Result<Vec<Vec<String>>> {
    Ok(
        semantic_search_many_inner(embeddings, ollama_url, model, queries, limit, false)?
            .into_iter()
            .map(|results| results.into_iter().map(|result| result.id).collect())
            .collect(),
    )
}

fn semantic_search_many_inner(
    embeddings: &Path,
    ollama_url: &str,
    model: &str,
    queries: &[String],
    limit: usize,
    include_terms: bool,
) -> Result<Vec<Vec<ScoredConcept>>> {
    validate_limit(limit)?;
    validate_query_count(queries.len())?;
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    let query_vecs = embed_queries_many(ollama_url, model, queries)?;
    Ok(
        search_embedding_vectors(embeddings, model, &query_vecs, limit, include_terms, &[])?
            .results,
    )
}

pub(crate) fn embed_queries_many(
    ollama_url: &str,
    model: &str,
    queries: &[String],
) -> Result<Vec<Vec<f32>>> {
    validate_query_count(queries.len())?;
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    let profile = embedding_profile::resolve(model)?;
    embed_queries(ollama_url, model, profile, queries)
}

pub(crate) fn search_embedding_vectors(
    embeddings: &Path,
    model: &str,
    query_vecs: &[Vec<f32>],
    limit: usize,
    include_terms: bool,
    required_ids: &[String],
) -> Result<SemanticSearchBatch> {
    let profile = embedding_profile::resolve(model)?;
    validate_limit(limit)?;
    validate_query_count(query_vecs.len())?;
    anyhow::ensure!(
        !query_vecs.is_empty(),
        "semantic search needs at least one query vector"
    );
    let file = std::fs::File::open(embeddings)
        .with_context(|| format!("opening {}", embeddings.display()))?;
    let reader = FileReader::try_new(file, None).context("reading Arrow IPC file")?;

    // Refuse to search with a model other than the one that built the file.
    // The dimension check below cannot catch a same-dimension model swap, and
    // cross-model cosine scores are silently garbage. Files written before
    // this metadata existed get a stderr note instead (we cannot verify them).
    let schema = reader.schema();
    let stored_model = schema.metadata().get("sct.embedding_model").cloned();
    check_model_compat(stored_model.as_deref(), model, embeddings)?;
    check_profile_compat(
        schema
            .metadata()
            .get(embedding_profile::METADATA_KEY)
            .map(String::as_str),
        profile,
        embeddings,
    )?;
    check_text_scheme(
        schema
            .metadata()
            .get("sct.embed_text_scheme")
            .map(String::as_str),
        embeddings,
    )?;

    let query_norms: Vec<f32> = query_vecs.iter().map(|vector| l2_norm(vector)).collect();
    let mut results: Vec<BinaryHeap<RankedConcept>> =
        (0..query_vecs.len()).map(|_| BinaryHeap::new()).collect();
    let mut required: HashSet<&str> = required_ids.iter().map(String::as_str).collect();
    let mut vector_count = 0;
    let mut dimensions = None;

    for batch in reader {
        let batch = batch.context("reading Arrow batch")?;
        vector_count += batch.num_rows();

        let ids = batch
            .column_by_name("id")
            .context("missing 'id' column")?
            .as_any()
            .downcast_ref::<StringArray>()
            .context("'id' column is not StringArray")?;

        let terms = if include_terms {
            Some(
                batch
                    .column_by_name("preferred_term")
                    .context("missing 'preferred_term' column")?
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .context("'preferred_term' column is not StringArray")?,
            )
        } else {
            None
        };

        let active_col = active_column(&batch)?;

        let embeddings_col = batch
            .column_by_name("embedding")
            .context("missing 'embedding' column")?;

        let list = embeddings_col
            .as_fixed_size_list_opt()
            .context("'embedding' column is not FixedSizeList")?;

        // Read the stored dimension from the Arrow schema, not from the query
        // vector. A mismatch means the embeddings file was built with a
        // different model and scores will be garbage.
        let stored_dim = list.value_length() as usize;
        match dimensions {
            Some(previous) => anyhow::ensure!(
                previous == stored_dim,
                "embeddings file contains inconsistent vector dimensions"
            ),
            None => dimensions = Some(stored_dim),
        }
        for query_vec in query_vecs {
            anyhow::ensure!(
                query_vec.len() == stored_dim,
                "query embedding dimension ({}) does not match embeddings file dimension ({}) - \
                 the file was built with a different model. Re-run `sct embed` with --model {}",
                query_vec.len(),
                stored_dim,
                model,
            );
        }

        let flat = list
            .values()
            .as_primitive_opt::<Float32Type>()
            .context("embedding values are not Float32")?;

        let flat_slice = flat.values();

        for i in 0..batch.num_rows() {
            let start = i * stored_dim;
            let end = start + stored_dim;
            if end > flat_slice.len() {
                break;
            }
            let stored = &flat_slice[start..end];
            required.remove(ids.value(i));
            let stored_norm = l2_norm(stored);
            let active = active_col.is_none_or(|col| col.value(i));
            for ((query_vec, query_norm), top) in
                query_vecs.iter().zip(&query_norms).zip(results.iter_mut())
            {
                let score = cosine_similarity(stored, query_vec, stored_norm, *query_norm);
                push_ranked(
                    top,
                    score,
                    ids.value(i),
                    terms.map(|terms| terms.value(i)),
                    active,
                    limit,
                );
            }
        }
    }

    let results = results
        .into_iter()
        .map(|top| {
            let mut top: Vec<_> = top.into_iter().map(|ranked| ranked.0).collect();
            top.sort_by(rank_cmp);
            top
        })
        .collect();
    let mut missing_required_ids: Vec<String> = required.into_iter().map(str::to_string).collect();
    missing_required_ids.sort_unstable();
    Ok(SemanticSearchBatch {
        results,
        vector_count,
        dimensions: dimensions.unwrap_or_default(),
        missing_required_ids,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn embed_query(base_url: &str, model: &str, query: &str) -> Result<Vec<f32>> {
    let profile = embedding_profile::resolve(model)?;
    let mut embeddings = embed_queries(base_url, model, profile, &[query.to_string()])?;
    Ok(embeddings.pop().unwrap_or_default())
}

fn embed_queries(
    base_url: &str,
    model: &str,
    profile: embedding_profile::EmbeddingProfile,
    queries: &[String],
) -> Result<Vec<Vec<f32>>> {
    let url = format!("{}/api/embed", base_url.trim_end_matches('/'));
    let formatted: Vec<String> = queries
        .iter()
        .map(|query| profile.format_query(query))
        .collect();
    let body = EmbedRequest {
        model,
        input: &formatted,
    };
    let resp: EmbedResponse = ureq::post(&url)
        .header("Content-Type", "application/json")
        .send_json(&body)
        .map_err(|e| {
            anyhow::anyhow!(
                "Could not reach Ollama at {base_url}: {e}\n\
                 Ensure Ollama is running: ollama serve\n\
                 Pull the model if needed: ollama pull {model}"
            )
        })?
        .into_body()
        .read_json()
        .context("parsing Ollama response")?;

    anyhow::ensure!(
        resp.embeddings.len() == queries.len(),
        "Ollama returned {} embeddings for {} queries",
        resp.embeddings.len(),
        queries.len()
    );
    let dimension = resp.embeddings.first().map(Vec::len).unwrap_or_default();
    anyhow::ensure!(
        dimension == profile.expected_dimensions,
        "model {model} returned {dimension} dimensions, but profile {} expects {}",
        profile.id,
        profile.expected_dimensions,
    );
    for embedding in &resp.embeddings {
        anyhow::ensure!(
            embedding.len() == dimension,
            "Ollama returned embeddings with inconsistent dimensions"
        );
        anyhow::ensure!(
            embedding.iter().all(|value| value.is_finite()),
            "Ollama returned a non-finite embedding value"
        );
    }
    Ok(resp.embeddings)
}

/// Resolve the `active` column of `batch`, or `None` if it is absent - an
/// embeddings file written before this column existed. `None` is not an
/// error: every row then defaults to active, the prior behaviour, matching
/// how the FST index treats a missing `inactive_ids` section.
fn active_column(batch: &RecordBatch) -> Result<Option<&arrow::array::BooleanArray>> {
    batch
        .column_by_name("active")
        .map(|col| {
            col.as_any()
                .downcast_ref::<arrow::array::BooleanArray>()
                .context("'active' column is not BooleanArray")
        })
        .transpose()
}

fn rank_cmp(a: &ScoredConcept, b: &ScoredConcept) -> std::cmp::Ordering {
    rank_values_cmp(a.score, &a.id, b.score, &b.id)
}

fn rank_values_cmp(score_a: f32, id_a: &str, score_b: f32, id_b: &str) -> std::cmp::Ordering {
    score_b.total_cmp(&score_a).then_with(|| id_a.cmp(id_b))
}

fn validate_limit(limit: usize) -> Result<()> {
    anyhow::ensure!(
        limit <= MAX_RESULTS,
        "--limit cannot exceed {MAX_RESULTS} results per query"
    );
    Ok(())
}

fn validate_query_count(count: usize) -> Result<()> {
    anyhow::ensure!(
        count <= MAX_BATCH_QUERIES,
        "query batch cannot exceed {MAX_BATCH_QUERIES} entries"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn push_ranked(
    results: &mut BinaryHeap<RankedConcept>,
    score: f32,
    id: &str,
    preferred_term: Option<&str>,
    active: bool,
    limit: usize,
) {
    if limit == 0 {
        return;
    }
    if results.len() >= limit {
        let worst = results.peek().expect("non-empty top-k result set");
        if !rank_values_cmp(score, id, worst.0.score, &worst.0.id).is_lt() {
            return;
        }
        results.pop();
    }
    results.push(RankedConcept(ScoredConcept {
        score,
        id: id.to_string(),
        preferred_term: preferred_term.map_or_else(String::new, str::to_string),
        active,
    }));
}

/// Compare the model recorded in the embeddings file against the requested
/// query model. Mismatch is a hard error; an absent record (file written by an
/// sct predating the metadata) gets a stderr warning because it cannot be
/// verified. The supported Nomic bare, `:latest`, and pinned `:v1.5` names
/// resolve to the same versioned embedding profile.
fn check_model_compat(stored: Option<&str>, requested: &str, path: &Path) -> Result<()> {
    match stored {
        Some(s) if embedding_profile::compatible_models(s, requested) => Ok(()),
        Some(s) => anyhow::bail!(
            "embeddings file {} was built with model '{}', but this search uses '{}'. \
             Cross-model similarity scores are meaningless. Re-run with --model {} \
             or rebuild the file: sct embed --model {}",
            path.display(),
            s,
            requested,
            s,
            requested,
        ),
        None => {
            eprintln!(
                "note: {} does not record which embedding model built it (written by an \
                 older sct), so it cannot be verified against --model {}. If results look \
                 poor, rebuild it with a current sct: `sct embed`.",
                path.display(),
                requested,
            );
            Ok(())
        }
    }
}

fn check_profile_compat(
    stored: Option<&str>,
    requested: embedding_profile::EmbeddingProfile,
    path: &Path,
) -> Result<()> {
    match stored {
        Some(stored) if stored == requested.id => Ok(()),
        Some(stored) => anyhow::bail!(
            "embeddings file {} uses embedding profile {stored:?}, but this search uses {:?}. \
             Rebuild the file or query it with the matching supported model",
            path.display(),
            requested.id,
        ),
        None if requested.accepts_legacy_text_scheme() => Ok(()),
        None => anyhow::bail!(
            "embeddings file {} predates embedding-profile metadata and cannot be safely queried \
             with profile {:?}. Rebuild it with the current sct: sct embed",
            path.display(),
            requested.id,
        ),
    }
}

fn check_text_scheme(stored: Option<&str>, path: &Path) -> Result<()> {
    let expected = crate::commands::embed::EMBED_TEXT_SCHEME;
    match stored {
        Some(scheme) if scheme == expected => Ok(()),
        Some(scheme) => anyhow::bail!(
            "embeddings file {} uses text scheme {}, but this sct version expects {}. \
             Rebuild it with the current version: sct embed",
            path.display(),
            scheme,
            expected,
        ),
        None => {
            eprintln!(
                "note: {} does not record its embedding text scheme (written by an older sct), \
                 so compatibility cannot be verified. If results look poor, rebuild it with a \
                 current sct: `sct embed`.",
                path.display(),
            );
            Ok(())
        }
    }
}

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn cosine_similarity(a: &[f32], b: &[f32], a_norm: f32, b_norm: f32) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let denom = a_norm * b_norm;
    if denom < 1e-9 {
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_vectors() {
        let v = vec![1.0f32, 2.0, 3.0];
        let norm = l2_norm(&v);
        let score = cosine_similarity(&v, &v, norm, norm);
        assert!((score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_orthogonal_vectors() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let a_norm = l2_norm(&a);
        let b_norm = l2_norm(&b);
        let score = cosine_similarity(&a, &b, a_norm, b_norm);
        assert!(score.abs() < 1e-5);
    }

    #[test]
    fn l2_norm_basic() {
        let v = vec![3.0f32, 4.0];
        assert!((l2_norm(&v) - 5.0).abs() < 1e-5);
    }

    fn batch_without_active_column() -> RecordBatch {
        let schema = std::sync::Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![std::sync::Arc::new(StringArray::from(vec!["22298006"]))],
        )
        .unwrap()
    }

    fn batch_with_active_column(active: bool) -> RecordBatch {
        let schema = std::sync::Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Utf8, false),
            arrow::datatypes::Field::new("active", arrow::datatypes::DataType::Boolean, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                std::sync::Arc::new(StringArray::from(vec!["9468002"])),
                std::sync::Arc::new(arrow::array::BooleanArray::from(vec![active])),
            ],
        )
        .unwrap()
    }

    /// An embeddings file written before the `active` column existed must
    /// still open and default every row to active, not error - the same
    /// backward-compatibility contract the FST index gives a missing section.
    #[test]
    fn active_column_is_none_for_an_embeddings_file_without_it() {
        let batch = batch_without_active_column();
        assert_eq!(active_column(&batch).unwrap(), None);
    }

    #[test]
    fn active_column_reads_true_and_false() {
        assert!(active_column(&batch_with_active_column(true))
            .unwrap()
            .unwrap()
            .value(0));
        assert!(!active_column(&batch_with_active_column(false))
            .unwrap()
            .unwrap()
            .value(0));
    }

    #[test]
    fn bounded_top_k_uses_score_then_sctid_order() {
        let mut results = BinaryHeap::new();
        push_ranked(&mut results, 0.5, "3", Some("three"), true, 2);
        push_ranked(&mut results, 0.9, "2", Some("two"), true, 2);
        push_ranked(&mut results, 0.9, "1", Some("one"), true, 2);
        push_ranked(&mut results, 0.1, "4", Some("four"), true, 2);
        let mut results: Vec<_> = results.into_iter().map(|ranked| ranked.0).collect();
        results.sort_by(rank_cmp);
        assert_eq!(
            results
                .iter()
                .map(|result| result.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
    }

    #[test]
    fn result_limit_is_bounded() {
        assert!(validate_limit(MAX_RESULTS).is_ok());
        assert!(validate_limit(MAX_RESULTS + 1).is_err());
    }

    #[test]
    fn query_batch_is_bounded() {
        assert!(validate_query_count(MAX_BATCH_QUERIES).is_ok());
        assert!(validate_query_count(MAX_BATCH_QUERIES + 1).is_err());
    }

    #[test]
    fn model_compat_exact_match_ok() {
        let p = Path::new("x.arrow");
        assert!(check_model_compat(Some("nomic-embed-text"), "nomic-embed-text", p).is_ok());
    }

    #[test]
    fn model_compat_latest_alias_ok() {
        let p = Path::new("x.arrow");
        assert!(check_model_compat(Some("nomic-embed-text:latest"), "nomic-embed-text", p).is_ok());
        assert!(check_model_compat(Some("nomic-embed-text"), "nomic-embed-text:latest", p).is_ok());
        assert!(check_model_compat(Some("nomic-embed-text"), "nomic-embed-text:v1.5", p).is_ok());
    }

    #[test]
    fn model_compat_mismatch_errors() {
        let p = Path::new("x.arrow");
        let err = check_model_compat(Some("nomic-embed-text"), "mxbai-embed-large", p).unwrap_err();
        assert!(err.to_string().contains("built with model"));
    }

    #[test]
    fn model_compat_absent_metadata_warns_but_allows() {
        let p = Path::new("x.arrow");
        assert!(check_model_compat(None, "nomic-embed-text", p).is_ok());
    }

    #[test]
    fn text_scheme_compatibility_is_enforced() {
        let p = Path::new("x.arrow");
        assert!(check_text_scheme(Some(crate::commands::embed::EMBED_TEXT_SCHEME), p).is_ok());
        assert!(check_text_scheme(Some("999"), p).is_err());
        assert!(check_text_scheme(None, p).is_ok());
    }

    #[test]
    fn embedding_profile_compatibility_is_enforced() {
        let p = Path::new("x.arrow");
        let current = embedding_profile::resolve("nomic-embed-text").unwrap();
        assert!(check_profile_compat(Some(current.id), current, p).is_ok());
        assert!(check_profile_compat(Some("future-profile/sct-2"), current, p).is_err());
        assert!(check_profile_compat(None, current, p).is_ok());

        let qwen = embedding_profile::resolve("qwen3-embedding:0.6b").unwrap();
        assert!(check_profile_compat(None, qwen, p).is_err());
    }
}
