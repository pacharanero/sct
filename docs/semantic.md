+++
title = "sct semantic"
weight = 5
+++

Semantic similarity search over a SNOMED CT Arrow IPC embeddings file.
**When to use:** you want to search by *meaning* rather than exact words. `sct semantic "sticky blood"` returns hypercoagulable state concepts; `sct semantic "water tablets"` returns diuretics — even though neither phrase appears in SNOMED. For exact keyword search, [`sct lexical`](lexical.md) is faster and requires no Ollama.
Embeds your query text via Ollama and performs cosine similarity against all concept embeddings in the `.arrow` file produced by `sct embed`. Returns the concepts whose meaning is closest to your query — including concepts that don't share any keywords.

---

## Usage

```
sct semantic <QUERY> [--embeddings <FILE>] [--model <MODEL>] [--ollama-url <URL>] [--limit <N>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `<QUERY>` | *(required)* | Natural-language search query. |
| `--embeddings <FILE>` | `snomed-embeddings.arrow` | Arrow IPC file produced by `sct embed`. |
| `--model <MODEL>` | `nomic-embed-text` | Ollama model — must match the model used when building the embeddings. |
| `--ollama-url <URL>` | `http://localhost:11434` | Ollama base URL. |
| `--limit <N>` | `10` | Maximum number of results. |

---

## Prerequisites

Ollama must be running with the same model that was used to build the embeddings:

```bash
ollama serve
ollama pull nomic-embed-text  # if not already pulled
```

---

## Examples

```bash
# Basic semantic search
sct semantic "heart attack"

# Finds concepts by meaning even if the words differ
sct semantic "difficulty breathing"
sct semantic "water tablets"          # → diuretic concepts
sct semantic "sticky blood"           # → hypercoagulable state concepts

# Return more results
sct semantic "chest pain" --limit 20

# Use embeddings built with a different model
sct semantic "fracture" \
  --embeddings snomed-embeddings-small.arrow \
  --model mxbai-embed-large

# Use embeddings on a remote host
sct semantic "epilepsy" --ollama-url http://192.168.1.100:11434
```

---

## Output

```
10 closest concepts to "heart attack":

  0.9821  [22298006] Heart attack
  0.9734  [57054005] Acute myocardial infarction
  0.9701  [233843008] Silent myocardial infarction
  0.9688  [194828000] Angina pectoris
  ...
```

Scores range from 0 (unrelated) to 1 (identical meaning). Anything above ~0.85 is typically a strong match.

---

## How it works

1. Your query text is sent to Ollama, which returns a 768-dimensional float32 vector.
2. The `.arrow` file is scanned; cosine similarity is computed between the query vector and each concept's embedding.
3. The top-N concepts by score are printed.

The search is entirely local — no network call beyond the Ollama process running on your machine.

---

## Comparison with `sct lexical`

| | `sct lexical` | `sct semantic` |
|---|---|---|
| Basis | Keyword matching (FTS5) | Meaning / vector similarity |
| Input | SQLite `.db` | Arrow `.arrow` + Ollama |
| Speed | Instant | ~1–2 s (embedding the query) |
| Finds synonyms | Only if indexed | Yes |
| Finds related concepts without shared words | No | Yes |
| Works offline | Yes | Requires local Ollama |

Use `sct lexical` when you know the SNOMED term. Use `sct semantic` when you're describing a concept in plain language or exploring related concepts.

---

## See also

- [`sct lexical`](lexical.md) — keyword search (faster, no Ollama required)
- [`sct embed`](embed.md) — build the embeddings file

---

*Next: connect Claude with [`sct mcp`](mcp.md), which also supports SNOMED search.*
