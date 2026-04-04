//! `sct embed` — Generate vector embeddings from a SNOMED CT NDJSON artefact
//! and write an Apache Arrow IPC file for vector search.
//!
//! Embeddings are produced by an Ollama instance running locally.
//! Default model: `nomic-embed-text` (768 dimensions, excellent for medical text).
//!
//! Output format: Arrow IPC (`.arrow`) with columns:
//!   id            — SCTID (UTF-8)
//!   preferred_term — preferred term (UTF-8)
//!   hierarchy     — top-level hierarchy name (UTF-8)
//!   embedding     — FixedSizeList<Float32>(dim)
//!
//! The Arrow IPC file can be queried directly in DuckDB:
//!   SELECT id, preferred_term,
//!          array_cosine_similarity(embedding, $query_vec::FLOAT[768]) AS score
//!   FROM read_ipc_auto('snomed-embeddings.arrow')
//!   ORDER BY score DESC LIMIT 10;
//!
//! It can also be imported into LanceDB or any Arrow-compatible vector store.

use anyhow::{Context, Result};
use arrow::array::{FixedSizeListArray, Float32Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::FileWriter;
use arrow::record_batch::RecordBatch;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::schema::ConceptRecord;

#[derive(Parser, Debug)]
pub struct Args {
    /// Input NDJSON file produced by `sct ndjson`. Use `-` for stdin.
    #[arg(long, short)]
    pub input: PathBuf,

    /// Ollama embedding model name.
    #[arg(long, default_value = "nomic-embed-text")]
    pub model: String,

    /// Ollama API base URL.
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// Output Arrow IPC file.
    #[arg(long, short, default_value = "snomed-embeddings.arrow")]
    pub output: PathBuf,

    /// Number of concepts to embed per Ollama API call.
    #[arg(long, default_value = "64")]
    pub batch_size: usize,
}

// ---------------------------------------------------------------------------
// Ollama API types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub fn run(args: Args) -> Result<()> {
    // Open input
    let input: Box<dyn std::io::Read> = if args.input.as_os_str() == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(
            std::fs::File::open(&args.input)
                .with_context(|| format!("opening {}", args.input.display()))?,
        )
    };

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} [{elapsed_precise}] {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(120));
    pb.set_message("Probing Ollama...");

    // Probe Ollama with a single embedding to verify it is reachable and to
    // discover the embedding dimension.
    let probe_text = "SNOMED CT concept".to_string();
    let probe = call_ollama(&args.ollama_url, &args.model, &[probe_text]).context(
        "Could not reach Ollama. Is it running?\n\
             Start it with: ollama serve\n\
             Then pull the model: ollama pull nomic-embed-text",
    )?;
    let dim = probe
        .first()
        .filter(|v| !v.is_empty())
        .map(|v| v.len())
        .context("Ollama returned an empty embedding on probe")?;

    pb.set_message(format!(
        "Ollama ready — model={}, dim={dim}. Reading concepts...",
        args.model
    ));

    // Read all concepts (we must buffer to produce the Arrow batch later)
    let reader = BufReader::new(input);
    let mut concepts: Vec<ConceptRecord> = Vec::new();
    for line in reader.lines() {
        let line = line.context("reading input")?;
        if line.trim().is_empty() {
            continue;
        }
        let record: ConceptRecord = serde_json::from_str(&line).context("parsing NDJSON record")?;
        concepts.push(record);
    }

    pb.set_message(format!(
        "{} concepts loaded. Embedding in batches of {}...",
        concepts.len(),
        args.batch_size
    ));

    // Embed in batches
    let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(concepts.len());
    let texts: Vec<String> = concepts.iter().map(embed_text).collect();

    for (chunk_idx, chunk) in texts.chunks(args.batch_size).enumerate() {
        let batch_vecs = call_ollama(&args.ollama_url, &args.model, chunk).with_context(|| {
            format!(
                "embedding batch starting at concept {}",
                chunk_idx * args.batch_size
            )
        })?;
        all_embeddings.extend(batch_vecs);

        let done = ((chunk_idx + 1) * args.batch_size).min(concepts.len());
        pb.set_message(format!("{}/{} concepts embedded...", done, concepts.len()));
    }

    pb.set_message("Writing Arrow IPC file...");

    write_arrow(&concepts, &all_embeddings, dim, &args.output)?;

    pb.finish_with_message(format!(
        "Done. {} embeddings (dim={}) → {}",
        concepts.len(),
        dim,
        args.output.display()
    ));

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the text string that will be embedded for a concept.
///
/// The `search_document:` prefix activates nomic-embed-text's asymmetric
/// retrieval mode. Queries must use the matching `search_query:` prefix
/// (see `sct semantic`). Without these prefixes the model uses a generic
/// symmetric space and similarity scores are noticeably lower.
fn embed_text(r: &ConceptRecord) -> String {
    let path = r.hierarchy_path.join(" > ");
    let body = if r.synonyms.is_empty() {
        format!("{}. {}. Hierarchy: {}.", r.preferred_term, r.fsn, path)
    } else {
        format!(
            "{}. {}. Synonyms: {}. Hierarchy: {}.",
            r.preferred_term,
            r.fsn,
            r.synonyms.join(", "),
            path
        )
    };
    format!("search_document: {body}")
}

/// POST a batch of texts to the Ollama `/api/embed` endpoint.
fn call_ollama(base_url: &str, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let url = format!("{}/api/embed", base_url.trim_end_matches('/'));
    let body = EmbedRequest {
        model,
        input: texts,
    };
    let resp: EmbedResponse = ureq::post(&url)
        .header("Content-Type", "application/json")
        .send_json(&body)
        .map_err(|e| anyhow::anyhow!("Ollama request failed: {e}"))?
        .into_body()
        .read_json()
        .context("parsing Ollama response")?;
    Ok(resp.embeddings)
}

/// Write an Arrow IPC file with columns: id, preferred_term, hierarchy, embedding.
fn write_arrow(
    concepts: &[ConceptRecord],
    embeddings: &[Vec<f32>],
    dim: usize,
    path: &Path,
) -> Result<()> {
    anyhow::ensure!(
        concepts.len() == embeddings.len(),
        "concept count ({}) != embedding count ({})",
        concepts.len(),
        embeddings.len()
    );

    let item_field = Arc::new(Field::new("item", DataType::Float32, false));
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("preferred_term", DataType::Utf8, false),
        Field::new("hierarchy", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(item_field.clone(), dim as i32),
            false,
        ),
    ]));

    let ids = StringArray::from_iter_values(concepts.iter().map(|c| c.id.as_str()));
    let terms = StringArray::from_iter_values(concepts.iter().map(|c| c.preferred_term.as_str()));
    let hierarchies = StringArray::from_iter_values(concepts.iter().map(|c| c.hierarchy.as_str()));

    // Flatten all embedding vectors into a single Float32 array
    let flat: Vec<f32> = embeddings.iter().flat_map(|v| v.iter().copied()).collect();
    let flat_array = Arc::new(Float32Array::from(flat));
    let embedding_array = FixedSizeListArray::new(item_field, dim as i32, flat_array, None);

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(ids),
            Arc::new(terms),
            Arc::new(hierarchies),
            Arc::new(embedding_array),
        ],
    )
    .context("building Arrow record batch")?;

    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut writer = FileWriter::try_new(file, &schema).context("creating Arrow IPC writer")?;
    writer.write(&batch).context("writing Arrow batch")?;
    writer.finish().context("finalising Arrow IPC file")?;

    Ok(())
}
