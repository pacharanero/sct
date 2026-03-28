# Benchmarking Suite

A set of Bash scripts that provide a reproducible, automated, fair comparison between `sct`
(local SQLite) and any FHIR R4 terminology server. Lives in `bench/` at the repository root.
Requires only `bash`, `curl`, `sqlite3`, `jq`, and `bc`/`awk` — no Rust build required.
[`hyperfine`](https://github.com/sharkdp/hyperfine) is an optional dependency for
statistically-rigorous timing.

---

## Purpose

- Demonstrate the latency advantage of local-first tooling in talks and documentation
- Detect regressions in `sct` query performance across releases
- Evaluate third-party terminology servers before adopting them

---

## Repository layout

```
bench/
  bench.sh              ← entry point; orchestrates all operations
  lib/
    timing.sh           ← hyperfine wrapper + manual timing fallback
    fhir.sh             ← curl wrappers for each FHIR operation
    local.sh            ← sqlite3 wrappers for each equivalent local operation
    report.sh           ← table/JSON/CSV rendering
  operations/
    lookup.sh           ← single-concept lookup by SCTID
    search.sh           ← free-text search (FTS5 vs ValueSet/$expand)
    children.sh         ← direct children of a concept
    ancestors.sh        ← full ancestor chain
    subsumption.sh      ← is concept A a subtype of concept B?
    bulk.sh             ← batch lookup of N concepts
  fixtures/
    concepts.txt        ← fixed SCTIDs used as lookup/hierarchy fixtures
    search_terms.txt    ← free-text queries used for search fixtures
  README.md
```

---

## Entry point

```bash
bench/bench.sh [OPTIONS]

Options:
  --server URL      Base URL of FHIR terminology server (e.g. https://terminology.openehr.org/fhir)
  --db PATH         Path to snomed.db (default: ./snomed.db)
  --runs N          Timed iterations per operation (default: 10)
  --warmup N        Warmup runs before timing (default: 2)
  --operations LIST Comma-separated: lookup,search,children,ancestors,subsumption,bulk (default: all)
  --format FORMAT   Output format: table (default), json, csv
  --no-remote       Benchmark local operations only
  --timeout SECS    Per-request timeout for remote calls (default: 30)
  --output FILE     Write report to file in addition to stdout
```

---

## Test fixtures

**`bench/fixtures/concepts.txt`**

```
22298006    # Myocardial infarction (disorder)
73211009    # Diabetes mellitus (disorder)
195967001   # Asthma (disorder)
44054006    # Type 2 diabetes mellitus (disorder)
84114007    # Heart failure (disorder)
34000006    # Crohn's disease (disorder)
13645005    # Chronic obstructive lung disease (disorder)
230690007   # Cerebrovascular accident (disorder)
396275006   # Osteoarthritis (disorder)
49436004    # Atrial fibrillation (disorder)
80146002    # Appendectomy (procedure)
302509004   # Entire heart (body structure)
119292006   # Wound of trunk (disorder)
271737000   # Anaemia (disorder)
59282003    # Pulmonary embolism (disorder)
```

**`bench/fixtures/search_terms.txt`**

```
heart attack
diabetes type 2
asthma
blood pressure
fracture femur
hypertension
appendicitis
pulmonary embolism
renal failure
```

---

## Operations

### 1. Concept lookup (`lookup.sh`)

| Side | Implementation |
|---|---|
| Local | `SELECT id, preferred_term, fsn, hierarchy FROM concepts WHERE id = ?` |
| FHIR | `GET {base}/CodeSystem/$lookup?system=http://snomed.info/sct&code={SCTID}&property=display&property=designation` |

Fixture: all 15 SCTIDs in `concepts.txt`; report uses median across all.

### 2. Free-text search (`search.sh`)

| Side | Implementation |
|---|---|
| Local | `SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH ? LIMIT 10` |
| FHIR | `GET {base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs&filter={term}&count=10` |

Fixture: all 9 terms in `search_terms.txt`; report uses median across all.

### 3. Direct children (`children.sh`)

| Side | Implementation |
|---|---|
| Local | `SELECT c.id, c.preferred_term FROM concept_isa ci JOIN concepts c ON ci.child_id = c.id WHERE ci.parent_id = ?` |
| FHIR | `GET {base}/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl%2F<!{SCTID}&count=1000` |

Fixture: `73211009` (Diabetes mellitus) and `195967001` (Asthma).

### 4. Ancestor chain (`ancestors.sh`)

| Side | Implementation |
|---|---|
| Local | Recursive CTE traversal of `concept_isa` |
| FHIR | Iterative `CodeSystem/$lookup` with `property=parent`, or `property=ancestor` if supported |

Fixture: `44054006` (Type 2 diabetes — depth ~8), `230690007` (CVA — depth ~7).

### 5. Subsumption (`subsumption.sh`)

| Side | Implementation |
|---|---|
| Local | Recursive CTE — check if B appears in ancestor chain of A |
| FHIR | `GET {base}/CodeSystem/$subsumes?system=http://snomed.info/sct&codeA={A}&codeB={B}` |

Fixture: 5 pairs — 3 true subsumptions, 2 false.

### 6. Bulk lookup (`bulk.sh`)

| Side | Implementation |
|---|---|
| Local | `SELECT id, preferred_term, fsn FROM concepts WHERE id IN (...)` — single query |
| FHIR | FHIR batch `POST {base}` with 50 `GET CodeSystem/$lookup` entries, falling back to 50 sequential requests |

---

## Timing implementation

```bash
# lib/timing.sh

time_operation() {
  local label="$1"; shift
  local runs="$1"; shift
  local warmup="$1"; shift

  if command -v hyperfine >/dev/null 2>&1; then
    hyperfine --runs "$runs" --warmup "$warmup" \
              --export-json /tmp/bench_result.json \
              "$@" >/dev/null 2>&1
    jq -r '.results[0].median * 1000 | floor' /tmp/bench_result.json
  else
    local times=()
    for _ in $(seq 1 "$((warmup + runs))"); do
      local start end_ns
      start=$(date +%s%N)
      "$@" >/dev/null 2>&1
      end_ns=$(date +%s%N)
      times+=( $(( (end_ns - start) / 1000000 )) )
    done
    printf '%s\n' "${times[@]:$warmup}" | sort -n | awk 'BEGIN{c=0} {a[c++]=$1} END{print a[int(c/2)]}'
  fi
}
```

---

## Report format

```
sct benchmark — 2026-03-28
  Local DB : /home/marcus/snomed.db  (v20260101, 831,132 concepts)
  Remote   : https://terminology.openehr.org/fhir
  Runs     : 10 (+ 2 warmup)

┌──────────────────────────┬───────────────────┬───────────────────┬──────────────────┐
│ Operation                │ sct (local)       │ FHIR (remote)     │ Speedup          │
├──────────────────────────┼───────────────────┼───────────────────┼──────────────────┤
│ Concept lookup           │       1.2 ms      │      248 ms       │    206× faster   │
│ Text search (top 10)     │       3.8 ms      │      334 ms       │     88× faster   │
│ Direct children          │       2.3 ms      │      421 ms       │    183× faster   │
│ Ancestor chain           │       3.1 ms      │     1 840 ms      │    594× faster   │
│ Subsumption test         │       1.4 ms      │      209 ms       │    149× faster   │
│ Bulk lookup (50)         │       4.6 ms      │   12 450 ms  [1]  │  2 707× faster   │
└──────────────────────────┴───────────────────┴───────────────────┴──────────────────┘

[1] Server does not support FHIR batch; 50 sequential requests issued.

Times shown are wall-clock median. Local times include sqlite3 process startup.
```

With `--format json`, each row is emitted as a JSON object — suitable for CI artifact storage.

---

## Fairness notes

- **SQLite vs HTTP**: The primary cost difference is network round-trips. The report always
  states measured ping latency to distinguish "server is slow" from "network is slow".
- **sct includes process startup**: Each local command is `sqlite3 snomed.db "..."`, which
  includes process fork + open overhead (~5–15 ms). This is intentional — it reflects real
  CLI usage.
- **Warm cache**: Warm-up runs are issued before timing to put both sides in a hot-cache state.
  We benchmark steady-state use, not cold-start.
- **Network jitter**: All remote timings report stddev alongside median.

---

## Dependencies

| Tool | Required | Purpose |
|---|---|---|
| `bash` ≥ 4.0 | Yes | Script runtime |
| `curl` | Yes | FHIR HTTP calls |
| `sqlite3` | Yes | Local queries |
| `jq` | Yes | JSON parsing |
| `bc` or `awk` | Yes | Arithmetic |
| `hyperfine` | Recommended | Statistical timing |

`hyperfine`: `cargo install hyperfine`, `brew install hyperfine`, or via your Linux package manager.
