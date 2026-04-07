# `snomed_semantic_search` - MCP Tool Specification

## Overview

A new MCP tool added to `sct mcp` that performs semantic vector search over SNOMED CT concepts, complementing the existing `snomed_search` lexical FTS5 tool. Where `snomed_search` matches on exact or near-exact terms, `snomed_semantic_search` retrieves concepts by meaning - handling clinical narrative language, abbreviations, lay terms, and descriptions that do not share vocabulary with SNOMED preferred terms.

---

## Motivation

The existing `snomed_search` tool uses SQLite FTS5 full-text search. This works well when the query language matches SNOMED's preferred terminology:

```
"myocardial infarction" → finds Myocardial infarction (disorder) ✓
"heart attack"          → finds Heart attack synonym             ✓
```

It fails when the clinical language diverges from SNOMED vocabulary:

```
"crushing central chest pain radiating to jaw with diaphoresis" → poor results ✗
"SOB on minimal exertion, orthopnoea, PND"                      → poor results ✗
"can't catch breath going upstairs, legs are puffy"             → poor results ✗
```

The third example is the critical one - a patient's own words in a consultation note. This is precisely the context where automated SNOMED coding is most valuable and where lexical search fails hardest.

`snomed_semantic_search` closes this gap by embedding the query into the same vector space as the pre-embedded SNOMED concepts and retrieving the nearest neighbours.

---

## Prerequisites

The tool requires a vector embedding index built by `sct embed`. It reads from an Arrow IPC file (`.arrow`) produced by that command. If the index file is absent, the tool returns a clear error suggesting the user run `sct embed`.

The embedding model used at query time must match the model used to build the index. The `.arrow` file header records which model was used; `sct mcp` validates this at startup.

---

## Tool definition

```json
{
  "name": "snomed_semantic_search",
  "description": "Search for SNOMED CT concepts by clinical meaning rather than exact terminology. Use this when the query is a clinical narrative, patient-reported symptoms, lay language, abbreviations, or any description that may not match SNOMED preferred terms exactly. Returns ranked candidates with similarity scores. Complement with snomed_search for verification of top results.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Clinical text to search by meaning. Can be a full consultation excerpt, a symptom description, a diagnosis in lay terms, or an abbreviation. Examples: 'crushing chest pain radiating to left arm', 'SOB on exertion with ankle oedema', 'patient says they feel their heart racing'"
      },
      "limit": {
        "type": "integer",
        "description": "Maximum number of results to return. Default 10, maximum 50. Use higher values when the clinical picture is complex or ambiguous and you want more candidates to reason over.",
        "default": 10
      },
      "min_similarity": {
        "type": "number",
        "description": "Minimum cosine similarity threshold (0.0 to 1.0). Results below this score are excluded. Default 0.5. Lower for broader recall, raise to 0.8+ for high-confidence matches only.",
        "default": 0.5
      },
      "hierarchy": {
        "type": "string",
        "description": "Optional. Restrict results to a SNOMED top-level hierarchy. One of: clinical_finding, procedure, body_structure, substance, pharmaceutical, observable, qualifier. Leave unset to search all hierarchies.",
        "enum": [
          "clinical_finding",
          "procedure",
          "body_structure",
          "substance",
          "pharmaceutical",
          "observable",
          "qualifier"
        ]
      }
    },
    "required": ["query"]
  }
}
```

---

## Response format

```json
{
  "query": "crushing chest pain radiating to jaw with sweating",
  "model": "sapbert-mean-token",
  "results": [
    {
      "id": "22298006",
      "preferred_term": "Myocardial infarction",
      "fsn": "Myocardial infarction (disorder)",
      "hierarchy": "Clinical finding",
      "similarity": 0.91,
      "synonyms": ["Heart attack", "Cardiac infarction", "MI - myocardial infarction"]
    },
    {
      "id": "57054005",
      "preferred_term": "Acute myocardial infarction",
      "fsn": "Acute myocardial infarction (disorder)",
      "hierarchy": "Clinical finding",
      "similarity": 0.88
    },
    {
      "id": "194828000",
      "preferred_term": "Angina pectoris",
      "fsn": "Angina pectoris (disorder)",
      "hierarchy": "Clinical finding",
      "similarity": 0.79
    }
  ],
  "total_searched": 412257,
  "search_time_ms": 45
}
```

The `similarity` score is cosine similarity between the query embedding and the concept embedding, in the range 0.0-1.0. Higher is more similar.

---

## Interaction with `snomed_search`

The two tools are designed to work together in a reasoning chain. The recommended LLM pattern is:

1. `snomed_semantic_search` - retrieve top-20 candidates by meaning from the clinical narrative
2. Review candidates - if top result has similarity > 0.85, likely a good match
3. `snomed_search` or `snomed_concept` - verify the top candidate(s), check synonyms and FSN match the clinical intent
4. `snomed_children` / `snomed_ancestors` - navigate hierarchy if a more specific or more general code is needed

This should be reflected in the system prompt guidance provided to the LLM when configuring `sct mcp` for clinical coding use cases.

---

