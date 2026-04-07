# `sct embed` - Embedding Model Specification

## Overview

`sct embed` generates vector embeddings for all active SNOMED CT concepts and writes them to an Arrow IPC file. The choice of embedding model significantly affects the quality of semantic search results, particularly for clinical coding use cases. This spec defines the supported models, their tradeoffs, and a benchmarking framework for evaluating them against each other.

The user retains full choice of model. No model is hardcoded as the default beyond what is practical for a first run.

---

## The model selection problem

Embedding models exist on a spectrum from general-purpose to clinically specialised. For SNOMED semantic search the relevant axis is: how well does the model map clinical language (narrative, lay, abbreviated) to the same vector neighbourhood as SNOMED preferred terms?

A general-purpose model trained on web text knows that "heart attack" and "myocardial infarction" are related but may not know that "crushing central chest pain with radiation to the jaw and diaphoresis" is more specifically an acute MI presentation than stable angina. A clinical model trained on medical literature and clinical notes is likely to get this right.

The tradeoff is operational: clinical models typically require ONNX runtime or a Python inference stack, whereas general-purpose models are available through Ollama with a single command.

---

## Supported models

### General-purpose (Ollama-backed)

These run via Ollama and require no additional model files beyond `ollama pull <model>`.

#### `nomic-embed-text`
- **Dimensions:** 768
- **Context window:** 8192 tokens
- **Strengths:** Strong general semantic similarity, good multilingual support, fast
- **Weaknesses:** Not trained on clinical text - lay-to-clinical mapping is approximate
- **Best for:** General SNOMED exploration, non-clinical use cases, getting started quickly
- **Ollama command:** `ollama pull nomic-embed-text`

#### `mxbai-embed-large`
- **Dimensions:** 1024
- **Context window:** 512 tokens
- **Strengths:** State-of-the-art general embedding quality as of early 2024
- **Weaknesses:** Shorter context window, not clinically trained
- **Best for:** Higher quality general search where clinical specificity is not critical
- **Ollama command:** `ollama pull mxbai-embed-large`

#### `all-minilm`
- **Dimensions:** 384
- **Context window:** 256 tokens
- **Strengths:** Very fast, small memory footprint
- **Weaknesses:** Lower quality, very short context
- **Best for:** Development, testing, resource-constrained environments
- **Ollama command:** `ollama pull all-minilm`

---

### Clinically specialised (ONNX-backed)

These require downloading ONNX model files and running via the `ort` Rust crate. No Python or Ollama dependency at inference time.

#### `sapbert` (recommended for clinical coding)
- **Full name:** SapBERT (Self-Alignment Pretraining for Biomedical Entity Representation)
- **Dimensions:** 768
- **Context window:** 512 tokens
- **Training data:** UMLS synonyms - explicitly trained so that different surface forms of the same biomedical concept embed close together
- **Strengths:** Best-in-class for biomedical entity linking; "heart attack", "MI", "myocardial infarction", "cardiac infarction" all land in the same neighbourhood; handles abbreviations well
- **Weaknesses:** Trained on short concept names and synonyms, not long clinical narratives - very long consultation excerpts may need chunking
- **Best for:** Clinical coding from any clinical language, automated SNOMED suggestion
- **Source:** `cambridgeltl/SapBERT-from-PubMedBERT-fulltext` on HuggingFace
- **ONNX file size:** ~438MB

#### `medcpt`  
- **Full name:** MedCPT (Medical text Contrastive Pre-Training)
- **Dimensions:** 768
- **Context window:** 512 tokens
- **Training data:** PubMed articles and clinical queries - trained as a query-document retrieval model
- **Strengths:** Excellent for retrieval tasks where the query is a clinical question and the document is a concept description; handles longer clinical text better than SAPBERT
- **Weaknesses:** Designed for article retrieval, may overfit to PubMed-style language
- **Best for:** Longer consultation text, research use cases
- **Source:** `ncats/MedCPT-Query-Encoder` on HuggingFace
- **ONNX file size:** ~438MB

#### `biobert`
- **Full name:** BioBERT (Biomedical BERT)
- **Dimensions:** 768
- **Context window:** 512 tokens
- **Training data:** PubMed abstracts and PMC full-text articles
- **Strengths:** Strong general biomedical language understanding; widely used and well-studied
- **Weaknesses:** Not specifically trained for entity linking - general biomedical similarity rather than concept-to-concept matching
- **Best for:** Baseline comparison; useful when SAPBERT is not available
- **Source:** `dmis-lab/biobert-base-cased-v1.2` on HuggingFace
- **ONNX file size:** ~438MB

