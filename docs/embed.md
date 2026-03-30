+++
title = "sct embed"
weight = 6
+++

Generate vector embeddings from a SNOMED CT NDJSON artefact and write an **Apache Arrow IPC file** for semantic vector search.

Embeddings are produced by a local [Ollama](https://ollama.com) instance. The Arrow IPC output can be queried directly in DuckDB, loaded into Python (PyArrow/Pandas), or imported into LanceDB or any Arrow-compatible vector store.

---

## Usage

```
sct embed --input <NDJSON> [--output <FILE>] [--model <MODEL>] [--batch-size <N>] [--ollama-url <URL>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--input <FILE>` | *(required)* | NDJSON file produced by `sct ndjson`. Use `-` for stdin. |
| `--output <FILE>` | `snomed-embeddings.arrow` | Output Arrow IPC file. |
| `--model <MODEL>` | `nomic-embed-text` | Ollama model name to use for embeddings. |
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

---

## Example

```bash
# Pull the model once
ollama pull nomic-embed-text

# Generate embeddings (takes ~30 minutes for 831k concepts on CPU)
sct embed \
  --input snomed.ndjson \
  --output snomed-embeddings.arrow
```

### Custom Ollama URL (e.g. remote GPU host)

```bash
sct embed \
  --input snomed.ndjson \
  --ollama-url http://192.168.1.100:11434 \
  --output snomed-embeddings.arrow
```

---

## Embedding text format

Each concept is embedded as a single string:

```
{preferred_term}. {fsn}. Synonyms: {synonyms joined with ", "}. Hierarchy: {hierarchy_path joined with " > "}.
```

Example:
```
Heart attack. Myocardial infarction (disorder). Synonyms: Cardiac infarction, MI - Myocardial infarction. Hierarchy: SNOMED CT Concept > Clinical finding > Disorder of cardiovascular system > Ischemic heart disease > Myocardial infarction.
```

---

## Output format

The output is a single Arrow IPC (`.arrow`) file with the following schema:

| Column | Type | Description |
|---|---|---|
| `id` | `utf8` | SCTID |
| `preferred_term` | `utf8` | Preferred term |
| `hierarchy` | `utf8` | Top-level hierarchy name |
| `embedding` | `fixed_size_list<float32>[N]` | Vector embedding (dimension determined by model) |

For `nomic-embed-text` the dimension is 768.

---

## Querying the embeddings

### DuckDB (vector similarity search)

DuckDB can read Arrow IPC files directly. For vector search, you first need to embed your query via Ollama, then use `array_cosine_similarity`:

```sql
-- Load the vss extension (DuckDB >= 0.10)
INSTALL vss;
LOAD vss;

-- Find the 10 closest concepts to a pre-computed query vector
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
resp = ollama.embed(model="nomic-embed-text", input=["heart attack"])
q = np.array(resp["embeddings"][0], dtype=np.float32)

# Cosine similarity (normalised vectors)
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

- Embedding 831k concepts takes significant time on CPU (estimate ~30 min). A GPU or Apple Silicon machine will be much faster.
- `nomic-embed-text` produces 768-dimensional float32 vectors. The actual dimension is detected from the first Ollama call; other models with different dimensions will work automatically.
- The complete dataset is held in memory during embedding. For a machine with limited RAM, use `--batch-size 16` or lower.
- To search the resulting `.arrow` file from the command line, use [`sct semantic`](semantic.md).

