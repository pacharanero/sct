# sct search

**One front door onto every search strategy `sct` offers.** `sct` grew keyword, typo-tolerant, and meaning-based search as separate mature commands - `sct lexical`, `sct sayt --fuzzy`, `sct semantic` - each good at a different job, but with no single place that names the choice. `sct search <mode>` is that place.

**When to use:** you want to pick a search strategy explicitly, or you're not sure which of `lexical`/`semantic`/`fuzzy` fits your query. If you already know you want keyword search, `sct lexical` works exactly the same and is one word shorter.

---

## Usage

```
sct search lexical  <QUERY|-> [the same options as `sct lexical`]
sct search semantic <QUERY|-> [the same options as `sct semantic`]
sct search fuzzy    <QUERY|-> [--index <FILE>] [--limit <N>] [--format text|json|yaml] [--ids]
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

## `hybrid`: not yet

Roadmap `R14`/`R15` call for evaluating a weighted lexical/semantic/fuzzy combination against the guideline-derived regression corpus (`sct bench semantic`, 67 cases, `R62`) before picking a weighting scheme and shipping it. A `sct search hybrid` mode is planned once that evidence exists, not before - shipping an unmeasured ranking fusion under a mode name a user would reasonably trust would be exactly the kind of silent-degrade defect this project's own assurance programme exists to catch. See [`spec/roadmap.md`](../../spec/roadmap.md) for the current state of that work.

## See also

- [`sct lexical`](lexical.md) - keyword search, full option reference
- [`sct semantic`](semantic.md) - meaning-based search, full option reference
- [`sct sayt`](sayt.md) - the live search-as-you-type surfaces `fuzzy` shares its engine with
- [`sct fst`](fst.md) - build the FST index `fuzzy` queries, and the lower-level `fst search` primitive
