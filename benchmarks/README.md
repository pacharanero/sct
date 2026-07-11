# bench

automated benchmarking suite for `sct` - compares local SQLite performance
against any FHIR R4 terminology server across six operations.

## Requirements

| tool | required | install |
|:---|:---|:---|
| `bash` ≥ 4.0 | yes | system |
| `curl` | yes (remote) | system |
| `sqlite3` | yes | system |
| `jq` | yes | `apt install jq` / `brew install jq` |
| `hyperfine` | recommended | `cargo install hyperfine` |

without `hyperfine`, timing falls back to `date +%s%N` (linux only; less accurate).

## Quick start

```bash
# conformance first: prove the FHIR server returns the expected shapes/results
benchmarks/conformance.sh --server http://localhost:8080/fhir

# local-only benchmark (no remote server required)
benchmarks/bench.sh --db snomed.db --no-remote

# compare against a FHIR terminology server
benchmarks/bench.sh --db snomed.db --server https://terminology.myserver.org/fhir

# write results to benchmarks.md in the project root
benchmarks/bench.sh --db snomed.db --server https://terminology.myserver.org/fhir \
  --write-benchmarks
```

## Conformance before benchmarks

`benchmarks/conformance.sh` checks a FHIR R4 terminology server before timing it.
It uses fixture matrices under `benchmarks/fixtures/conformance/` to assert:

- `/metadata` advertises the expected FHIR R4 terminology operations
- `CodeSystem/$lookup` returns `Parameters` with display and hierarchy
- `CodeSystem/$validate-code` handles known, unknown and display-mismatch cases
- `ValueSet/$expand` handles ECL, text filtering and expected members
- `CodeSystem/$subsumes` returns the four expected relationship outcomes
- `ValueSet/$validate-code` tests ECL membership
- `ConceptMap/$translate` works when the server advertises it
- invalid requests return FHIR `OperationOutcome`

### CI regression gate

`benchmarks/conformance-ci.sh` runs this suite on every push: it builds the
committed synthetic RF2 fixture (`tests/fixtures/rf2/`), starts `sct serve` over
it, and asserts a minimal fixture set matched to that 22-concept fixture
(`benchmarks/fixtures/conformance-ci/`). It exits non-zero on any failure, so a
conformance regression breaks the build. Run it locally too:

```bash
cargo build --bin sct && benchmarks/conformance-ci.sh
```

Run the full suite against any candidate server before using the benchmark numbers:

```bash
benchmarks/conformance.sh --server http://localhost:8080/fhir
benchmarks/conformance.sh --server http://localhost:8080/fhir --output reports/conformance.jsonl
```

The runner is HL7-aligned because it exercises the FHIR R4 terminology
operations, but it is not an official HL7 certification suite. A future
Touchstone/FHIR `TestScript` suite would complement it.

## Two comparison modes

The **sct side** is either its native SQLite path or sct serve over FHIR; the
**comparator** is always a FHIR server. This gives two comparisons:

```bash
# sct native (local SQLite) vs a FHIR server
benchmarks/bench.sh --sct-sqlite snomed.db --vs "$(s/snowstorm-lite url)" --write-benchmarks

# sct serve vs a FHIR server - like-for-like, FHIR to FHIR
sct serve --db snomed.db --port 8081 --fhir-base /fhir &
benchmarks/bench.sh --sct-fhir http://localhost:8081/fhir --vs "$(s/snowstorm-lite url)" --write-benchmarks
```

## Options

```
--sct-sqlite PATH   sct's native SQLite path (default: ./snomed.db; alias --db)
--sct-fhir URL      sct serve, over FHIR (e.g. http://localhost:8081/fhir)
--vs URL            comparator FHIR server (alias --server)
--runs N            timed iterations per operation (default: 5)
--warmup N          warmup iterations before timing (default: 1)
--operations LIST   comma-separated subset to run:
                    lookup,search,children,ancestors,subsumption,bulk
                    (default: all six)
--format FORMAT     table (default) | chart | json | csv
--no-remote         benchmark local operations only
--timeout SECS      per-request curl timeout (default: 30)
--output FILE       write report to FILE in addition to stdout
--write-benchmarks  write a timestamped report (bar chart + table) to benchmarks/reports/
```

## Operations

