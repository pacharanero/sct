# `sct semantic` — Semantic similarity search over concept embeddings

Embeds a natural-language query via Ollama and performs cosine similarity search against the
Arrow IPC embeddings file produced by `sct embed`. Returns the top-N most semantically similar
SNOMED CT concepts — useful for queries where lexical matching is brittle (synonyms,
paraphrasing, clinical shorthand).

---

## Synopsis

```bash
sct semantic <query> [--embeddings <arrow>] [--model <name>] [--limit <n>]
```

## Arguments & flags

| Argument / Flag | Default | Description |
|---|---|---|
| `<query>` | *(required)* | Natural-language query text. |
| `--embeddings <file>` | `snomed-embeddings.arrow` in cwd | Arrow IPC file produced by `sct embed`. |
| `--model <name>` | `nomic-embed-text` | Ollama embedding model — must match the model used by `sct embed`. |
| `--ollama-url <url>` | `http://localhost:11434` | Ollama API base URL. |
| `--limit <n>` | `10` | Maximum number of results. |

---

## Prerequisites

Ollama must be running with the same model used to build the embeddings:

```bash
ollama pull nomic-embed-text
ollama serve
```

If Ollama is not reachable, `sct semantic` exits with a clear error message and instructions.

---

## Examples

```bash
sct semantic "heart attack"
sct semantic "difficulty breathing" --limit 20
sct semantic "beta blocker" --model nomic-embed-text --embeddings /data/snomed-embeddings.arrow
```

---

## Design notes

- The query text is embedded using the same template format as `sct embed`, ensuring the query
  vector lives in the same embedding space as the concept vectors.
- Cosine similarity is computed in-process over the full Arrow file — no vector store or server
  required.
- For lexical (keyword) search, use `sct lexical` instead.
