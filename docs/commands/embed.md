# sct embed

Generate vector embeddings from a SNOMED CT NDJSON artefact and write an **Apache Arrow IPC file** for semantic vector search.

Embeddings are produced by a local [Ollama](https://ollama.com) instance - no bundled model, no external API key. The Arrow IPC output can be queried in DuckDB, loaded into Python (PyArrow/Pandas), or imported into LanceDB or any Arrow-compatible vector store.

`sct embed` is the only `sct` subcommand that requires an external process (Ollama). All others work fully offline.

Design rationale and model-selection notes live in [`spec/commands/embed.md`](https://github.com/pacharanero/sct/blob/main/spec/commands/embed.md).

---

## Usage

```
sct embed --ndjson <NDJSON> [--output <FILE>] [--model <MODEL>] [--batch-size <N>] [--ollama-url <URL>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--ndjson <FILE>` | *(required)* | NDJSON file produced by `sct ndjson`. Use `-` for stdin. Accepts `--input` as an alias. |
| `--output <FILE>` | *(input name + `-embeddings.arrow`)* | Output Arrow IPC file. `uk-monolith-42.ndjson` → `uk-monolith-42-embeddings.arrow`; stdin input gives `snomed-embeddings.arrow`. |
| `--model <MODEL>` | `nomic-embed-text` | Supported profile: `nomic-embed-text` (or pinned `:v1.5`), `nomic-embed-text-v2-moe`, `qwen3-embedding:0.6b`, or `embeddinggemma`. Other models are rejected until correctly adapted. |
| `--batch-size <N>` | `64` | Number of concepts to embed per Ollama API call. |
| `--ollama-url <URL>` | `http://localhost:11434` | Ollama base URL. |

---

## Prerequisites: Ollama

This command requires [Ollama](https://ollama.com) to be running with the `nomic-embed-text` model pulled:

```bash
# Install Ollama (see https://ollama.com/download)
ollama pull nomic-embed-text
ollama serve   # or it may already be running as a service
```

Verify it's working:

```bash
curl http://localhost:11434/api/embed \
  -d '{"model": "nomic-embed-text", "input": ["test"]}'
```

If Ollama is not running when you run `sct embed`, you will see a helpful error with instructions to start it.

The newer Nomic v2 MoE profile is also supported, but is not the default or recommended over v1 until R15 measures it against the fixed clinical query set:

```bash
ollama pull nomic-embed-text-v2-moe
sct embed --ndjson snomed.ndjson \
  --model nomic-embed-text-v2-moe \
  --output snomed-embeddings-nomic-v2.arrow
```

Qwen3 Embedding 0.6B is supported under an explicit tag. Its profile leaves documents unprefixed and applies a versioned clinical-terminology retrieval instruction to queries, as recommended by Qwen's instruction-aware interface:

```bash
ollama pull qwen3-embedding:0.6b
sct embed --ndjson snomed.ndjson \
  --model qwen3-embedding:0.6b \
  --output snomed-embeddings-qwen3-0.6b.arrow
```

Do not pass bare `qwen3-embedding`: in Ollama that currently selects the 8B model, not the supported 0.6B profile.

EmbeddingGemma uses Google's documented retrieval prompts on both sides and remains 768-dimensional:

```bash
ollama pull embeddinggemma
sct embed --ndjson snomed.ndjson \
  --model embeddinggemma \
  --output snomed-embeddings-gemma.arrow
```

---

## Example

```bash
# Pull the model once
ollama pull nomic-embed-text

# Generate embeddings (takes ~30 minutes for 837,930 concepts on CPU)
sct embed \
  --ndjson snomed.ndjson \
  --output snomed-embeddings.arrow
```

### Custom Ollama URL (e.g. remote GPU host)

```bash
sct embed \
  --ndjson snomed.ndjson \
  --ollama-url http://192.168.1.100:11434 \
  --output snomed-embeddings.arrow
```

---

## Embedding text format

Each concept starts from one body combining all its human-readable content:

```
{preferred_term}. {fsn}. Synonyms: {synonyms joined with ", "}. Hierarchy: {hierarchy_path joined with " > "}.
```

The selected versioned model profile then applies the model's documented retrieval formatting:

| Profile | Document formatting | Query formatting |
|---|---|---|
| Nomic v1.5 / v2 MoE | `search_document: {body}` | `search_query: {query}` |
| Qwen3 Embedding 0.6B | `{body}` | `Instruct: {clinical retrieval task}\nQuery:{query}` |
| EmbeddingGemma | `title: none \| text: {body}` | `task: search result \| query: {query}` |

Real example (Myocardial infarction, `22298006`, from a UK Monolith build):
```
Myocardial infarction. Myocardial infarction (disorder). Synonyms: Infarction of heart, Cardiac infarction, Heart attack, Myocardial infarct, MI - myocardial infarction. Hierarchy: SNOMED CT Concept > Clinical finding > Finding of trunk structure > Finding of upper trunk > Finding of thoracic region > Disorder of thorax > Disorder of mediastinum > Heart disease > Structural disorder of heart > Myocardial lesion > Myocardial necrosis > Myocardial infarction.
```

This gives the model the concept's full vocabulary surface, so a query sharing *any* of these words has something to match against. It is not a guarantee: this scheme has real, documented limitations - see [`sct semantic` - Known limitations](semantic.md#known-limitations) before relying on results.

---

## Output format

The output is a single Arrow IPC (`.arrow`) file with the following schema:

| Column | Type | Description |
|---|---|---|
| `id` | `utf8` | SCTID |
| `preferred_term` | `utf8` | Preferred term |
| `hierarchy` | `utf8` | Top-level hierarchy name |
| `active` | `bool` | False for a concept SNOMED International has retired |
| `embedding` | `fixed_size_list<float32>[N]` | Vector embedding (dimension determined by model) |

For `nomic-embed-text` the dimension is 768.

`active` is `true` for every row unless the source NDJSON was built with [`sct ndjson --include-inactive`](ndjson.md). [`sct semantic`](semantic.md) treats an embeddings file written before this column existed the same way: every row reads active.

The Arrow schema also carries metadata identifying how the file was built: `sct.embedding_model`, `sct.embedding_profile` (the versioned model-specific query/document adapter), and `sct.embed_text_scheme` (the version of the concept-text composition above), alongside the usual release provenance (edition, release date, `sct` version). `sct semantic` validates all three before querying - a same-dimension model or formatting swap would otherwise produce silently misleading cosine scores. Existing Nomic scheme-2 files written before profile metadata remain compatible.

---

## Querying the embeddings

### Via `sct semantic` (recommended)

```bash
sct semantic "blocked coronary artery" --embeddings snomed-embeddings.arrow --limit 5
```

See [`sct semantic`](semantic.md) for full documentation.

### DuckDB (vector similarity search)

```sql
INSTALL vss;
LOAD vss;

SELECT id, preferred_term, hierarchy,
       array_cosine_similarity(embedding, $query_vec::FLOAT[768]) AS score
FROM read_ipc_auto('snomed-embeddings.arrow')
ORDER BY score DESC
LIMIT 10;
```

### Python (PyArrow + NumPy)

```python
import pyarrow.ipc as ipc
import numpy as np
import ollama

# Load embeddings
with ipc.open_file("snomed-embeddings.arrow") as f:
    table = f.read_all()

embeddings = np.array(table["embedding"].to_pylist(), dtype=np.float32)

# Embed query
resp = ollama.embed(model="nomic-embed-text", input=["search_query: heart attack"])
q = np.array(resp["embeddings"][0], dtype=np.float32)

# Cosine similarity
norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
normed = embeddings / (norms + 1e-9)
q_normed = q / (np.linalg.norm(q) + 1e-9)
scores = normed @ q_normed

top_idx = np.argsort(scores)[::-1][:10]
ids = table["id"].to_pylist()
terms = table["preferred_term"].to_pylist()
for i in top_idx:
    print(f"{scores[i]:.4f}  {ids[i]}  {terms[i]}")
```

### Import into LanceDB

```python
import lancedb
import pyarrow.ipc as ipc

with ipc.open_file("snomed-embeddings.arrow") as f:
    table = f.read_all()

db = lancedb.connect("snomed.lance")
db.create_table("concepts", data=table, mode="overwrite")
```

---

## Notes

- Embedding 837,930 concepts takes significant time on CPU (~30 min). A GPU or Apple Silicon machine will be much faster.
- `nomic-embed-text` produces 768-dimensional float32 vectors. Other models with different dimensions will work automatically.
- The complete dataset is held in memory during embedding. For limited RAM, use `--batch-size 16` or lower.
- The `.arrow` file is also consumed by `sct mcp --embeddings` to expose `snomed_semantic_search` to AI clients.