| operation | local implementation | fhir equivalent |
|:---|:---|:---|
| concept lookup | `SELECT … WHERE id = ?` | `CodeSystem/$lookup` |
| text search | FTS5 `MATCH` | `ValueSet/$expand?filter=` |
| direct children | `JOIN concept_isa WHERE parent_id = ?` | `ValueSet/$expand` with ECL `<!SCTID` |
| ancestor chain | recursive CTE (all hops, one query) | sequential `$lookup?property=parent` calls |
| subsumption test | CTE ancestor check | `CodeSystem/$subsumes` |
| bulk lookup (15) | `WHERE id IN (…)` (single query) | batch bundle or sequential `$lookup` |

## Fairness

- **conformance first**: do not publish timing comparisons for a server that
  fails the relevant fixture profile.
- **local times include sqlite3 process startup** (~5–15 ms). this reflects
  real cli usage, not in-process query time.
- **remote warm-up runs** are issued before timing to ensure both sides are
  in a hot-cache state.
- **ancestor chain** on the fhir side performs sequential `$lookup` calls
  (one per hop), matching the actual cost a fhir client would incur. the
  local side resolves the full chain in a single recursive CTE.
- **ping latency** to the remote server is measured and reported so readers
  can distinguish server latency from network latency.

## Adding operations

create `benchmarks/operations/myop.sh` that defines `run_myop()` and calls
`append_result`. then pass `--operations myop` or include it in the default
list in `bench.sh`.

## Adding conformance cases

Add rows to the TSV files in `benchmarks/fixtures/conformance/`. Keep them
stratified: common concepts, deep hierarchy concepts, high-fanout parents,
invalid codes, ECL expressions, refsets and cross-terminology mappings. The
goal is a production-shaped request matrix, not a handful of easy examples.

## Run a local Snowstorm Lite comparator

Snowstorm Lite is SNOMED International's FHIR-native, single-container terminology
server - the apt lightweight comparator for `sct serve`. The one-command wrapper
[`s/snowstorm-lite`](../s/snowstorm-lite) handles the Docker run, health-wait, and
release load (allocate at least ~20 GB RAM to Docker; the default Java heap is
`-Xmx16g`):

```bash
s/snowstorm-lite up                    # pull + start, wait for health

# Load the Clinical Edition (or the full UK Monolith if you can get it to work!)
s/snowstorm-lite load uk_sct2cl_41.6.0_20260311000001Z.zip \
  --version-uri "http://snomed.info/sct/83821000000107/version/20260311"

# Prove correctness first, then benchmark
benchmarks/conformance.sh --server "$(s/snowstorm-lite url)"
benchmarks/bench.sh --db snomed.db --server "$(s/snowstorm-lite url)" --write-benchmarks

s/snowstorm-lite down                  # stop when finished (keeps the loaded data)
```

Run `s/snowstorm-lite --help` for all subcommands (`status`, `logs`, `--heap`, `--port`).

## Load testing (`load.sh`)

Where `bench.sh` measures *single-request* latency one operation at a time,
`benchmarks/load.sh` drives `sct serve` under **sustained concurrency** and
reports how it behaves as load climbs: throughput (req/s), tail latency
(p50/p95/p99/p99.9), the error rate, and where it saturates. It is a
**single-server** test - no comparator - so the numbers are freely publishable.

It uses [`oha`](https://github.com/hatoo/oha) (keep-alive, JSON output) by
default, falling back to `bombardier`:

```bash
cargo install oha    # or download a release binary

# ramp concurrency 1..128, 10s per level, against a running sct serve
benchmarks/load.sh --url http://localhost:8080/fhir

# a quick subset
benchmarks/load.sh --url http://localhost:8080/fhir \
  --concurrencies 1,4,16,64 --duration 5s --operations lookup,expand
```

Each operation prints a concurrency-vs-throughput/latency table and the peak
(`req/s @ N clients`). `--write-report` saves a markdown report to
`benchmarks/reports/` (gitignored). `--stat-container <name>` samples
`docker stats` mid-run to record the server's memory under load (host runs
only). Run `benchmarks/load.sh --help` for all options.

**Running it against a Dockerised `sct serve`** whose port isn't published to
the host: run `load.sh` from a throwaway container on the same Docker network,
exactly like the container recipe works for `bench.sh` - install `oha` in the
container and point `--url` at the service name (`http://sct:8080/fhir`). Watch
memory with `docker stats sct` in another terminal on the host.

## Notes

- `date +%s%N` requires linux (GNU coreutils). on macOS, install
  `gdate` via `brew install coreutils` and symlink it, or use hyperfine.
- the fhir ancestor traversal can be slow for deep concepts (~8–12 hops at
  200–400 ms per hop = 2–5 seconds per timed run). with 5 runs and 1 warmup
  this operation may take 30–60 seconds against a remote server.
