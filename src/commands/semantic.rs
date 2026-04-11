//! `sct semantic` — Semantic similarity search over a SNOMED CT Arrow IPC embeddings file.
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
use clap::Parser;
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::format::{ConceptFields, ConceptFormat};

#[derive(Parser, Debug)]
pub struct Args {
    /// Natural-language search query.
    pub query: String,

    /// Arrow IPC embeddings file produced by `sct embed`.
    #[arg(long, short, default_value = "snomed-embeddings.arrow")]
    pub embeddings: PathBuf,

    /// Ollama embedding model — must match the model used by `sct embed`.
    #[arg(long, default_value = "nomic-embed-text")]
    pub model: String,

    /// Ollama API base URL.
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// Maximum number of results to return.
    #[arg(long, short, default_value = "10")]
    pub limit: usize,

    /// Override the per-result line template.
    /// Default: `{score} | {id} | {pt}`. See `docs/commands/refset.md`.
    #[arg(long)]
    pub format: Option<String>,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct ScoredConcept {
    pub score: f32,
    pub id: String,
    pub preferred_term: String,
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
    let results = semantic_search(
        &args.embeddings,
        &args.ollama_url,
        &args.model,
        &args.query,
        args.limit,
    )?;

    if results.is_empty() {
        println!("No embeddings found in {}", args.embeddings.display());
        return Ok(());
    }

    let format = ConceptFormat {
        line: "{score} | {id} | {pt}".into(),
        fsn_suffix: String::new(),
    }
    .with_overrides(args.format, Some(String::new()));

    for ScoredConcept {
        score,
        id,
        preferred_term,
    } in &results
    {
        println!(
            "{}",
            format.render(&ConceptFields {
                id,
                pt: preferred_term,
                score: Some(*score as f64),
                ..Default::default()
            })
        );
    }

    Ok(())
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
    let query_vec = embed_query(ollama_url, model, query)?;
    let q_norm = l2_norm(&query_vec);

    let file = std::fs::File::open(embeddings)
        .with_context(|| format!("opening {}", embeddings.display()))?;
    let reader = FileReader::try_new(file, None).context("reading Arrow IPC file")?;

    let mut results: Vec<ScoredConcept> = Vec::new();

    for batch in reader {
        let batch = batch.context("reading Arrow batch")?;

        let ids = batch
            .column_by_name("id")
            .context("missing 'id' column")?
            .as_any()
            .downcast_ref::<StringArray>()
            .context("'id' column is not StringArray")?;

        let terms = batch
            .column_by_name("preferred_term")
            .context("missing 'preferred_term' column")?
            .as_any()
            .downcast_ref::<StringArray>()
            .context("'preferred_term' column is not StringArray")?;

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
        anyhow::ensure!(
            query_vec.len() == stored_dim,
            "query embedding dimension ({}) does not match embeddings file dimension ({}) — \
             the file was built with a different model. Re-run `sct embed` with --model {}",
            query_vec.len(),
            stored_dim,
            model,
        );

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
            let score = cosine_similarity(&flat_slice[start..end], &query_vec, q_norm);
            results.push(ScoredConcept {
                score,
                id: ids.value(i).to_string(),
                preferred_term: terms.value(i).to_string(),
            });
        }
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    Ok(results)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn embed_query(base_url: &str, model: &str, query: &str) -> Result<Vec<f32>> {
    let url = format!("{}/api/embed", base_url.trim_end_matches('/'));
    // The `search_query:` prefix pairs with the `search_document:` prefix used
    // by `sct embed`, activating nomic-embed-text's asymmetric retrieval mode.
    let prefixed = format!("search_query: {query}");
    let body = EmbedRequest {
        model,
        input: &[prefixed],
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

    resp.embeddings
        .into_iter()
        .next()
        .filter(|v: &Vec<f32>| !v.is_empty())
        .context("Ollama returned an empty embedding for the query")
}

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn cosine_similarity(a: &[f32], b: &[f32], b_norm: f32) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let a_norm = l2_norm(a);
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
        let score = cosine_similarity(&v, &v, norm);
        assert!((score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_orthogonal_vectors() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let b_norm = l2_norm(&b);
        let score = cosine_similarity(&a, &b, b_norm);
        assert!(score.abs() < 1e-5);
    }

    #[test]
    fn l2_norm_basic() {
        let v = vec![3.0f32, 4.0];
        assert!((l2_norm(&v) - 5.0).abs() < 1e-5);
    }
}
