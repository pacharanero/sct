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

The tool requires a vector embedding index built by `sct embed`. It reads from an Arrow IPC file (`.arrow`) produced by that command. An unavailable configured file produces a tool error naming the path.

The embedding model used at query time must match the model used to build the index. The `.arrow` file header records which model was used; the tool validates compatibility when a semantic-search call opens the file.

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
        "description": "Maximum number of results to return. Default 10, maximum 100.",
        "default": 10
      }
    },
    "required": ["query"],
    "additionalProperties": false
  }
}
```

---

## Response format

```json
[
  {
    "id": "22298006",
    "preferred_term": "Myocardial infarction",
    "similarity": 0.91
  },
  {
    "id": "57054005",
    "preferred_term": "Acute myocardial infarction",
    "similarity": 0.88
  }
]
```

The response is an array ordered by descending cosine similarity, then SCTID for stable ties. Cosine similarity is in the range -1.0 to 1.0; higher is more similar. A zero-result search returns `[]`.

---

## Interaction with `snomed_search`

The two tools are designed to work together in a reasoning chain. The recommended LLM pattern is:

1. `snomed_semantic_search` - retrieve candidates by meaning from the clinical narrative
2. Review candidates and their relative ranking; scores are model-dependent and have no universal confidence threshold
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

The query text is embedded through the configured Ollama endpoint using the same model that produced the index:

```
POST http://localhost:11434/api/embed
{"model": "nomic-embed-text", "input": ["search_query: <query text>"]}
```

Ollama must be running. Connection and model errors are returned as MCP tool errors with instructions to start Ollama or pull the configured model.

### Vector similarity search

The Arrow IPC file contains all concept embeddings. The implementation scans every vector, computes cosine similarity, and retains a bounded top-K heap. Approximate nearest-neighbour indexes and hierarchy filters are not part of the current tool contract.

### Registration and call-time validation

`sct mcp` registers `snomed_semantic_search` only when `--embeddings <FILE>` is supplied explicitly. Startup stores the path and Ollama settings without contacting Ollama or opening the Arrow file. The first tool call validates file access, recorded model compatibility, response count/dimensions/finiteness, and Ollama availability; failures are returned as tool errors without crashing the server.

---

## CLI flag additions to `sct mcp`

```bash
sct mcp --db snomed.db --embeddings snomed.arrow --model nomic-embed-text
```

| Flag | Default | Description |
|---|---|---|
| `--embeddings` | none | Explicit path to an Arrow embedding index; supplying it registers the tool |
| `--model` | `nomic-embed-text` | Ollama model, which must match the Arrow metadata |
| `--ollama-url` | `http://localhost:11434` | Ollama base URL |

If the `--embeddings` flag is omitted, `sct mcp` starts normally but `snomed_semantic_search` is not registered as an available tool. This preserves backwards compatibility - existing `sct mcp` users without an embedding index are unaffected.

---

## Error responses

| Condition | Error message |
|---|---|
| Tool not configured | `"snomed_semantic_search is not available: start sct mcp with --embeddings <file>"` |
| Embeddings file unavailable | File-open error naming the configured path |
| Model mismatch | Error naming the stored and requested models and the matching `--model` / rebuild commands |
| Embedding text-scheme mismatch | Error naming the stored and expected scheme and requiring an `sct embed` rebuild |
| Ollama unavailable | Error naming the configured endpoint and suggesting `ollama serve` / `ollama pull` |
| Invalid Ollama response | Error for wrong response count, empty/inconsistent dimensions, or non-finite values |

---

## Benchmarking targets

| Operation | Target |
|---|---|
| Query embedding (Ollama, nomic-embed-text) | Measure separately from local scan |
| Exact vector similarity scan | Benchmark against representative Arrow artefacts |
| Total tool response time | Report embedding and scan components together |

These targets are for interactive MCP use. Batch coding pipelines have different requirements and are out of scope for this tool.
