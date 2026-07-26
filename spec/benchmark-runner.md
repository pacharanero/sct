# Typed benchmark runner

Status: Proposed. Programme roadmap item: `R20`. Delivery stages: `R48` through `R51`. The current Bash implementation remains the working baseline until each replacement path reaches result and report parity.

## Decision summary

- `sct` will gain one non-shipped Rust benchmark runner that owns scenario selection, target configuration, execution policy, result capture, and report generation.
- The suite will share scenarios and a canonical result schema, not force every measurement through one timing engine.
- Criterion remains the engine for in-process Rust microbenchmarks. The runner measures SDK and CLI boundaries, single-request FHIR latency, and comparative terminology-server behaviour. `oha` remains the load generator for concurrent HTTP tests.
- FHIR conformance remains logically separate from performance measurement, but conformance and timing consume the same declarative scenario corpus where their inputs overlap.
- The runner records raw samples and complete run metadata as versioned JSON. Tables, CSV, Markdown, and charts are derived views that can be regenerated without rerunning a benchmark.
- Requested targets fail closed. An unavailable comparator, failed command, invalid response, or unmet capability cannot silently become a successful local-only result.
- Comparisons are labelled by boundary. SDK, CLI, raw SQLite, and FHIR measurements may appear in one report, but only equivalent interfaces receive a headline speed ratio.
- Existing Bash entry points are removed only after fixture, behaviour, failure, and report parity. Shell remains appropriate for thin wrappers around Docker, `perf`, flamegraph, callgrind, and external load tools.

## Context

The current suite under `benchmarks/` has proved the value of a conformance-first benchmark workflow. It can compare native SQLite with a FHIR server, compare `sct serve` with another FHIR server, run a synthetic-fixture conformance gate in CI, drive concurrent HTTP load, profile whole-binary queries, and render useful reports. Bash enabled that surface to evolve quickly and the existing suite remains the behavioural reference during migration.

The suite has now outgrown shell as its main application language. Operation definitions, timing, mutable state, temporary TSV files, capability handling, reporting, and cleanup are spread across sourced scripts and global variables. The manual timer can hide command failures, a missing comparator can silently downgrade a requested comparison to local-only, and fixture documentation has drifted from the cases actually timed. `load.sh` advertises two load generators but hard-codes `oha` for warm-up. The native side invokes `sqlite3` directly, so it measures schema/query behaviour and process startup rather than the Rust SDK or `sct` CLI.

The repository already has the right building blocks for a typed replacement: a public Rust SDK, Criterion microbenchmarks for FST and ECL parsing, synthetic RF2 fixtures, FHIR conformance cases, structured output conventions, and a real-server load runner. The migration should connect these pieces rather than replace established tools merely to make the implementation uniformly Rust.

## Goals

1. Detect regressions in the `sct` engine, SDK, CLI, and FHIR server at the boundary where each regression matters.
2. Compare `sct serve` fairly with any FHIR R4 terminology server that passes the relevant conformance profile.
3. Make every published number reproducible from a scenario definition, raw result file, dataset identity, target identity, and environment record.
4. Run the core suite on Linux, macOS, and Windows without Bash, GNU `date`, `jq`, `awk`, or the `sqlite3` command-line program.
5. Keep real SNOMED CT content local and user-supplied. Committed and CI runs use only the synthetic fixture.
6. Preserve the current public benchmark policy: publish `sct`-solo results and same-machine comparisons with the fully owned Snowstorm Lite comparator; keep commercial-server measurements private.
7. Make adding a scenario a data change in the common case, not a new script with duplicated timing and reporting logic.

## Non-goals

- The runner is not a public `sct` subcommand and is not included in release artefacts, crates.io packages, Python wheels, or container images.
- Criterion will not be used for remote HTTP latency or concurrent load merely to present one framework name.
- The project will not implement its own high-throughput HTTP load generator. A specialised tool such as `oha` is more credible and easier to validate.
- SDK-to-remote-HTTP ratios will not be presented as like-for-like server comparisons. They answer different questions.
- The home-grown FHIR scenarios will not be described as official HL7 certification. `R17` remains the externally verified conformance programme.
- Performance numbers will not initially become CI pass/fail thresholds. Synthetic CI runs prove functionality and result-schema stability; statistically meaningful regression thresholds require controlled hardware and a stored baseline policy.
- Migration will not delete the existing suite in one rewrite.

## Architecture

### One command, several measurement engines

The implementation target is a non-shipped crate under `benchmarks/runner/`, invoked through a tiny `s/bench` wrapper. It imports `sct-rs` through a local path for SDK measurements and uses the same lockfile discipline as the existing Python subcrate. Its command surface should remain explicit about the type of evidence being collected:

```text
s/bench conformance --target sct=http://localhost:8080/fhir
s/bench internal --db snomed.db --profiles sdk,cli
s/bench latency --target sct=http://localhost:8080/fhir --target snowstorm=http://localhost:8081/fhir
s/bench load --target sct=http://localhost:8080/fhir --concurrency 1,2,4,8,16,32,64,128
s/bench report benchmarks/runs/<run-id>/results.json --format markdown
```

