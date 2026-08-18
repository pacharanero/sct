# `sct embed` - Embedding Model Specification

## Overview

`sct embed` generates vector embeddings for all active SNOMED CT concepts and writes them to an Arrow IPC file. The choice of embedding model significantly affects the quality of semantic search results, particularly for clinical coding use cases. This spec defines the intended supported models, their tradeoffs, and a benchmarking framework for evaluating them against each other.

**Implementation reality (August 2026):** `R56` introduces shared, distinct versioned profiles for `nomic-embed-text`/`nomic-embed-text:v1.5`, `nomic-embed-text-v2-moe`, `qwen3-embedding:0.6b`, and `embeddinggemma`; records the selected profile in new Arrow artefacts; validates expected dimensions; and rejects unadapted Ollama model names before an expensive build. Existing Nomic v1 scheme-2 artefacts remain compatible. The initial curated menu is ready for R15 comparison; adding future models requires another explicit profile and compatibility evidence.

---

## The model selection problem

Embedding models exist on a spectrum from general-purpose to clinically specialised. For SNOMED semantic search the relevant axis is: how well does the model map clinical language (narrative, lay, abbreviated) to the same vector neighbourhood as SNOMED preferred terms?

A general-purpose model trained on web text knows that "heart attack" and "myocardial infarction" are related but may not know that "crushing central chest pain with radiation to the jaw and diaphoresis" is more specifically an acute MI presentation than stable angina. A clinical model trained on medical literature and clinical notes is likely to get this right.

The tradeoff is operational: clinical models typically require ONNX runtime or a Python inference stack, whereas general-purpose models are available through Ollama with a single command.

## Model-aware adapters (`R56`)

Embedding models are not interchangeable endpoints. Retrieval models may require asymmetric query/document prefixes, natural-language task instructions, or no prefix at all; applying Nomic's prefixes to another model can depress quality while still returning structurally valid vectors. Vector dimensions, useful context limits, and recommended tags also vary. A fair `R15` benchmark therefore depends on this adapter layer.

`R56` introduces a curated registry of embedding profiles shared by `sct embed`, `sct semantic`, and MCP. A profile defines:

- A stable profile identifier and the compatible Ollama model/tag.
- Document formatting and query formatting, including model-specific instructions.
- Expected vector dimensions, validated against Ollama responses, plus the model's documented context constraint for build planning.
- The profile/version metadata written into the Arrow artefact, in addition to the exact model identity and embedding text scheme.

Query-time commands must reject an artefact built with a different model/profile or an unknown formatting version. CLI help must list profiles that `sct` actually adapts; accepting an arbitrary Ollama model name must not imply support. Model pulls remain explicit, no network call is introduced at runtime beyond the configured local Ollama endpoint, and existing `nomic-embed-text` artefacts remain readable under their recorded scheme.

The initial compatibility candidates are `nomic-embed-text:v1.5`, `nomic-embed-text-v2-moe`, `qwen3-embedding:0.6b`, and `embeddinggemma`. Inclusion here means "build an adapter and evaluate it", not "promise it as a recommended model". `R15` chooses recommendations and any future default only after measuring the fixed clinical query set, latency, memory, vector dimensions, and full-release Arrow size.

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

#### `nomic-embed-text-v2-moe`
- **Dimensions:** 768
- **Context window:** 512 tokens
- **Strengths:** Newer multilingual retrieval model; distinct profile but the same documented `search_query:` / `search_document:` interface
- **Weaknesses:** Larger local model than v1.5, shorter context, and not clinically trained; no recommendation until R15 compares it on the fixed SNOMED query set
- **Best for:** R15 model comparison and multilingual terminology retrieval experiments
- **Ollama command:** `ollama pull nomic-embed-text-v2-moe`
- **Verified locally:** Ollama 0.32.9 returned finite 768-dimensional query and document vectors on 18 August 2026

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
sct embed --ndjson snomed.ndjson --output snomed.arrow --model nomic-embed-text
sct embed --ndjson snomed.ndjson --output snomed.arrow --model mxbai-embed-large

