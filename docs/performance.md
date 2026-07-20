# Performance and tuning

`sct` is fast by construction, not by configuration: a single static binary, a read-only SQLite database memory-mapped into the process, and a handful of precomputed indexes. For most people there is nothing to tune - a concept lookup on the full UK Monolith is sub-millisecond once the cache is warm, and the server's resident footprint is ~26 MB. This page explains *why* it is fast (so you can reason about it) and how to tune `sct serve` when you have a specific load in mind.

!!! note "The short version"
    Build the transitive closure table (`sct tct`), give the machine enough RAM for the OS page cache to hold your database's hot pages, and - for the server - leave `--pool-size` on its default unless a benchmark tells you otherwise.

## What makes it fast

### The database is memory-mapped, not read page-by-page

Every read command opens the SQLite database read-only and sets `PRAGMA mmap_size`, so query reads come straight from the memory-mapped file. On a warm cache a query is reading RAM: there is no `read()` syscall and no per-connection buffer copy for resident pages. The first touch of a page faults it in from disk; everything after that is at memory speed.

This is the key mental model: **`sct` is already an in-memory query engine for warm data.** Loading the whole database into a `:memory:` database would not make warm queries meaningfully faster (the B-tree query algorithm is identical) and would *hurt* the run-and-exit CLI, which only ever pages in the handful of pages a single lookup touches.

!!! warning "The mmap window is ~2 GiB"
    SQLite clamps `mmap_size` to its compile-time maximum (`SQLITE_MAX_MMAP_SIZE`, ~2 GiB in the bundled build). The **UK Monolith database is 3-4 GB**, so roughly 1-1.5 GB of it sits outside the mmap window and is served by ordinary buffered I/O instead. That tail is still cached by the OS page cache, so with enough RAM the practical impact is small - but it is why raising the `mmap_size` pragma alone changes nothing (see roadmap `R81`). If you are memory-constrained, the UK **Clinical** edition (~1.5 GB) maps in full.

### Precomputed transitive closure (the biggest single lever)

Subsumption - "is X a kind of Y?", "all descendants of Y", ECL `<<` / `>>` / `^`, FHIR `$expand` - is answered from the `concept_ancestors` transitive closure table (TCT) when it exists: an indexed lookup instead of a recursive graph walk. Without it, `sct` falls back to a recursive Common Table Expression that re-walks the IS-A graph on every query, which is dramatically slower for large hierarchies (and `sct` prints a one-line stderr nudge when it notices the table is missing).

The TCT uses INTEGER identifier columns (SCTIDs are numeric), which makes it ~35% smaller and its index builds faster than the equivalent text table.

!!! tip "Build the TCT once"
    ```bash
    sct tct --db snomed.db          # add it to an existing database
    sct sqlite --transitive-closure # or build it during the SQLite step
    ```
    This is the single highest-value thing you can do for subsumption-heavy workloads (`$expand`, `sct size`, large ECL). It costs a one-time build and some disk; every subsequent query benefits.

### Purpose-built indexes for the other access patterns

- **Lexical search** (`sct lexical`, FHIR text filters) uses SQLite's FTS5 full-text index.
- **Search-as-you-type** (`sct sayt`, `sct serve /autocomplete`) uses a separate, memory-mapped [FST index](commands/fst.md) - the whole file is mapped once and prefix queries are sub-millisecond.
- **Semantic search** (`sct semantic`) reads an Arrow embeddings file and ranks by cosine similarity.

Each is derived from the canonical NDJSON and is optional - you only build (and pay for) the ones you use.

## Cold vs warm

The first query after opening a database pays for page faults as the working set is faulted in; everything after that is at RAM speed. A long-running `sct serve` warms up over its first few requests and stays warm. For a one-shot CLI invocation on a cold cache you can pre-warm the file if it matters:

```bash
cat snomed.db > /dev/null     # pull the file into the OS page cache
# or, more precisely, with the vmtouch tool:
vmtouch -t snomed.db
```

