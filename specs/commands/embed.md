# `sct embed` — Generate vector embeddings for every concept

Takes the canonical NDJSON artefact and produces an Apache Arrow IPC file containing one
embedding vector per concept. Embeddings are generated via a locally-running
[Ollama](https://ollama.com) instance — no bundled model, no external API key required.

---

## Synopsis

```bash
sct embed --input <ndjson> --model <model> --output <arrow>
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--input <file>` | *(required)* | Input `.ndjson` file produced by `sct ndjson`. Use `-` for stdin. |
| `--model <name>` | `nomic-embed-text` | Ollama embedding model name. |
| `--output <file>` | `snomed-embeddings.arrow` | Output Arrow IPC file path. |

---

## Prerequisites

```bash
ollama pull nomic-embed-text
ollama serve
```

If Ollama is not reachable, `sct embed` exits with a clear error message and instructions.

---

## Examples

```bash
sct embed --input snomed-20260311.ndjson \
          --model nomic-embed-text \
          --output snomed-embeddings.arrow
```

---

## Arrow schema

Columns: `id` (Utf8), `preferred_term` (Utf8), `hierarchy` (Utf8), `embedding` (FixedSizeList\<Float32\>).

---

## Embedding text template

Each concept is embedded as:

```
"{preferred_term}. {fsn}. Synonyms: {synonyms joined with ", "}. Hierarchy: {hierarchy_path joined with " > "}"
```

---

## Design notes

- The Arrow IPC file can be queried in DuckDB, loaded via PyArrow, or imported into LanceDB or
  any Arrow-compatible vector store.
- No vector database server is required at query time.
- `sct semantic` reads this file for cosine similarity search.
- `sct mcp` will optionally load this file to expose `snomed_semantic_search` (planned).
- `sct embed` is the only `sct` subcommand that requires an external process (Ollama); all
  others are fully offline.
