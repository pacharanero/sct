# `sct bench` - self-benchmark

Status: Proposed. Roadmap item: `R52`. Companion document: [`benchmark-runner.md`](../benchmark-runner.md), which specifies the separate, non-shipped comparative runner.

## 1. Why

`sct` makes performance claims - "almost ridiculously fast on modern hardware" in the README, a benchmarks page with figures from three machines. A user has no way to check any of that on their own hardware without cloning the repository and running a Bash suite that expects Docker, `oha`, and a comparator server.

That gap has practical consequences beyond marketing. "Is it slow for everyone or just me?" is unanswerable. Bug reports about performance arrive without numbers. The [Android/Termux timings](../../docs/benchmarks.md#android-phone-oneplus-13-termux) that produced the most interesting performance finding to date - a phone matching a laptop on one stage and a Raspberry Pi on another - were assembled by hand from `time` output pasted into a chat.

`sct bench` closes the gap: one command, no clone, no extra tooling, a result the user can read and paste.

### 1.1 Explicitly not the comparative runner

[`benchmark-runner.md`](../benchmark-runner.md) states that the benchmark runner is not a shipped subcommand. That remains true of the *comparative* runner. This command exists because the two tools answer different questions with different dependencies:

- `sct bench` measures the local artefacts through the SDK and CLI boundaries. It needs a database and nothing else.
- The comparative runner measures other people's servers, drives load, and checks conformance. It needs Docker, `oha`, and a comparator.

The dependency boundary is the shipping boundary. See [Relationship to `sct bench`](../benchmark-runner.md#relationship-to-sct-bench) for the full split and the shared result schema.

## 2. Design decisions

### 2.1 Measuring from inside the shipped binary is a feature

Timing the SDK from within the release binary measures the artefact the user actually runs, built with the flags it actually shipped with. A separately compiled dev harness measures something subtly different. For the `cli` profile this is unavoidable anyway: the thing being measured is process startup plus argument parsing plus database open, which only the real binary exhibits.

### 2.2 No Criterion in the shipped binary

Criterion is a heavy dependency and pulls in plotting and reporting machinery irrelevant here. [`benchmark-runner.md`](../benchmark-runner.md) already assigns `sdk` and `cli` profiles to a plain monotonic clock and reserves Criterion for in-process `micro` benches, which stay as `cargo bench` targets in the repository. `sct bench` therefore needs a sampling loop, percentile arithmetic, and nothing else.

Statistical honesty comes from method and disclosure, not from a framework: report the median with p95 and sample count, never a bare mean, and state that this is a single run on an uncontrolled machine.

### 2.3 Default to a run that finishes

A benchmark nobody waits for gets no data. The default run targets **under 30 seconds** on a modest machine, using the resolved database as-is. `--full` opts into the longer profile set. Anything requiring a build step (`sct ndjson`, `sct sqlite`) is **not** in the default run - a user benchmarking a pipeline they have already run once should not be made to run it again unprompted. `--pipeline` covers that case explicitly, given an RF2 input.

### 2.4 Machine-shareable by default, pretty by default

These are not in tension. Terminal output is formatted for reading; `--format markdown` produces something pasteable into an issue or a forum post; `--format json` is the canonical machine form and matches the runner's schema; `--format html` writes a standalone file for sharing outside a terminal.

## 3. Command surface

```text
sct bench [OPTIONS]

Options:
      --db <PATH>           Database to benchmark. Default: the usual resolution chain.
      --profiles <LIST>     Comma-separated: sdk, cli, artefact. Default: sdk,cli,artefact.
      --full                Longer run: more samples, deeper hierarchy and ECL cases.
      --pipeline <RF2>      Also time a full build from this RF2 zip or directory.
                            Writes to a temporary directory, removed afterwards.
      --samples <N>         Override the per-case sample count.
      --warmup <N>          Override the per-case warm-up count.
      --format <FMT>        text | markdown | json | html. Default: text.
      --output <PATH>       Write to a file instead of stdout. Required for html
                            unless stdout is redirected.
      --baseline <PATH>     Compare against a previous JSON result and show deltas.
      --no-provenance       Omit dataset identity (release id, concept count) from output.
```

`sct bench` follows the repo-wide conventions in [`adding-a-command.md`](../adding-a-command.md): `--format`/`OutputFormat`, `--db`/`tilde_pathbuf`, data on stdout and progress on stderr, and a read-only database connection through `open_db_readonly`.

### 3.1 Profiles

| Profile | Measures | Boundary |
|---|---|---|
| `sdk` | Concept lookup, lexical search, children, ancestors, subsumption, ECL expansion, FST prefix/fuzzy search where an index is present | In-process, warm cache |
| `cli` | The same operations through a subprocess, capturing startup, argument parsing, database open, query, and output | Whole binary, per-invocation |
| `artefact` | Sizes of the database, FST index, embeddings, and derived tables; presence of TCT; schema version | Static inspection, not timed |
| `pipeline` | `sct ndjson`, `sct sqlite`, `sct parquet`, `sct fst build`, `sct tct` end to end | Whole binary, opt-in via `--pipeline` |

The `sdk`/`cli` pairing is the point of the exercise: the difference between them is what a user pays for process startup, which is the single most common surprise in CLI performance and is invisible to an in-process benchmark.

### 3.2 Scenario selection

Cases are embedded in the binary, not read from `benchmarks/scenarios/` - the user has no repository. They are drawn from the same declarative corpus at build time, so the shipped set and the runner's set cannot drift silently.

Cases must degrade honestly on an arbitrary database. A case naming a concept absent from the user's edition is **skipped and reported as skipped**, never silently dropped or substituted, and never timed against a missing row. The default set should prefer concepts present in every SNOMED CT edition (for example `138875005` as the root, and International-core disorders) over UK-specific ones, and each case declares the dataset requirements it needs.

### 3.3 Output

Terminal output leads with the machine and dataset, then per-operation medians, then the SDK-versus-CLI gap:

```text
sct bench 0.21.0

  Machine     Intel Core Ultra 9 185H, 22 cores, 64 GB
  Database    uk-monolith-42.db, 837,930 concepts, UK Monolith 42.3.0 (2026-07-01)
  Artefacts   db 2.4 GB, fst 135 MB, tct present, embeddings absent

  Operation              SDK (median)      CLI (median)     startup cost
  lookup by SCTID              0.09 ms          8.4 ms           8.3 ms
  lexical search "heart"       1.20 ms          9.6 ms           8.4 ms
  children                     0.21 ms          8.6 ms           8.4 ms
  ancestors                    0.34 ms          8.7 ms           8.4 ms
  subsumption test             0.05 ms          8.3 ms           8.3 ms
  ECL << 73211009             14.80 ms         23.4 ms           8.6 ms
  FST prefix "myoca"           0.03 ms          8.3 ms           8.3 ms

  10 samples per case after 3 warm-up runs; medians shown, p95 in --format json.
  Single run on an uncontrolled machine - treat as an order of magnitude.

  Share:  sct bench --format markdown | pbcopy
```

Markdown output is the same content as tables with a fenced environment block, sized for a forum post or issue. HTML is a standalone file with inline CSS and no network assets, consistent with the project's no-CDN rule. JSON is the canonical schema from [`benchmark-runner.md`](../benchmark-runner.md#result-model), restricted to the supported profiles.

### 3.4 Provenance and privacy

Output carries the metadata a number is meaningless without: `sct` version, OS, architecture, CPU model, core count, total RAM, dataset identity from the provenance table, and sampling policy. It must never carry absolute filesystem paths, hostnames, usernames, or a TRUD API key. `--no-provenance` additionally suppresses release identity for users who consider their edition licensing sensitive.

Nothing is uploaded. There is no telemetry, no submission endpoint, and no network access in any profile - consistent with the project's local-first guarantee.

## 4. Acceptance criteria

- `sct bench` runs to completion against the committed synthetic fixture and against a full Monolith database, with no arguments beyond database resolution.
- The default run completes in under 30 seconds on a Raspberry Pi 5.
- A case whose concepts are absent from the database is reported as skipped, and skipped cases never contribute timing rows.
- `--format json` validates against the shared result schema and can be ingested by the comparative runner as a labelled target result.
- `--format markdown` output pastes into a GitHub issue and a Discourse post without manual fixing.
- `--format html` writes a self-contained file with no external requests.
- No output in any format contains a filesystem path, hostname, username, or credential.
- Terminal output states sample count, warm-up count, and the single-run caveat.
- `--baseline` reports per-case deltas and flags changes outside the noise band rather than presenting every fluctuation as a regression.
- The binary gains no heavyweight dependency; Criterion in particular is not linked into the shipped artefact.

## 5. Deferred

- **Result submission.** A "contribute your benchmark" flow would populate the benchmarks page from real hardware, but it needs a privacy review, a moderation story, and an endpoint the project does not have. Users can paste Markdown into the forum in the meantime.
- **Clipboard integration.** `--copy` would need a clipboard crate per platform, and `| pbcopy` / `| wl-copy` / `| clip` already work. Revisit only if the pipe proves a real obstacle.
- **CI regression gating.** Out of scope here for the same reason as the runner: meaningful thresholds need controlled hardware and a baseline policy.
- **Comparative modes.** Any form of "compare my `sct` against server X" belongs to the runner, not here, however tempting it is to add one flag.
