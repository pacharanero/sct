# sct bench `experimental!`

Benchmark `sct` on **your** machine against **your** database. `sct bench` times the operations you actually perform - concept lookup, lexical search, children, ancestors, subsumption, ECL expansion, and FST prefix search - through two boundaries: the in-process SDK and a full CLI subprocess. The gap between them is what you pay for process startup, which an in-process benchmark cannot see. It needs a database and nothing else: no repository clone, no container runtime, no external load generator, and no network access. Nothing is uploaded.

---

## Usage

```bash
sct bench [--db <PATH>] [--profiles <LIST>] [--full] [--pipeline <RF2>]
          [--samples <N>] [--warmup <N>] [--format <FMT>] [--output <PATH>]
          [--baseline <PATH>] [--no-provenance]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--db <PATH>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |
| `--profiles <LIST>` | `sdk,cli,artefact` | Comma-separated measurement profiles. See [Profiles](#profiles). |
| `--full` | *(flag)* | Longer run: more samples (30 after 5 warm-ups), plus deeper hierarchy and ECL cases. |
| `--pipeline <RF2>` | *(none)* | Also time a full build from this RF2 zip or directory: `sct ndjson`, then `sct sqlite`, then `sct fst build`, into a temporary directory that is removed afterwards. |
| `--samples <N>` | `10` (`30` with `--full`) | Per-case sample count. |
| `--warmup <N>` | `3` (`5` with `--full`) | Per-case warm-up count, run before and excluded from the samples. |
| `--format <FMT>` | `text` | `text`, `markdown`, `json`, or `html`. |
| `--output <PATH>` / `-o` | stdout | Write the report to a file. Required for `--format html` unless stdout is redirected. |
| `--baseline <PATH>` | *(none)* | Compare medians against a previous `--format json` result and show per-case deltas. |
| `--no-provenance` | *(flag)* | Withhold dataset release identity (edition, release date, release id). Concept count and schema version are still shown. |

---

## Profiles

| Profile | Measures | Boundary |
|---|---|---|
| `sdk` | Each operation through `Snomed`, on an already-open database | In-process, warm cache |
| `cli` | The same operation as a subprocess of the running binary: spawn, argument parsing, database open, query, output | Whole binary, per-invocation |
| `artefact` | Database, FST index, and embeddings file sizes; transitive-closure presence; schema version | Static inspection, not timed |

The `sdk`/`cli` pairing is the point of the exercise. The `startup cost` column is `cli` median minus `sdk` median.

Hierarchy operations reach the CLI through `sct ecl expand`, which is the CLI's expression of exactly those relations: `<!` for children, `>` for ancestors, and `<<left AND right` for subsumption (non-empty precisely when `left` subsumes `right`).

---

## Example

```text
sct bench 0.22.0

  Machine     Intel(R) Xeon(R) Processor @ 2.80GHz, 4 cores, 15.7 GB (linux/x86_64)
  Database    uk-monolith-42.db, 837,930 concepts, UK Monolith (2026-07-01), schema v6
  Artefacts   db 2.4 GB, fst 135.0 MB, tct present, embeddings absent

  Operation                 SDK (median)    CLI (median)    startup cost
  lookup by SCTID               0.090 ms        8.400 ms        8.310 ms
  lexical search "heart"        1.200 ms        9.600 ms        8.400 ms
  children                      0.210 ms        8.600 ms        8.390 ms
  ancestors                     0.340 ms        8.700 ms        8.360 ms
  subsumption test              0.050 ms        8.300 ms        8.250 ms
  ECL <<73211009               14.800 ms       23.400 ms        8.600 ms
  FST prefix "myoca"            0.030 ms        8.300 ms        8.270 ms

  10 samples per case after 3 warm-up runs; medians shown, p95 in --format json.
  Single run on an uncontrolled machine - treat as an order of magnitude.

  Share:  sct bench --format markdown | pbcopy
```

---

## Honest degradation

Cases are embedded in the binary and each one declares what it needs. A case naming a concept your edition does not contain, or an FST case with no matching index beside the database, is **skipped and reported as skipped** under `Not measured`. It never contributes a timing row, and it is never timed against a missing row:

```text
  Not measured
    lookup by SCTID     concept 22298006 is not present in this database
    FST prefix "myoca"  no FST index alongside this database (build one with `sct fst build`)
```

Every case is also run once outside the timed region before its samples are accepted, so an operation that fails cannot be reported as a fast one. Failed samples are excluded from the timings and surface as an `error_rate` in the JSON.

---

## Comparing two runs

```bash
sct bench --format json --output before.json
# ... change something ...
sct bench --baseline before.json
```

```text
  Baseline comparison (noise band ±15%)
    lookup by SCTID         sdk       0.181 ms →     0.131 ms    -28.0%  faster
    lexical search "heart"  sdk       0.160 ms →     0.178 ms    +11.2%  noise
```

Deltas inside ±15% are labelled `noise`. A single run on an uncontrolled machine cannot distinguish a 4% change from the weather, so `sct bench` does not dress one up as a regression.

---

## Sharing a result

`--format markdown` produces something that pastes into a GitHub issue or a Discourse post without manual fixing:

```bash
sct bench --format markdown | pbcopy     # macOS
sct bench --format markdown | wl-copy    # Wayland
sct bench --format markdown | clip       # Windows
```

`--format html` writes one self-contained file - inline CSS, no scripts, no fonts, no images, and therefore no network requests:

```bash
sct bench --format html --output bench.html
```

`--format json` is the canonical form. It carries the schema version, run metadata, host, dataset provenance, sampling policy, and **raw per-sample timings** alongside the summaries, so medians and percentiles can be recomputed and challenged rather than taken on trust. It follows the shared result model in `spec/benchmark-runner.md`, so a result can be ingested later as a labelled target.

---

## Privacy

No output in any format contains an absolute filesystem path, a hostname, a username, or a credential. The database and any RF2 input are identified by **file name** only. Sample failures are recorded as fixed classes (`nonzero_exit`, `sdk_error`) rather than error messages, which could embed a path.

`--no-provenance` additionally withholds the release identity - edition, release date, and release id - for users who consider their edition licensing sensitive. Concept count, schema version, and machine details are still shown; they identify no release and no person.

There is no telemetry, no submission endpoint, and no network access in any profile.

---

## Timing a full build

```bash
sct bench --pipeline ~/releases/SnomedCT_InternationalRF2_PRODUCTION_20260301T120000Z
```

Each stage runs once - a build takes minutes on a real release, and the interesting number is the wall clock you would actually wait, not its distribution. Output goes to a temporary directory which is removed when the run finishes. A stage that fails stops the pipeline and is reported rather than timed as a success.

---

## Notes and limitations

- The default run targets well under 30 seconds on a modest machine. Build steps are never in the default run: `--pipeline` is opt-in.
- Results are a single run on an uncontrolled machine. Treat them as an order of magnitude, not a benchmark-grade figure.
- Comparing `sct` against another terminology server is deliberately out of scope; that belongs to the separate, non-shipped comparative runner.

## See also

- [`sct info`](info.md) - inspect an artefact without timing it.
- [`sct size`](size.md) - estimate export sizes for a subtree.
- [Benchmarks](../benchmarks.md) - published figures from several machines.