The exact flags are an implementation contract for `R48`, but the separation between conformance, internal latency, FHIR latency, load, and rendering is architectural. A run may select several profiles, while each result retains its profile and target boundary.

### Measurement profiles

| Profile | Question | Target boundary | Measurement engine |
|---|---|---|---|
| `micro` | Did a pure or in-process hot path regress? | Rust function or SDK call | Criterion |
| `sdk` | How fast is the reusable `sct-rs` API against a real database/index? | In-process SDK | Runner monotonic clock |
| `cli` | What does a user pay for process startup, argument parsing, database opening, query, and output? | Release `sct` subprocess | Runner monotonic clock |
| `sqlite-diagnostic` | Is a regression in schema/query behaviour below the Rust adapter? | Direct SQLite connection | Runner monotonic clock; diagnostic, not branded as the `sct` product result |
| `fhir-latency` | What is steady-state single-request latency through the real HTTP interface? | `sct serve` or comparator FHIR endpoint | Runner HTTP client |
| `fhir-load` | Where does a server saturate and what happens to tail latency and errors? | One FHIR endpoint at a time | `oha`, orchestrated and parsed by the runner |
| `conformance` | Is the target semantically eligible for the requested benchmark cases? | FHIR endpoint | Runner HTTP client and assertions; not timed evidence |

Microbenchmarks remain ordinary `cargo bench` targets under `benchmarks/`. The first internal additions should exercise typed SDK lookup, lexical search, hierarchy traversal, subsumption, ECL expansion, and FST queries over the synthetic fixture, with an opt-in real database/index for publication-quality local runs.

### Targets

A target has a stable label, kind, endpoint or artefact path, version information, and capabilities discovered before measurement:

```text
SctSdk             database and optional FST/embedding artefacts
SctCli             binary plus database and optional derived artefacts
SqliteDiagnostic   database only
Fhir               label, base URL, optional non-secret headers, software/version metadata
```

The runner accepts more than two targets. Reports should not encode a permanent `local` versus `remote` pair because useful runs include `sct-sdk`, `sct-cli`, `sct-fhir`, and one or more FHIR comparators. Credentials may be supplied through environment variables or ignored local configuration, but are never written to scenarios, result files, reports, command lines, or logs.

Target readiness is explicit. The runner resolves paths, checks artefact provenance, probes endpoint metadata, records capability support, and fails before warm-up if a requested target cannot run the selected scenarios. `--allow-skip-unsupported` may skip a capability-gated scenario, but connectivity and execution failures are never silently converted into skips.

### Scenarios

Scenarios are declarative, reviewable data under `benchmarks/scenarios/`. TOML is the preferred authoring format because it supports comments, typed fields, and readable diffs; the runner deserialises it into a versioned scenario model. Existing TSV fixtures remain valid during migration and can be converted after the vertical slice proves the model.

Each scenario records:

- Stable scenario id and human label.
- Operation such as lookup, lexical search, children, ancestors, subsumption, ECL expansion, validation, translation, or bulk lookup.
- Input values and output limits.
- Profiles in which the scenario participates.
- Required target capabilities.
- Semantic expectations used outside the timed region: status/resource type, expected code/display, membership, minimum count, or expected error class.
- Dataset requirements such as known concept ids, edition, refsets, maps, or minimum schema version.
- Timing defaults such as warm-up count, sample count, timeout, and whether cold-cache measurement is meaningful.
- Tags for common, deep-hierarchy, high-fanout, negative, error, bulk, or specialist cases.

The same case can prove correctness and then provide latency evidence, but validation is never performed inside the measured interval unless response parsing is intentionally part of the target boundary. Every timed response is validated at least once before samples are accepted.

### Result model

The canonical result is versioned JSON written under a gitignored run directory. It stores raw evidence rather than only rendered aggregates:

```text
schema_version
run_id, started_at, git_commit, sct_version
host: os, architecture, cpu, logical_cores, memory, power/profile notes
dataset: edition, release_date, release_id, schema_version, concept_count, artefact sizes
topology: in_process, same_host, same_lan, or remote; client and server host labels
target: label, kind, endpoint without credentials, software/version, capabilities
policy: profile, warmup, samples, timeout, cache mode, concurrency, load duration
case: scenario_id, operation, input summary
samples: elapsed_ns, success, status, response_bytes, error_class
summary: median, mean, standard deviation, min, max, p50, p95, p99, error rate
```

The renderer consumes this file to produce text, JSON, CSV, Markdown, and charts. Summary algorithms and units live in one implementation and are covered by deterministic tests. Reports identify unavailable or skipped results directly rather than encoding them as `-` strings in numeric columns.

Run directories may also contain environment text, conformance results, `oha` JSON, logs, and rendered reports. The JSON result remains canonical; every other file is supporting evidence or a derived view.

## Fairness and publication rules