## MCP tool guidance text (for system prompt)

When configuring Claude Desktop or another MCP client for clinical coding, include this guidance:

```
You have access to two complementary SNOMED CT search tools:

- snomed_search: lexical search using exact and near-exact term matching.
  Use when you already know the clinical terminology (e.g. "myocardial infarction",
  "appendectomy").

- snomed_semantic_search: semantic search using vector similarity.
  Use when working with clinical narratives, patient-reported symptoms, lay language,
  or abbreviations. This tool understands meaning rather than just matching words.

For clinical consultation coding, the recommended workflow is:
1. Use snomed_semantic_search with the relevant excerpt from the consultation
2. Review the top candidates and their similarity scores
3. Use snomed_concept or snomed_search to verify your top choice
4. Use snomed_children if a more specific code is clinically appropriate
5. Always prefer the most specific code that is fully supported by the clinical text

Never assign a code solely on the basis of a similarity score - always verify
that the preferred term and FSN match the clinical intent.
```

---

## Implementation notes

### Embedding the query at runtime

The query text is embedded using the same model that produced the index. The embedding call must be synchronous from the MCP tool handler's perspective (the LLM awaits the result).

Two runtime embedding approaches depending on which model is in use:

**Ollama-backed models** (nomic-embed-text, mxbai-embed-large):
```
POST http://localhost:11434/api/embeddings
{"model": "nomic-embed-text", "prompt": "<query text>"}
```
Ollama must be running. If unavailable, return a clear error: `"Ollama not running - start with 'ollama serve' or use a different embedding backend"`.

**ONNX-backed models** (SAPBERT, BioBERT variants):
Run inference directly in-process using the `ort` crate (ONNX Runtime for Rust). The model file is loaded once at `sct mcp` startup and kept in memory. No external service dependency.

The ONNX approach is strongly preferred for clinical models - it eliminates the Ollama dependency and keeps the tool self-contained.

### Vector similarity search

The Arrow IPC file contains all concept embeddings as a 2D float array. Cosine similarity search over 400,000+ vectors of typical embedding dimension (768 for BERT-class models) is fast enough to do naively in ~50ms on modern hardware, but can be accelerated:

- **Naive scan**: iterate all vectors, compute cosine similarity, keep top-K. Simple, no dependencies, adequate for interactive use (~50ms).
- **HNSW index**: use the `usearch` or `instant-distance` crate for approximate nearest neighbour search. Reduces search time to ~5ms. Worth adding if search latency becomes noticeable.

For v1, naive scan is acceptable. Add HNSW as a follow-on optimisation, benchmarked against the naive approach.

### Hierarchy filtering

If `hierarchy` is specified, filter the candidate set before similarity ranking by joining against the concepts table. This reduces the search space and improves precision when the clinical context implies a specific hierarchy (e.g. procedure coding in an operation note).

### Startup validation

At `sct mcp` startup:
1. Check for Arrow index file at configured path
2. Read model name from index metadata
3. Validate embedding model is available (Ollama running, or ONNX file present)
4. Load ONNX model into memory if applicable
5. Log index size, model name, and embedding dimension

If validation fails, `sct mcp` should start but `snomed_semantic_search` should return a structured error rather than crashing the server.

---

## CLI flag additions to `sct mcp`

```bash
sct mcp --db snomed.db --embeddings snomed.arrow --embedding-model sapbert
```

| Flag | Default | Description |
|---|---|---|
| `--embeddings` | `snomed.arrow` in same dir as `--db` | Path to Arrow embedding index |
| `--embedding-model` | auto-detected from index metadata | Override model name |
| `--embedding-backend` | auto | `ollama` or `onnx` |
| `--ollama-url` | `http://localhost:11434` | Ollama base URL if using Ollama backend |
| `--onnx-model` | none | Path to ONNX model file if using ONNX backend |

If `--embeddings` file is absent, `sct mcp` starts normally but `snomed_semantic_search` is not registered as an available tool. This preserves backwards compatibility - existing `sct mcp` users without an embedding index are unaffected.

---

## Error responses

| Condition | Error message |
|---|---|
| No embedding index found | `"No embedding index found at snomed.arrow. Run 'sct embed' to build one."` |
| Model mismatch | `"Index was built with model 'sapbert' but current model is 'nomic-embed-text'. Rebuild with 'sct embed --model sapbert'."` |
| Ollama unavailable | `"Ollama not running. Start with 'ollama serve' or switch to ONNX backend."` |
| ONNX model missing | `"ONNX model file not found at <path>. Download with 'sct embed --download-model'."` |
| Query too long | `"Query exceeds maximum token length for this model (512 tokens). Shorten the clinical excerpt."` |

---

## Benchmarking targets

| Operation | Target |
|---|---|
| Query embedding (ONNX, SAPBERT) | < 20ms |
| Query embedding (Ollama, nomic-embed-text) | < 100ms |
| Vector similarity scan, 412k concepts | < 100ms naive, < 10ms HNSW |
| Total tool response time | < 200ms |

These targets are for interactive MCP use. Batch coding pipelines have different requirements and are out of scope for this tool.
