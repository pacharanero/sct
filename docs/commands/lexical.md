# sct lexical

Keyword search over the SNOMED CT SQLite database using FTS5 full-text search.

**When to use:** you know what words to search for. `sct lexical "heart attack"` returns concepts containing those words. For meaning-based search (when exact words don't match), use [`sct semantic`](semantic.md).

---

## Usage

```
sct lexical <QUERY|-> [--db <FILE>] [--hierarchy <NAME>] [--limit <N>] [--format text|json|yaml]
```

## Options

| Argument / Flag | Default | Description |
|---|---|---|
| `<QUERY>` | *(required)* | Search query. FTS5 syntax: `"exact phrase"`, `prefix*`, `term AND term`, etc. Pass `-` to read one complete query per line from stdin. |
| `--db <FILE>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |
| `--hierarchy <NAME>` | *(all)* | Restrict results to a top-level hierarchy (e.g. `"Clinical finding"`). |
| `--status <STATUS>` | `all` | Restrict by lifecycle status: `all`, `active`, or `inactive`. Only has an effect on a database built with `--include-inactive`. |
| `--limit <N>` | `10` | Maximum number of results. |
| `-f, --format <FORMAT>` | `text` | Output format: `text`, `json`, or `yaml`. |
| `--ids` | off | Emit only matching SCTIDs (newline-delimited) for piping into other commands; mutually exclusive with an explicit `--format`. |
| `--template <TEMPLATE>` | *(built-in)* | Override the per-concept line template (text output only). See [`sct refset`](refset.md) for the variable list. |
| `--template-fsn-suffix <TEMPLATE>` | *(built-in)* | Override the FSN suffix template (rendered only when the FSN differs from the preferred term). |
| `--provenance` / `--no-provenance` | on for TTY, off otherwise | Show/hide release provenance (edition, release date) on this query's output. |

An empty search exits `0`. Text and `--ids` output leave stdout empty and write the "No results" hint to stderr; structured formats emit an empty collection.

---

## Examples

```bash
sct lexical "heart attack"
sct lexical "myocardial infarct*"
sct lexical "heart attack" --hierarchy "Clinical finding"
sct lexical "beta blocker" --limit 20 --db /data/snomed.db

# Pipe matching SCTIDs straight into a code list
sct lexical "asthma" --ids --limit 50 | sct codelist add asthma.codelist -

# JSON output for scripting
sct lexical "heart attack" --format json

# Search several queries and retain their separate result sets
printf '%s\n' 'heart attack' diabetes | sct lexical - --format json
```

---

## Batch input

Passing `-` reads each trimmed, nonblank stdin line as a complete query, with a 64 KiB line limit, up to 10,000 entries and 100,000 retained results across the batch. Unlike code-list input, `#` has no comment meaning here because it may be intentional query text. `--limit` and `--hierarchy` apply independently to every query, and input order and duplicates are preserved.

Text and `--ids` output flatten result sets in query order; `--ids` cannot be combined with an explicit `--format`. JSON and YAML emit one document shaped as `{ "items": [{ "input": "heart attack", "result": [...] }] }`; use a structured format when the caller needs to retain query/result boundaries. Every query completes before stdout is written. SQLite applies `--limit` while FTS5 streams rank order, avoiding a full-result sort; equal-rank rows retain stable FTS index order for a fixed database.

---

## Inactive concepts

A concept SNOMED International has retired is prefixed with a flag:

```
⚠ [INACTIVE] 9468002 | Inactive example disorder (Clinical finding)
```

The prefix is applied by the shared renderer rather than the line template, so `--template` cannot remove it: a retired code that looks identical to a live one in a result list is the failure this exists to prevent. Structured output carries the same information as an `active` boolean on each hit.

This only arises on a database built with [`sct ndjson --include-inactive`](ndjson.md); the default build contains active concepts only, so no flag ever appears. Use [`sct lookup`](lookup.md) on a flagged concept to see why it was retired and what replaces it.

`--status` narrows results to one lifecycle state, which is how you answer "which of these has been retired?" without dropping to SQL:

```bash
# Only retired concepts - e.g. auditing a code list after a release
sct lexical "disorder" --status inactive --limit 5

# Only concepts still current
sct lexical "disorder" --status active

# Retired ids, piped into a code list for review
sct lexical "disorder" --status inactive --ids | sct codelist add review.codelist -
```

`--ids` applies the same filter, so a piped set always matches what was shown. The default is `all` rather than `active`: a retired concept is flagged, not hidden, because a search that silently omits it is how an old record gets misread as current.

## FTS5 query syntax

| Syntax | Example | Matches |
|---|---|---|
| Plain terms | `heart attack` | Concepts containing both words (implicit phrase) |
| Exact phrase | `"heart attack"` | Concepts containing the exact phrase |
| Prefix | `cardio*` | Concepts with any word starting with "cardio" |
| Boolean AND | `heart AND failure` | Concepts containing both terms |
| Boolean OR | `infarct OR infarction` | Concepts containing either term |
| Boolean NOT | `asthma NOT occupational` | Asthma, excluding occupational variants |

Plain text queries (no operators) are automatically quoted to avoid parse errors on special characters. Results are ranked by FTS5 BM25 relevance; equal-rank rows use the FTS index's stable order within a fixed database.

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