1. Conformance first: a FHIR target must pass every semantic case relevant to a reported timing. Unsupported optional capabilities are identified before timing and shown as unavailable, not failed performance results.
2. Compare equivalent boundaries: headline FHIR ratios compare the same HTTP method, operation, parameters, response requirements, network topology, warm-up, and sampling policy. SDK and CLI measurements are separately labelled.
3. Match content: comparative targets must identify the same SNOMED edition/release or disclose the mismatch prominently. Counts and selected scenario expectations provide a practical cross-check.
4. Separate cold and warm runs: steady-state warm-cache latency remains the default; startup, first-query, and cold-cache measurements are separate profiles with explicit cache-control methodology.
5. Record topology: same-process, same-host loopback, same-LAN, and remote measurements are not mixed. Ping alone is not sufficient metadata.
6. Preserve failures: non-zero subprocess exits, transport failures, timeouts, invalid FHIR responses, and non-conformant results fail the run or scenario according to explicit policy. They are never timed as successes.
7. Store raw samples: medians and percentiles can be regenerated and reviewed. A chart is not the source of truth.
8. Avoid synthetic performance claims: the committed fixture supports CI correctness and smoke timing only. Published capacity or latency claims use a representative licensed release supplied locally.
9. Protect private comparisons: commercial-server targets, endpoints, credentials, and outputs remain under ignored local storage and are not copied into committed documentation.

## Process and lifecycle

The runner should own child-process lifecycle where it starts `sct serve`: select an unused loopback port, capture logs, wait on `/metadata` with a deadline, terminate the process on success/failure/interrupt, and report startup separately from request latency. External comparators remain operator-managed unless a dedicated local wrapper such as `s/snowstorm-lite` owns their lifecycle.

Temporary files use scoped directories and are removed automatically. A requested output directory is created deliberately and never deleted by cleanup. Interrupted runs retain a marked partial result when enough metadata exists to make it useful, while temporary request bodies and credentials are removed.

The runner uses monotonic time for local measurements. It does not shell out to `date`, build SQL strings from scenario input, or parse benchmark state through process-global environment variables. CLI timing invokes argument arrays directly rather than constructing shell command strings.

## Migration

### R48 - contract and vertical slice

Define versioned scenario and result types, create the non-shipped runner and `s/bench` wrapper, and migrate lookup plus lexical search across SDK, CLI, `sct serve`, and one arbitrary FHIR target. Prove parity against the Bash reports, including warm-up, raw samples, semantic preflight, fail-closed behaviour, and text/JSON/Markdown rendering. Keep every existing script.

### R49 - internal sct profiles

Expand Criterion and runner coverage across the SDK, CLI, FST/FTS, hierarchy, subsumption, ECL, startup, and artefact-size boundaries. Treat raw SQLite as a diagnostic target instead of the principal `sct` result. Add synthetic-fixture tests and document the opt-in real-release workflow.

### R50 - comparative FHIR and conformance

Migrate the FHIR request builders, capability discovery, semantic assertions, fixture matrices, latency sampling, and multi-target reports. Replace the CI Bash conformance gate only after the Rust path exercises the same fixture set and failure semantics. Preserve the distinction between HL7-aligned local assertions and official external validation under `R17`.

### R51 - load, reporting, and retirement

Have the runner generate load URLs, invoke `oha`, preserve its raw JSON, normalise load results, capture resource metadata, and render scaling curves. Migrate report generation and environment capture, then remove `bench.sh`, `load.sh`, their sourced libraries/operation scripts, and `conformance.sh` only after parity. Keep thin wrappers for profiling and comparator lifecycle where shell remains the clearest tool.

## Acceptance criteria

- One declarative lookup or search scenario can run unchanged against SDK, CLI, `sct serve`, and an external FHIR target where the operation exists.
- The synthetic fixture exercises scenario parsing, target dispatch, validation, failures, raw-result serialisation, and all renderers in automated tests.
- A failed command, HTTP timeout, invalid response, unavailable requested target, or conformance mismatch cannot produce a successful timing row.
- Raw JSON includes enough version, dataset, target, host, topology, and policy metadata to reproduce or challenge a published result.
- FHIR reports compare any number of labelled targets and only calculate ratios for equivalent boundaries.
- Internal reports distinguish in-process SDK, end-to-end CLI, raw SQLite diagnostic, and FHIR server measurements.
- Load reports preserve raw `oha` output and show throughput, p50, p95, p99, error rate, concurrency, duration, and available resource observations.
- Windows, macOS, and Linux can run the Rust-controlled profiles; platform-specific profiler/load dependencies are detected with actionable errors.
- The current Bash suite remains runnable until its replacement passes fixture and report-parity checks.
- Documentation and published reports name the measurement boundary and do not imply that SDK-versus-network figures are like-for-like server comparisons.

## Deferred decisions

- Whether stable controlled-hardware baselines should eventually gate CI, and which statistical policy would make that trustworthy.
- Whether a hosted benchmark-history service adds enough value beyond committed summary reports and archived raw JSON.
- Whether load-resource capture should use platform APIs, container APIs, or optional adapters after the first `oha` migration.
- Whether the scenario format needs reusable parameter matrices after real cases reveal repetition; do not design a templating language in `R48`.
