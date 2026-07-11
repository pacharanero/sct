# sct sayt

**Search-as-you-type over SNOMED CT** - instant, offline, typo-tolerant autocomplete over 800k+ concepts, backed by the mmap'd [FST index](fst.md). Sub-millisecond per keystroke, no server, no Elasticsearch, no Java.

It comes in **three surfaces that share one engine**, so results are identical whichever you use:

1. **An interactive terminal UI** - the live demo. Type and watch results appear as you go.
2. **A stdio line protocol** (`--stdio`) - embed `sct` as a local search backend in a desktop or editor app.
3. **An HTTP endpoint** (`sct serve`'s `/autocomplete`) - drop-in autocomplete for a web front-end.

All three call the same `search_typeahead` core, which blends whole-term prefix matching with multi-word intersection and an optional typo-tolerant fuzzy pass.

---

## Prerequisites

Build an FST index once from your NDJSON artefact (see [`sct fst`](fst.md)):

```bash
sct fst build --ndjson snomed.ndjson --output snomed.fst
```

---

## 1. Interactive terminal UI

```bash
sct sayt --index snomed.fst
```

A full-screen search box: results repaint on **every keystroke**, with a live latency readout (typically well under a millisecond), the loaded edition, and the matched concept's semantic tag. `↑`/`↓` select, **Enter** emits the selected `SCTID⇥term` to stdout (so you can pipe it), `Esc` quits.

```
sct sayt | cut -f1 | sct codelist add asthma.codelist -
```

!!! note "In the default build"
    The interactive UI is part of the default feature set, so it ships in the released binaries and in `cargo install sct-rs`. It is only absent from `--no-default-features` builds (such as the Docker server image, which has no terminal) - the `--stdio` protocol below works in any build.

---

## 2. Stdio line protocol (embed as a search backend)

```bash
sct sayt --stdio --index snomed.fst
```

One query per line on **stdin**; one line of JSON per query on **stdout**, flushed immediately:

```console
$ printf 'myoc\ntype 2 diab\n' | sct sayt --stdio --index snomed.fst --limit 3
{"query":"myoc","hits":[{"id":"9516401000001103","display":"Myocet","score":0.788,"tag":"product"}, ...]}
{"query":"type 2 diab","hits":[{"id":"44054006","display":"Type 2 diabetes mellitus","score":0.8,"tag":"disorder"}, ...]}
```

Each hit is `{"id", "display", "score", "tag"}`. **`id` is a string** because SCTIDs exceed JavaScript's safe-integer range (2^53) - a JSON number would silently lose precision. Queries are processed in order and each is sub-millisecond, so a consumer simply reads the latest line.

This is the shape a native app (Tauri, Electron, a Vim/VS Code plugin) drives as a child process for local, offline autocomplete.

---

## 3. HTTP autocomplete endpoint

Start [`sct serve`](serve.md) with an FST index (auto-discovered as `snomed.fst` next to the database, or set explicitly with `--fst`):

```bash
sct serve --db snomed.db --fst snomed.fst
```

Then a browser front-end hits it per keystroke (debounced):

```console
$ curl 'http://localhost:8080/autocomplete?q=myocard&count=5'
{"query":"myocard","hits":[{"id":"22298006","display":"Myocardial infarction","score":0.77,"tag":"disorder"}, ...]}
```

`q` is the partial query; `count` (default 10, max 100) caps the results. Same JSON shape as the stdio protocol. If the server was started without an FST index, `/autocomplete` returns `501` with a message telling you to supply `--fst`. This is the drop-in **"autocomplete for 800k SNOMED concepts, sub-ms, offline, no Elasticsearch"** for web apps.

---

## Options (`sct sayt`)

| Flag | Default | Description |
|---|---|---|
| `--index <FILE>` | `snomed.fst` | FST index produced by `sct fst build`. |
| `-l, --limit <N>` | `10` | Maximum results shown / returned. |
| `--min-chars <N>` | `1` | Minimum query length before results are computed. |
| `--fuzzy` | off | Enable typo-tolerant fuzzy fallback (broader, still sub-ms). |
| `--stdio` | off | Machine mode: the stdin→stdout JSON line protocol above (no TUI; any build). |

---

## How the search works

`search_typeahead` runs up to three sub-millisecond FST passes and merges them (dedupe by concept, keep the best score, rank, truncate):

- **Whole-term prefix** - the primary signal: terms starting with the query (`myoc` → *Myocardial infarction*, and matches on any of a concept's synonyms - `hypert` finds *Increased muscle tone* via its synonym *Hypertonia*).
- **Multi-word intersection** - for queries of more than one word: terms containing every whole word, in any order (`type 2 diab` → *Type 2 diabetes mellitus*).
- **Fuzzy fallback** (`--fuzzy`, and on the HTTP endpoint) - Levenshtein-tolerant matches for typos (`asthmaa` → *Asthma*), used only when the cheaper passes found little.

Relevance ordering is a length-proximity heuristic (shorter, more query-covering completions first); frequency-weighted clinical relevance ranking is a future refinement.

## See also

- [`sct fst`](fst.md) - build the index and run one-shot exact/prefix/fuzzy/word queries
- [`sct serve`](serve.md) - the FHIR R4 server that also hosts `/autocomplete`
- [`sct lexical`](lexical.md) - FTS5 keyword search over the SQLite database