In practice, on any machine that runs `sct` more than occasionally, the database is already resident and this is unnecessary.

## Tuning `sct serve`

The server opens a **warm pool of read-only connections** that all share the one memory-mapped database (so per-connection private cache is deliberately small - 8 MiB - because the big win is the shared mmap, not per-connection buffers).

### `--pool-size`

```bash
sct serve --pool-size 16
```

Left at its default (`0`), the pool is sized to **2× the CPU cores, clamped to 4-64**. That is a good default for mixed read workloads. Raise it if you have many concurrent clients and spare cores; there is rarely a reason to lower it. The pool bounds concurrency, not per-request work, so it does not protect against a single very expensive request (see the notes per load type below).

### By load type

=== "High-concurrency lookup / validate"

    Small, uniform requests (`CodeSystem/$lookup`, `$validate-code`, `$subsumes`). These are the cheapest operations and scale with the pool.

    - Ensure the **TCT is built** so `$subsumes` is an indexed lookup.
    - Size `--pool-size` to your core count and client concurrency; measure with `benchmarks/load.sh`.

=== "Autocomplete (search-as-you-type)"

    The `/autocomplete` endpoint needs an [FST index](commands/fst.md). Supply it explicitly or place a `snomed.fst` next to the database and it is picked up automatically:

    ```bash
    sct serve --fst snomed.fst
    ```

    The FST is memory-mapped and shared across the pool, so this endpoint is very cheap per request.

=== "Large ECL / \$expand"

    Expansion is the operation most sensitive to indexing and to result size.

    - The **TCT is essential** here - a `<<` expansion without it re-walks the hierarchy on every call.
    - Use `count` / `offset` paging; a very large `count` materialises the whole result set into one response (bounding this is tracked as roadmap `R72`).

=== "Batch Bundles"

    A `POST /` batch Bundle runs its entries **sequentially on one pooled connection**, so a bundle with very many entries ties up a connection for its duration (entry-count bounding is tracked as `R73`). Prefer several smaller bundles over one enormous one, and keep client-side concurrency in mind.

=== "Memory-constrained"

    Resident memory is dominated by the OS page cache holding the database's hot pages, not by `sct` itself (~26 MB base). To shrink the footprint, prefer the UK **Clinical** edition over the full **Monolith**, and expose only the artefacts you need (skip the FST/embeddings if you do not serve autocomplete/semantic).

### Binding and exposure

`sct serve` binds `127.0.0.1` by default. If you set `--host 0.0.0.0` to expose it, put it behind the reverse proxy in the [deployment stack](deploy/terminology-server.md) - `sct serve` deliberately has no auth, rate limiting, or request timeout of its own (the localhost default is the security boundary).

## Build-time performance

The build writes the canonical NDJSON (RF2 → NDJSON) and then reads it back (NDJSON → SQLite); that intermediate write + read is the largest single build cost and is I/O-bound. This is a deliberate trade for the file-first, inspectable, distributable NDJSON artefact. A fused one-pass build is a tracked opt-in idea (roadmap `R32`); the TCT build is a separate one-time cost you opt into with `--transitive-closure` / `sct tct`.

## Measuring

- **[Benchmarks](benchmarks.md)** - methodology and published `sct`-solo figures, plus the `sct serve` vs Snowstorm Lite comparison; `benchmarks/bench.sh` and `benchmarks/load.sh` are the runners.
- **`sct size`** - estimate the on-disk / export size of a subtree before you build or filter.
- **`sct info <db>`** - concept counts, index presence (including whether the TCT exists), and per-hierarchy breakdown for a built database.

## See also

- [`sct serve`](commands/serve.md) - the terminology server and its full flag set
- [Deploy a terminology server](deploy/terminology-server.md) - the reverse-proxy stack for public exposure
- [Benchmarks](benchmarks.md) and [FHIR conformance](fhir-conformance-benchmarks.md)
