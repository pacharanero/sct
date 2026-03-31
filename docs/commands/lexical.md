Keyword search over the SNOMED CT SQLite database using FTS5 full-text search.

**When to use:** you know what words to search for. `sct lexical "heart attack"` returns concepts containing those words. For meaning-based search (when exact words don't match), use [`sct semantic`](semantic.md).

---

## Usage

```
sct lexical <QUERY> [--db <FILE>] [--hierarchy <NAME>] [--limit <N>]
```

## Options

| Argument / Flag | Default | Description |
|---|---|---|
| `<QUERY>` | *(required)* | Search query. FTS5 syntax: `"exact phrase"`, `prefix*`, `term AND term`, etc. |
| `--db <FILE>` | `snomed.db` (cwd or `$SCT_DB`) | SQLite database produced by `sct sqlite`. |
| `--hierarchy <NAME>` | *(all)* | Restrict results to a top-level hierarchy (e.g. `"Clinical finding"`). |
| `--limit <N>` | `10` | Maximum number of results. |

---

## Examples

```bash
sct lexical "heart attack"
sct lexical "myocardial infarct*"
sct lexical "heart attack" --hierarchy "Clinical finding"
sct lexical "beta blocker" --limit 20 --db /data/snomed.db
```

---

## FTS5 query syntax

| Syntax | Example | Matches |
|---|---|---|
| Plain terms | `heart attack` | Concepts containing both words (implicit phrase) |
| Exact phrase | `"heart attack"` | Concepts containing the exact phrase |
| Prefix | `cardio*` | Concepts with any word starting with "cardio" |
| Boolean AND | `heart AND failure` | Concepts containing both terms |
| Boolean OR | `infarct OR infarction` | Concepts containing either term |
| Boolean NOT | `asthma NOT occupational` | Asthma, excluding occupational variants |

Plain text queries (no operators) are automatically quoted to avoid parse errors on special characters. Results are ranked by FTS5 BM25 relevance.

---

## Comparison with `sct semantic`

| | `sct lexical` | `sct semantic` |
|---|---|---|
| Basis | Keyword matching (FTS5) | Meaning / vector similarity |
| Input | SQLite `.db` | Arrow `.arrow` + Ollama |
| Speed | Instant | ~1–2 s (embedding the query) |
| Finds synonyms | Only if indexed | Yes |
| Finds related concepts without shared words | No | Yes |
| Works offline | Yes | Requires local Ollama |

Use `sct lexical` when you know the SNOMED term. Use [`sct semantic`](semantic.md) when you're describing a concept in plain language.
