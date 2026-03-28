# bench

automated benchmarking suite for `sct` — compares local SQLite performance
against any FHIR R4 terminology server across six operations.

## requirements

| tool | required | install |
|:---|:---|:---|
| `bash` ≥ 4.0 | yes | system |
| `curl` | yes (remote) | system |
| `sqlite3` | yes | system |
| `jq` | yes | `apt install jq` / `brew install jq` |
| `hyperfine` | recommended | `cargo install hyperfine` |

without `hyperfine`, timing falls back to `date +%s%N` (linux only; less accurate).

## quick start

```bash
# local-only benchmark (no remote server required)
bench/bench.sh --db snomed.db --no-remote

# compare against a FHIR terminology server
bench/bench.sh --db snomed.db --server https://terminology.openehr.org/fhir

# write results to benchmarks.md in the project root
bench/bench.sh --db snomed.db --server https://terminology.openehr.org/fhir \
  --write-benchmarks
```

## options

```
--server URL        FHIR base URL (required for remote comparison)
--db PATH           path to snomed.db (default: ./snomed.db)
--runs N            timed iterations per operation (default: 5)
--warmup N          warmup iterations before timing (default: 1)
--operations LIST   comma-separated subset to run:
                    lookup,search,children,ancestors,subsumption,bulk
                    (default: all six)
--format FORMAT     table (default) | json | csv
--no-remote         benchmark local operations only
--timeout SECS      per-request curl timeout (default: 30)
--output FILE       write report to FILE in addition to stdout
--write-benchmarks  write results to ./benchmarks.md
```

## operations

| operation | local implementation | fhir equivalent |
|:---|:---|:---|
| concept lookup | `SELECT … WHERE id = ?` | `CodeSystem/$lookup` |
| text search | FTS5 `MATCH` | `ValueSet/$expand?filter=` |
| direct children | `JOIN concept_isa WHERE parent_id = ?` | `ValueSet/$expand` with ECL `<!SCTID` |
| ancestor chain | recursive CTE (all hops, one query) | sequential `$lookup?property=parent` calls |
| subsumption test | CTE ancestor check | `CodeSystem/$subsumes` |
| bulk lookup (15) | `WHERE id IN (…)` (single query) | batch bundle or sequential `$lookup` |

## fairness

- **local times include sqlite3 process startup** (~5–15 ms). this reflects
  real cli usage, not in-process query time.
- **remote warm-up runs** are issued before timing to ensure both sides are
  in a hot-cache state.
- **ancestor chain** on the fhir side performs sequential `$lookup` calls
  (one per hop), matching the actual cost a fhir client would incur. the
  local side resolves the full chain in a single recursive CTE.
- **ping latency** to the remote server is measured and reported so readers
  can distinguish server latency from network latency.

## adding operations

create `bench/operations/myop.sh` that defines `run_myop()` and calls
`append_result`. then pass `--operations myop` or include it in the default
list in `bench.sh`.

## notes

- `date +%s%N` requires linux (GNU coreutils). on macOS, install
  `gdate` via `brew install coreutils` and symlink it, or use hyperfine.
- the fhir ancestor traversal can be slow for deep concepts (~8–12 hops at
  200–400 ms per hop = 2–5 seconds per timed run). with 5 runs and 1 warmup
  this operation may take 30–60 seconds against a remote server.