#### `clinical-bert` (ClinicalBERT)
- **Full name:** ClinicalBERT
- **Dimensions:** 768
- **Context window:** 512 tokens
- **Training data:** MIMIC-III clinical notes (ICU discharge summaries, nursing notes)
- **Strengths:** Trained on real clinical documentation - handles clinical note language, abbreviations common in clinical practice (SOB, STEMI, PMH, etc.)
- **Weaknesses:** MIMIC-III is US ICU data - may not generalise well to UK primary care or outpatient language; licence restrictions on MIMIC data
- **Best for:** ICU/secondary care coding; US clinical settings
- **Source:** `emilyalsentzer/Bio_ClinicalBERT` on HuggingFace
- **ONNX file size:** ~438MB

---

## CLI interface

```bash
# Ollama-backed (model must be pulled first)
sct embed --input snomed.ndjson --output snomed.arrow --model nomic-embed-text
sct embed --input snomed.ndjson --output snomed.arrow --model mxbai-embed-large

# ONNX-backed (downloads model if not present)
sct embed --input snomed.ndjson --output snomed.arrow --model sapbert
sct embed --input snomed.ndjson --output snomed.arrow --model medcpt
sct embed --input snomed.ndjson --output snomed.arrow --model biobert

# With explicit ONNX model file (advanced, skip download)
sct embed --input snomed.ndjson --output snomed.arrow \
  --model onnx --onnx-file ~/models/sapbert.onnx

# Benchmark mode - embeds a sample and reports quality metrics
sct embed --benchmark --models sapbert,nomic-embed-text,medcpt \
  --input snomed.ndjson --output-dir ./benchmark-results/
```

---

## Model download management

ONNX models are downloaded from HuggingFace on first use and cached in `~/.cache/sct/models/`. Subsequent runs use the cached file.

```bash
# Download without embedding (pre-cache for offline use)
sct embed --download-model sapbert

# List cached models
sct embed --list-models

# Show cache location and sizes
sct embed --cache-info
```

The cache directory can be overridden with `SCT_MODEL_CACHE` environment variable.

---

## What gets embedded

Each concept is embedded as a single string constructed from its fields:

```
{preferred_term}. {fsn}. {synonyms joined by ", "}.
```

Example for Myocardial infarction:
```
Myocardial infarction. Myocardial infarction (disorder). Heart attack, Cardiac infarction, Infarction of heart, MI - myocardial infarction.
```

This concatenation gives the model the full vocabulary surface of the concept. Alternatives considered:

- **Preferred term only** - fast, small, but misses synonyms; "heart attack" would not find MI if the model hasn't learned the synonymy
- **FSN only** - includes semantic tag (disorder) which adds noise
- **Hierarchy path appended** - adds "Clinical finding > Cardiovascular finding > Myocardial infarction" - may help with context but makes strings long and pushes some models past their context window
- **All fields concatenated** - current recommendation; best recall, manageable length for most concepts

The embedding text format is stored in the Arrow file metadata so `sct mcp` can reproduce the same format at query time.

---

## Arrow file format

The output is an Arrow IPC file with the following schema:

```
schema:
  - id: utf8
  - embedding: fixed_size_list<float32>[768]   -- dimension varies by model

metadata:
  model_name: "sapbert"
  model_dimension: "768"
  embedding_text_format: "{preferred_term}. {fsn}. {synonyms}."
  snomed_release: "20260311"
  concept_count: "412257"
  sct_version: "0.3.0"
  created: "2026-03-28T18:00:00Z"
```

The metadata block is critical - `sct mcp` reads it at startup to validate that the embedding model matches the configured runtime model before serving queries.

---

## Benchmarking framework

Because the right model choice depends on use case, `sct embed --benchmark` evaluates models against a standard test set of clinical-to-SNOMED mappings.

### Test set structure

A YAML file of clinical queries with known correct SNOMED codes:

```yaml
test_cases:
  - query: "crushing chest pain radiating to left arm with sweating"
    correct_id: "22298006"      # Myocardial infarction
    acceptable_ids:             # also acceptable (related concepts)
      - "57054005"              # Acute myocardial infarction
    hierarchy: clinical_finding

  - query: "SOB on exertion, orthopnoea, bilateral ankle swelling"
    correct_id: "84114007"      # Heart failure
    acceptable_ids:
      - "10335000"              # Chronic heart failure
    hierarchy: clinical_finding

  - query: "patient can't catch their breath going up stairs, legs puffy"
    correct_id: "84114007"      # Heart failure (lay language test)
    hierarchy: clinical_finding

  - query: "appendix out"
    correct_id: "80146002"      # Appendectomy
    hierarchy: procedure

  - query: "high BP"
    correct_id: "38341003"      # Hypertension
    hierarchy: clinical_finding

  - query: "STEMI"
    correct_id: "401303003"     # Acute ST segment elevation MI
    hierarchy: clinical_finding
```