# ONNX-backed (downloads model if not present)
sct embed --ndjson snomed.ndjson --output snomed.arrow --model sapbert
sct embed --ndjson snomed.ndjson --output snomed.arrow --model medcpt
sct embed --ndjson snomed.ndjson --output snomed.arrow --model biobert

# With explicit ONNX model file (advanced, skip download)
sct embed --ndjson snomed.ndjson --output snomed.arrow \
  --model onnx --onnx-file ~/models/sapbert.onnx

# Benchmark mode - embeds a sample and reports quality metrics
sct embed --benchmark --models sapbert,nomic-embed-text,medcpt \
  --ndjson snomed.ndjson --output-dir ./benchmark-results/
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

Each concept starts as a single body constructed from its fields:

```
{preferred_term}. {fsn}. Synonyms: {synonyms joined by ", "}. Hierarchy: {hierarchy path joined by " > "}.
```

Example for Myocardial infarction:
```
Myocardial infarction. Myocardial infarction (disorder). Synonyms: Heart attack, Cardiac infarction, Infarction of heart, MI - myocardial infarction. Hierarchy: Clinical finding > Disease.
```

The selected R56 profile applies its model-specific document prefix/instruction to this body and the paired query formatting described under [Model-aware adapters](#model-aware-adapters-r56). The profile identifier locks those transformations independently of the body scheme version.

This concatenation gives the model the full vocabulary surface of the concept. Alternatives considered:

- **Preferred term only** - fast, small, but misses synonyms; "heart attack" would not find MI if the model hasn't learned the synonymy
- **FSN only** - includes semantic tag (disorder) which adds noise
- **No hierarchy path** - shorter, but loses useful top-level and parent context
- **PT, FSN, synonyms, and hierarchy path** - current implementation; best observed recall with manageable length for most concepts

The embedding text scheme and model-profile versions are stored separately in Arrow metadata for compatibility checks and future migrations. Query-time search uses the profile's paired query transformation rather than reconstructing the document text.

---

## Arrow file format

The output is an Arrow IPC file with the following schema:

```
schema:
  - id: utf8
  - preferred_term: utf8
  - hierarchy: utf8
  - embedding: fixed_size_list<float32>[768]   -- dimension varies by model

metadata:
  sct.embedding_model: "nomic-embed-text"
  sct.embed_text_scheme: "2"
  sct.edition_label: "UK Monolith"             # provenance, when available
  sct.release_date: "2026-07-01"
  sct.release_id: "uk_sct2mo_42.3.0_20260701000001Z"
  sct.sct_version: "0.21.0"
  sct.created_at: "2026-08-03T18:00:00Z"
  sct.content_fingerprint: "sha256:..."
```

The metadata block is critical - `sct semantic` and `sct mcp` read it when serving a query to validate that the embedding model matches the configured runtime model.

---

## Benchmarking framework

Because the right model choice depends on use case, `sct embed --benchmark` evaluates models against a standard test set of clinical-to-SNOMED mappings.

### Delivery plan (`R56` then `R15`)

`R56` is phase zero of the semantic-quality programme. The later experiments are invalid until every candidate model receives its own documented query/document formatting and the Arrow artefact records that profile precisely.

1. **R56 - trustworthy model plumbing.** Introduce the shared curated profile registry, preserve current Nomic compatibility, fail closed for unsupported or mismatched profiles, and compatibility-check the initial Ollama candidates. Capture model/profile identity, dimensions, build duration, peak model memory, query latency, and Arrow size. Do not change the default based on general MTEB scores.
2. **R15 baseline - freeze the evidence.** Run the committed clinical query set against one-vector-per-concept Nomic and every supported R56 profile. Record full ranked outputs as well as aggregate metrics so regressions remain diagnosable. Pin the SNOMED release, model tags, `sct` commit, hardware, and embedding text scheme.
3. **R15 representation - reduce synonym dilution.** Compare the current PT + FSN + all-synonyms + hierarchy paragraph with separate PT/FSN/synonym vectors and max-per-concept pooling. Measure index growth and scan latency, not just retrieval quality. Also test hierarchy omitted, retained separately, and used only as a filter/feature.
4. **R15 retrieval - combine complementary evidence.** Compare dense-only ranking with FST exact/prefix/fuzzy candidate generation and a documented fusion method (start with reciprocal-rank fusion; separately report exact-synonym boosts). A typo must not be delegated solely to the embedding model when the lexical index can recover it deterministically.
5. **R15 reranking and constraints.** On the bounded fused candidate set, evaluate active-status preference, optional hierarchy/semantic-tag constraints, and only then a local clinical reranker if simpler features do not resolve the documented failures. Keep candidate generation and reranking metrics separate.
6. **Decision gate.** Choose supported recommendations and any new default from the fixed evidence. Report quality by query class alongside build/query time, RAM/VRAM, dimensions, and artefact size. A model or strategy does not ship as the default merely because its aggregate score is higher; clinically important regression cases and operational cost remain explicit.

The initial named regression cases are:

| Query | Expected concept | August 2026 Nomic baseline | Failure class |
|---|---|---|---|
| `heart attack` | `22298006` Myocardial infarction | rank 31, cosine 0.6934 | synonym dilution / literal-phrase competition |
| `heart attak` | `22298006` Myocardial infarction | absent from top 1,000; FST fuzzy rank 1 | misspelling / lexical-semantic fusion |
| `burning when I wee` | `58250006` Scalding pain on urination | rank 407, cosine 0.5824; thermal burns rank first | colloquial symptom language / specificity |
| `water on the lungs` | pulmonary oedema disorder | procedure ranks above intended disorder | hierarchy/category drift |
| `sticky blood` | hypercoagulable state | target absent | idiom without shared vocabulary |

These figures were rerun against the 29 July 2026 UK Monolith artefact containing 1,151,029 Nomic vectors; earlier July 1 figures are not the baseline. For the first three cases, the minimum acceptance target is the expected active concept in the top five; `heart attack` and `heart attak` should reach rank one when exact/fuzzy synonym evidence is enabled. These cases are seeds for the broader 50-100 query set, not a sufficient benchmark by themselves and not fixtures to overfit model instructions against.

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
- Misspellings where fuzzy lexical retrieval and dense retrieval should be compared separately
- Colloquial anatomy and symptom descriptions where the intended specificity matters
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
| `build_time` | Full-release embedding build duration on recorded hardware |
| `model_memory` | Peak RAM/VRAM attributable to the embedding model |
| `embedding_dimensions` | Stored vector width for the profile |
| `arrow_size` | Full Arrow artefact size, including multi-vector variants |

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

Embed concepts in batches rather than one at a time. Optimal batch size varies by model and hardware but 32-128 is typical for BERT-class models. The ONNX runtime handles batching efficiently, and Ollama's `/api/embed` endpoint accepts an input array so each chunk can be sent in one HTTP request.

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

### Ollama batching

Send each configured chunk as the `input` array of one `/api/embed` request. This avoids one HTTP round trip per concept while `--batch-size` keeps request and response memory bounded. Query-time stdin batches use the same endpoint: `sct semantic -` accepts at most 100 query strings, sends them in one input array, and scans the Arrow file once.

---

## Future model candidates

Worth evaluating as they mature:

- **BGE-M3** (BAAI) - multilingual, strong biomedical performance, available via Ollama
- **E5-mistral-7b** - large but very high quality general embeddings
- **OpenAI text-embedding-3-large** - API-based, not local, but useful as a quality ceiling benchmark
- **Domain-specific fine-tuned models** - fine-tuning SAPBERT on UK primary care consultation language would likely improve top-1 accuracy significantly for the target use case; a future research contribution

---

## Relationship to `sct mcp`

The model name recorded in the Arrow file metadata is the single source of truth for which model `sct mcp` must use at query time. The two must match. If they do not, the `snomed_semantic_search` call returns a model-mismatch error; the server remains available for its other tools.

This means users can maintain multiple index files for different models:

```
snomed-sapbert.arrow        ← for clinical coding
snomed-nomic.arrow          ← for general exploration
```

And switch between them by pointing `sct mcp --embeddings` at the appropriate file.
