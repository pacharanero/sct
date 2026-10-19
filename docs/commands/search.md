# sct search

**One front door onto every search strategy `sct` offers.** `sct` grew keyword, typo-tolerant, and meaning-based search as separate mature commands - `sct lexical`, `sct sayt --fuzzy`, `sct semantic` - each good at a different job, but with no single place that names the choice. `sct search <mode>` is that place.

**When to use:** you want to pick a search strategy explicitly, or you're not sure which of `lexical`/`semantic`/`fuzzy` fits your query. If you already know you want keyword search, `sct lexical` works exactly the same and is one word shorter.

---

## Usage

```
sct search lexical  <QUERY|-> [the same options as `sct lexical`]
sct search semantic <QUERY|-> [the same options as `sct semantic`]
sct search fuzzy    <QUERY|-> [--index <FILE>] [--limit <N>] [--format text|json|yaml] [--ids]
sct search hybrid   <QUERY|-> [--db <FILE>] [--index <FILE>] [--embeddings <FILE>] [--dense-weight <W>] [--lexical-weight <W>] [--fuzzy-weight <W>] [--rrf-k <K>] [--limit <N>] [--format text|json|yaml] [--ids]
```

---

## The three modes

| | `sct search lexical` | `sct search fuzzy` | `sct search semantic` |
|---|---|---|---|
| Basis | Keyword matching (FTS5) | Prefix + multi-word + typo-tolerant (FST) | Meaning / vector similarity |
| Input | SQLite `.db` | FST `.fst` | Arrow `.arrow` + Ollama |
| Speed | Instant | Sub-millisecond | ~1-2 s (embedding the query) |
| Finds a misspelled term | No | Yes (1-2 edit Levenshtein) | Sometimes, if the model generalises |
| Finds synonyms | Only if indexed | Only if indexed | Yes |
| Finds related concepts without shared words | No | No | Yes |
| Works offline | Yes | Yes | Requires local Ollama |

There is no default mode and no bare `sct search <query>` - naming one explicitly is the point of this command. If you don't know which to reach for: you know the word but might have mistyped it → `fuzzy`; you know the word and spelled it right → `lexical`; you're describing the concept rather than naming it → `semantic`.

---

## `lexical` and `semantic`: genuine passthroughs

`sct search lexical` and `sct search semantic` are not reimplementations - they carry the real `sct lexical`/`sct semantic` argument types and call straight into the same functions. Output, flags, batch-stdin handling, and error messages are identical to invoking the shorter command directly; there is no drift to worry about between the two spellings. See [`sct lexical`](lexical.md) and [`sct semantic`](semantic.md) for their full option references.

## `fuzzy`: the same engine as `sct sayt --fuzzy`, in a one-shot form

`sct sayt` searches as you type, live in a terminal or over a stdio/HTTP line protocol - built for a session, not a single query. `sct search fuzzy` calls the exact same `search_typeahead` engine (whole-term prefix, then multi-word intersection, then a Levenshtein-tolerant fuzzy fallback when the cheaper passes found little) but wraps it as an ordinary one-shot command: `--format text|json|yaml`, `--ids` for piping, batch stdin (`-`), and a `--provenance` footer, matching the conventions every other `sct` query command follows.

```console
$ sct search fuzzy "asthm" --index snomed.fst
195967001           Asthma (disorder)

$ sct search fuzzy "asthma" --index snomed.fst --format json
{
  "results": [
    { "id": "195967001", "display": "Asthma", "score": 0.8, "tag": "disorder", "active": true }
  ]
}
```

The JSON shape matches `sct sayt --stdio` and `sct serve`'s `/autocomplete` exactly (`id` as a string, since SCTIDs exceed JavaScript's safe-integer range) - not `sct lexical`'s result shape, because the FST index genuinely knows different things about a concept. It has a preferred term and a semantic tag, but no FSN and no hierarchy name, so text output shows `<id>  <term> (<tag>)` rather than the fuller line `sct lexical`/`sct semantic` render from SQLite. A retired concept is flagged with the same `⚠ [INACTIVE]` marker every other search surface uses.

`sct fst search --fuzzy <N>` still exists as the lower-level primitive: a single Levenshtein pass at an explicit edit distance, with no prefix/word fallback and no `--format`/batch/provenance handling, useful for development and benchmarking (`benchmarks/fst_bench.rs`). `sct search fuzzy` sits above it the same way `sct lexical` sits above raw FTS5, or `sct sqlite` sits above hand-written SQL.

## `hybrid`: experimental weighted fusion

`sct search hybrid` runs the meaning-based, keyword and typo-tolerant searches and merges them with weighted reciprocal-rank fusion (RRF). Each concept earns `weight / (k + position)` points from every search that finds it, and the points are added up. A concept several searches agree on rises; one found by a single search stays lower.

```console
$ sct search hybrid "heart attack" --limit 3
22298006            Myocardial infarction  [dense #1, lexical #2, fuzzy #1]
```

Each line shows which searches found the concept and at what position, so a surprising result can be traced back to the search that surfaced it. `--format json` adds the same `source_ranks` per result and a `fusion` object recording the method, `k` and weights used.

The weights are the sliders:

| Flag | Default | Effect |
|---|---|---|
| `--dense-weight` | `1.0` | How much the meaning-based (Ollama) ranking counts. `0` leaves it out. |
| `--lexical-weight` | `1.0` | How much the keyword (FTS5) ranking counts. `0` leaves it out. |
| `--fuzzy-weight` | `1.0` | How much the typo-tolerant (FST) ranking counts. `0` leaves it out. |
| `--rrf-k` | `60` | Small `k` rewards top positions heavily; large `k` flattens the differences. |
| `--depth` | `100` | How many results each search contributes before fusing. |

**Experimental.** The defaults come from the `R62` comparison (67 guideline-derived queries, UK Monolith 43.0.0): fused search found the expected concept first about 31-34% of the time, against 18-25% for meaning-based search alone, and about a third of the queries were missed by every search. Moving `k` between 10 and 200 made no difference, and weights tuned on that corpus did not generalise, so equal weights are the default. Exact-term queries and the other embedding profiles have not been checked yet, so treat this as evidence-backed, not a clinical guarantee. See [`sct bench`](bench.md#compare-retrieval-strategies) to repeat the comparison on your own release, and [`spec/roadmap.md`](../../spec/roadmap.md) (`R14`/`R15`) for what remains.

**It does not quietly degrade.** The database, FST index and embeddings must come from the same release and content (checked by release identifier and fingerprint), and if Ollama is unreachable the command fails with a message saying so rather than returning keyword-only results. To run without the meaning-based part on purpose, pass `--dense-weight 0`; Ollama is then not contacted.

`--db`, `--embeddings` and `--index` use the usual discovery order; with no `--index`, the `.fst` beside the database with the same name is used first. Batch stdin (`-`) accepts up to 100 queries.

## See also

- [`sct lexical`](lexical.md) - keyword search, full option reference
- [`sct semantic`](semantic.md) - meaning-based search, full option reference
- [`sct sayt`](sayt.md) - the live search-as-you-type surfaces `fuzzy` shares its engine with
- [`sct fst`](fst.md) - build the FST index `fuzzy` queries, and the lower-level `fst search` primitive