A starter test set of 50-100 cases covering:
- Standard clinical terminology
- Lay patient language
- Common abbreviations (SOB, HTN, STEMI, T2DM, AF)
- UK-specific terms (surgical sieve, clerking language)
- Drug names to dm+d codes (if dm+d index present)

### Benchmark metrics

For each model, report:

| Metric | Description |
|---|---|
| `top_1_accuracy` | Correct code is rank 1 result |
| `top_5_accuracy` | Correct code is in top 5 results |
| `top_10_accuracy` | Correct code is in top 10 results |
| `mean_reciprocal_rank` | Average of 1/rank for correct code |
| `mean_similarity_correct` | Average similarity score for correct code |
| `mean_similarity_rank1` | Average similarity of top result (regardless of correctness) |
| `embed_time_ms` | Time to embed all test queries |
| `search_time_ms` | Time to search for all test queries |

### Benchmark output

```
sct embed benchmark results
============================
Test set: 50 cases | SNOMED: UK Clinical Edition 20260311

Model               top-1   top-5   top-10  MRR    embed_ms  search_ms
────────────────────────────────────────────────────────────────────────
sapbert             0.76    0.88    0.94    0.81   12ms      45ms
medcpt              0.71    0.85    0.92    0.77   14ms      45ms
biobert             0.64    0.80    0.88    0.71   13ms      45ms
nomic-embed-text    0.58    0.74    0.83    0.65   8ms       45ms
mxbai-embed-large   0.61    0.76    0.85    0.68   22ms      62ms
all-minilm          0.44    0.62    0.72    0.52   4ms       28ms

Recommendation: sapbert for clinical coding, nomic-embed-text for general use
```

The benchmark output is also written as JSON to `./benchmark-results/` for tracking over time as models and test sets are updated.

---

## Build performance considerations

Embedding 412,000 concepts is not instant. Expected build times:

| Model | Backend | Estimated time | Index size |
|---|---|---|---|
| all-minilm | Ollama | ~15 min | ~600MB |
| nomic-embed-text | Ollama | ~25 min | ~1.2GB |
| mxbai-embed-large | Ollama | ~35 min | ~1.6GB |
| sapbert | ONNX | ~20 min | ~1.2GB |
| medcpt | ONNX | ~20 min | ~1.2GB |

These are rough estimates on a developer workstation - GPU acceleration (if Ollama is configured to use it) reduces Ollama times dramatically.

Progress reporting should be prominent:

```
sct embed - building SNOMED vector index
Model: sapbert (ONNX)
Concepts: 412,257
  [=============================>    ] 387,000/412,257 (93%) | 45 concepts/sec | ETA 4m32s
Output: snomed-sapbert.arrow
```

### Batching

Embed concepts in batches rather than one at a time. Optimal batch size varies by model and hardware but 32-128 is typical for BERT-class models. The ONNX runtime handles batching efficiently; Ollama's embedding endpoint accepts single strings only so batching requires parallel HTTP requests with a concurrency limit.

---

## Implementation notes

### ONNX runtime in Rust

Use the `ort` crate (ONNX Runtime bindings for Rust). The model is loaded once at startup and kept in memory for the duration of the embed run.

```toml
[dependencies]
ort = { version = "2", features = ["load-dynamic"] }
```

BERT-class models require tokenisation before inference. Use the `tokenizers` crate (HuggingFace tokenizers, Rust port) with the model-specific vocabulary file downloaded alongside the ONNX file.

Mean pooling over the final hidden states is the standard approach for sentence-level embeddings from BERT models. SAPBERT specifically uses mean pooling of the last hidden layer.

### Ollama batching workaround

Since Ollama's `/api/embeddings` endpoint is single-string, use `tokio` with a bounded semaphore to issue parallel requests:

```rust
let semaphore = Arc::new(Semaphore::new(8)); // max 8 concurrent requests
```

Tune concurrency based on Ollama throughput - too high and Ollama queues internally, too low and GPU sits idle.

---

## Future model candidates

Worth evaluating as they mature:

- **BGE-M3** (BAAI) - multilingual, strong biomedical performance, available via Ollama
- **E5-mistral-7b** - large but very high quality general embeddings
- **OpenAI text-embedding-3-large** - API-based, not local, but useful as a quality ceiling benchmark
- **Domain-specific fine-tuned models** - fine-tuning SAPBERT on UK primary care consultation language would likely improve top-1 accuracy significantly for the target use case; a future research contribution

---

## Relationship to `sct mcp`

The model name recorded in the Arrow file metadata is the single source of truth for which model `sct mcp` must use at query time. The two must match. If they do not, `sct mcp` logs an error at startup and disables `snomed_semantic_search`.

This means users can maintain multiple index files for different models:

```
snomed-sapbert.arrow        ← for clinical coding
snomed-nomic.arrow          ← for general exploration
```

And switch between them by pointing `sct mcp --embeddings` at the appropriate file.
