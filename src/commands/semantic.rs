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
use clap::Parser;
use serde::Serialize;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::format::{ConceptFields, ConceptFormat};
use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, Provenance, ProvenanceFlags};

#[derive(Parser, Debug)]
pub struct Args {
    /// Natural-language search query.
    pub query: String,

    /// Arrow IPC embeddings file produced by `sct embed`.
    /// See `docs/path-resolution.md` for the discovery order when omitted.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub embeddings: Option<PathBuf>,

    /// Ollama embedding model - must match the model used by `sct embed`.
    #[arg(long, default_value = "nomic-embed-text")]
    pub model: String,

    /// Ollama API base URL.
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// Maximum number of results to return.
    #[arg(long, short, default_value = "10")]
    pub limit: usize,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Emit only matching SCTIDs (newline-delimited) for piping.
    #[arg(long)]
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
    let embeddings = crate::paths::resolve_embeddings(args.embeddings.as_deref())?.path;
    let prov = read_arrow_provenance(&embeddings).unwrap_or(None);
    let out = args.format;
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let results = semantic_search(
        &embeddings,
        &args.ollama_url,
        &args.model,
        &args.query,
        args.limit,
    )?;

    // `--ids`: machine output for pipes - just SCTIDs on stdout.
    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for c in &results {
            writeln!(out, "{}", c.id)?;
        }
        return Ok(());
    }

    if results.is_empty() && !out.is_structured() {
        println!("No embeddings found in {}", embeddings.display());
        return Ok(());
    }

    if out.is_structured() {
        let items: Vec<Value> = results
            .iter()
            .map(|c| json!({ "score": c.score, "id": c.id, "preferred_term": c.preferred_term }))
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

    provenance::print_human_footer(prov.as_ref(), show_prov);

    Ok(())
}

/// Open the embeddings file just to read its schema-level metadata.
/// Cheap because Arrow IPC stores the schema in the footer; we don't have
/// to scan any record batches.
pub fn read_arrow_provenance(path: &Path) -> Result<Option<Provenance>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = FileReader::try_new(file, None).context("reading Arrow IPC file")?;
    let schema = reader.schema();
    Ok(provenance::from_arrow_metadata(schema.metadata()))
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
    let file = std::fs::File::open(embeddings)
        .with_context(|| format!("opening {}", embeddings.display()))?;
    let reader = FileReader::try_new(file, None).context("reading Arrow IPC file")?;

    // Refuse to search with a model other than the one that built the file.
    // The dimension check below cannot catch a same-dimension model swap, and
    // cross-model cosine scores are silently garbage. Files written before
    // this metadata existed get a stderr note instead (we cannot verify them).
    let stored_model = reader
        .schema()
        .metadata()
        .get("sct.embedding_model")
        .cloned();
    check_model_compat(stored_model.as_deref(), model, embeddings)?;

    let query_vec = embed_query(ollama_url, model, query)?;
    let q_norm = l2_norm(&query_vec);

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
            "query embedding dimension ({}) does not match embeddings file dimension ({}) - \
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

/// Compare the model recorded in the embeddings file against the requested
/// query model. Mismatch is a hard error; an absent record (file written by an
/// sct predating the metadata) gets a stderr warning because it cannot be
/// verified. `nomic-embed-text` and `nomic-embed-text:latest` are the same
/// model, so a bare name and its `:latest` alias are treated as equal.
fn check_model_compat(stored: Option<&str>, requested: &str, path: &Path) -> Result<()> {
    let canon = |m: &str| {
        m.strip_suffix(":latest")
            .map(String::from)
            .unwrap_or_else(|| m.to_string())
    };
    match stored {
        Some(s) if canon(s) == canon(requested) => Ok(()),
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
}
