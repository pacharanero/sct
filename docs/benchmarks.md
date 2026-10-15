Timing measurements for `sct` commands run against a real SNOMED CT release:

- **UK Monolith** - `SnomedCT_MonolithRF2_PRODUCTION_20260701T120000Z` (837,930 active concepts)

**Last verified**: 2026-10-15, against `sct 0.29.0`. The v0.20.1 and non-Lenovo measurements are retained as labelled historical evidence rather than silently replaced. Re-run [How to benchmark yourself](#how-to-benchmark-yourself) for your own hardware and release.

---

## Benchmark machines

| Machine | CPU | RAM | Storage | Cores |
|---|---|---|---|---|
| Lenovo Yoga 9i Pro | Intel Core Ultra 9 185H | 64 GB | NVMe SSD | 22 |
| Raspberry Pi 5 | Broadcom BCM2712 (ARM Cortex-A76) | 8 GB | microSD | 4 |
| OnePlus 13 (CPH2653, Termux) | Qualcomm Snapdragon 8 Elite (Oryon, 4.32 GHz) | 16 GB LPDDR5X | 512 GB UFS 4.0 | 8 |

---

## Methodology

Each pipeline command was timed as a single run (not averaged) with peak RSS captured via `/usr/bin/time -v` (GNU time `getrusage`) on the Raspberry Pi and zsh `%M` (`getrusage`) on the Lenovo. Wall time is wall-clock seconds. Both methods report the maximum resident set size of the process in GiB.

On the Lenovo, the source ZIP was on NVMe with a warm page cache. On the Raspberry Pi, the source ZIP was on the microSD card. Both machines processed the same `uk_sct2mo_42.3.0_20260701000001Z.zip` archive with default settings (active concepts only, simple refsets). The `sct ndjson` stage extracts the ZIP to a temporary directory before processing.

This is a single documented run per machine, not an average over many iterations - treat the numbers as a real, reproducible order of magnitude rather than a precise statistical claim. Wall-clock time is sensitive to whatever else is running at the time.

FHIR terminology server timings should be treated differently from command timings. Run the FHIR conformance harness first, then benchmark only servers that pass the relevant profile:

```bash
benchmarks/conformance.sh --server http://localhost:8080/fhir
benchmarks/bench.sh --db snomed.db --server http://localhost:8080/fhir --runs 20 --warmup 5
```

See [FHIR Conformance And Benchmarks](fhir-conformance-benchmarks.md) for the full methodology.

---

## Results - UK Monolith Edition (837,930 concepts)

### Lenovo v0.29.0 refresh

This run used the `v0.29.0` source tag at commit `bfcf41c2196152da5cc08e906c683ff3621b25a8`, compiled in release mode. The archive was read once immediately before the timed `sct ndjson` run to warm its page-cache pages. Each later stage read the artefact produced by the preceding current-version stage; the table therefore does not mix old database or index formats with the current binary.

| Command | Output size | Wall | Peak RSS | Change from Lenovo v0.20.1 |
|---|---:|---:|---:|---:|
| `sct ndjson` | 1.3 GB | 34.64 s | 3.88 GiB | 17.7% faster |
| `sct sqlite` | 1.8 GB | 23.11 s | 0.33 GiB | 12.1% faster |
| `sct parquet` | 785 MB | 5.15 s | 0.99 GiB | 14.2% faster |
| `sct tct` | db grows 1.8 -> 2.6 GB | 35.77 s | 0.80 GiB | 8.5% faster |
| `sct fst build` | 134.2 MB | 14.89 s | 0.77 GiB | 17.7% faster |

The exact current outputs were 1,347,019,519-byte NDJSON, 1,952,866,304-byte SQLite before TCT, 2,780,413,952-byte SQLite after TCT, 822,718,072-byte Parquet, and 140,753,811-byte FST. The TCT contained 11,607,152 verified ancestor-descendant pairs. Peak RSS is not uniformly lower than v0.20.1, so the table reports performance and memory separately rather than presenting the wall-time improvements as an across-the-board resource reduction.

### Warm SDK and CLI queries in v0.29.0

`sct bench --full` ran 30 measured samples after 5 warmups against the current database, verified TCT, and matching FST. SDK is in-process with warm caches; CLI includes process startup, argument parsing, artefact opening, query execution, and output. Median and p95 are shown separately so one fast sample is not presented as typical.

| Operation | SDK median | SDK p95 | CLI median | CLI p95 |
|---|---:|---:|---:|---:|
| Lookup by SCTID | 0.023 ms | 0.025 ms | 31.7 ms | 33.8 ms |
| Lexical search `heart` | 0.885 ms | 0.974 ms | 32.7 ms | 36.2 ms |
| Direct children | 0.026 ms | 0.028 ms | 2.11 ms | 2.50 ms |
| Ancestors | 0.632 ms | 0.660 ms | 2.30 ms | 2.70 ms |
| Subsumption | 0.115 ms | 0.133 ms | 2.30 ms | 2.80 ms |
| ECL descendants | 0.104 ms | 0.114 ms | 2.47 ms | 2.82 ms |
| FST prefix | 0.069 ms | 0.074 ms | 3.89 ms | 4.78 ms |
| Broad ECL expansion | 18.2 ms | 20.7 ms | 23.4 ms | 26.7 ms |
| High-fanout children | 0.528 ms | 0.590 ms | 3.25 ms | 3.77 ms |

The large CLI gap for lookup and lexical search is the whole-process boundary, not database query latency; use the SDK or long-running server when repeatedly querying from one application.

### Snowstorm Lite comparison

The comparator was Snowstorm Lite 2.5.0, image `snomedinternational/snowstorm-lite@sha256:892fcdc8f482c5ec0480d67cba4271669ea06b54ec73738bba02322ad2c89c11`, running Java 17 with `-Xms8g -Xmx16g`. A clean Docker volume received the same `uk_sct2mo_42.3.0_20260701000001Z.zip`; Snowstorm reported the loaded code system as `http://snomed.info/sct/83821000000107/version/20260701`. Its synchronous package load took 702.76 s, produced a 482.4 MiB Lucene volume, and reached a cgroup `memory.peak` of 9,423,003,648 bytes (8.78 GiB). The comparable `sct ndjson` plus `sct sqlite` path took 57.75 s and peaked at 3.88 GiB; including the optional TCT brought `sct` to 93.52 s.

Query timings compare `sct serve 0.29.0` and Snowstorm Lite over separate loopback ports on the same machine. Both served the same release, were warmed by conformance and metadata requests, and were measured by Hyperfine 2.0.0 for 20 runs after 5 warmups. These are FHIR-to-FHIR measurements: both sides include loopback HTTP, parsing, server processing, and serialization.

| Operation | `sct serve` median | Snowstorm median | Result |
|---|---:|---:|---:|
| Concept lookup | 5.2 ms | 9.5 ms | `sct` 1.8x faster |
| Text search, top 10 | 5.0 ms | 11.9 ms | `sct` 2.4x faster |
| Direct children | 4.9 ms | 8.4 ms | `sct` 1.7x faster |
| Ancestor chain, 6 sequential lookups | 60.5 ms | 98.7 ms | `sct` 1.6x faster |
| Subsumption | 5.2 ms | 38.8 ms | `sct` 7.5x faster |
| Bulk lookup, 15 sequential lookups | 74.0 ms | 117.8 ms | `sct` 1.6x faster |

The exact benchmark requests succeeded against both servers. The broader 35-case conformance harness was also retained rather than reduced to a pass/fail marketing claim: `sct` passed 30 and failed 5, while Snowstorm passed 32 and failed 3. `sct`'s failures were two deliberately bounded broad expansions and three translation cases unavailable because this default build omitted payload refsets; Snowstorm's were two translation limitations and an unknown-code lookup returning HTTP 500 instead of 404. Those failed scenarios were not timed. The [raw self-benchmark](https://github.com/pacharanero/sct/blob/main/benchmarks/reports/2026-10-15-v0.29.0-lenovo-self-current-artefacts.json), [server comparison](https://github.com/pacharanero/sct/blob/main/benchmarks/reports/2026-10-15-v0.29.0-sct-serve-vs-snowstorm-lite.json), [environment record](https://github.com/pacharanero/sct/blob/main/benchmarks/reports/2026-10-15-v0.29.0-lenovo-environment.md), [`sct` conformance output](https://github.com/pacharanero/sct/blob/main/benchmarks/reports/2026-10-15-sct-v0.29.0-current-artefacts-conformance.jsonl), and [Snowstorm conformance output](https://github.com/pacharanero/sct/blob/main/benchmarks/reports/2026-10-15-snowstorm-lite-current-artefacts-conformance.jsonl) are committed evidence for these figures.

This supports a narrow claim: on this release and laptop, `sct serve` was faster for these six read-only FHIR workflows. It does not establish that `sct` is universally faster, more conformant, or more capable than Snowstorm.

### Historical v0.20.1 and Raspberry Pi 5 pipeline timings

| Command | Output size | Lenovo 9i - wall | Lenovo 9i - RSS | RPi 5 - wall | RPi 5 - RSS | Notes |
|---|---|---:|---:|---:|---:|---|
| `sct ndjson` | 1.3 GB | 42.1 s | 3.73 GiB | 103.1 s | 3.69 GiB | RF2 parsing + join + stream serialise |
| `sct sqlite` | 1.8 GB | 26.3 s | 0.28 GiB | 273.8 s | 0.29 GiB | Stream NDJSON to WAL SQLite + FTS5 rebuild |
| `sct parquet` | 785 MB | 6.0 s | 0.99 GiB | 12.9 s | 0.98 GiB | Batched Arrow writes (50k rows/batch) |
| `sct tct` | db grows 1.8 -> 2.6 GB | 39.1 s | 0.74 GiB | 156.2 s | 0.80 GiB | 11.6M ancestor/descendant pairs over IS-A |
| `sct fst build` | 135 MB | 18.1 s | 0.77 GiB | 34.5 s | 0.79 GiB | 1.25M distinct keys, 178k word tokens |

The v0.20.1 Lenovo verification produced a 1,972,310,016-byte SQLite database before TCT and a 2,799,837,184-byte database afterwards. The human-readable values above use the same 1024-based formatter as `sct info`; `ls -lh` rounds the same files to approximately 1.9 GiB and 2.7 GiB.

### Android phone (OnePlus 13, Termux)

`sct` runs under [Termux](android-termux.md) on Android, so the whole pipeline was run on a phone. This is kept separate from the table above rather than added as two more columns, because it is **not a like-for-like comparison**: peak RSS was not captured, and the binary was built from source under Termux (linking against Bionic) rather than being the released `linux-aarch64` musl build the other machines effectively represent. Single run, wall-clock only, thermal state not controlled.

| Command | OnePlus 13 | RPi 5 | Lenovo 9i | Phone vs Pi | Phone vs Lenovo |
|---|---:|---:|---:|---:|---:|
| `sct ndjson` | 103.7 s | 103.1 s | 42.1 s | 1.0x | 2.5x slower |
| `sct sqlite` | 73.8 s | 273.8 s | 26.3 s | 3.7x faster | 2.8x slower |
| `sct parquet` | 12.0 s | 12.9 s | 6.0 s | 1.1x faster | 2.0x slower |
| `sct tct` | 64.6 s | 156.2 s | 39.1 s | 2.4x faster | 1.7x slower |
| `sct fst build` | 18.8 s | 34.5 s | 18.1 s | 1.8x faster | 1.04x slower |

A 2026 flagship phone builds a national SNOMED CT edition end to end in about 4.5 minutes, and matches a 22-core laptop on `sct fst build`.

The interesting part is that it does *not* behave like uniformly slower hardware. On `sct fst build`, `sct tct`, and `sct sqlite` it lands much closer to the laptop than to the Pi. On `sct ndjson` and `sct parquet` it drops back to Pi-5 pace, despite far faster silicon and UFS 4.0 storage where the Pi has a microSD card.

Those two stages are the allocation-heavy ones - `sct ndjson` builds millions of short-lived strings and records, and `sct parquet` accumulates Arrow arrays - which suggests the ceiling is allocator throughput rather than CPU or I/O. Bionic's hardened `scudo` allocator is a reasonable suspect against glibc on the other two machines. Supporting evidence: the `sct parquet` progress bar reported ~108 MiB/s reading the NDJSON, far below what UFS 4.0 delivers, so the stage is not storage-bound.

**This is a hypothesis, not a measured result.** Testing it properly means running the same stage on the same handset under a glibc userland (`proot-distro` Debian) and comparing, plus capturing peak RSS. If you have a phone and ten minutes, that data would be welcome.

### Raspberry Pi 5: from OOM crash to completion

Before the v0.20.1 streaming optimisation, `sct ndjson` crashed on the Raspberry Pi 5 (8 GB RAM) when building the UK Monolith, almost certainly due to out-of-memory conditions. The old implementation materialised every RF2 file as a whole-file `Vec<Row>` before aggregation, then built the complete `Vec<ConceptRecord>` before writing a single byte - so on a national edition the loaded dataset, multi-gigabyte transient row vectors, and the full output record set could all be resident at once.

The v0.20.1 streaming implementation streams rows directly into the dataset maps and writes records to the output file as they are built. The Raspberry Pi 5 now completes the full pipeline (ndjson, sqlite, parquet, tct, fst) successfully within its 8 GB RAM, with 3.69 GiB peak RSS during the most memory-intensive stage.

### `sct ndjson` memory improvement in v0.20.1

The v0.20.1 streaming implementation was compared with its immediate pre-change parent (`b2ca527`) on 2026-07-27. Both were optimised release builds processing the same `uk_sct2mo_42.3.0_20260701000001Z.zip` archive with the default active-concept and simple-refset settings, a disk-backed temporary directory, and regular file output. Peak RSS is Linux `getrusage` maximum resident set size as reported by zsh `%M`; wall time is one paired run rather than a statistical benchmark.

| Implementation | Peak RSS | Wall time |
|---|---:|---:|
| Before streaming | 6.42 GiB | 58.71 s |
| v0.20.1 streaming | 3.73 GiB | 43.95 s |
| Improvement | **2.68 GiB / 41.84% lower** | **25.14% shorter** |

Both runs emitted 837,930 concepts in 1,346,971,886-byte artefacts with byte-identical concept records and the same content fingerprint (`sha256:abc9de055e67073b56cc21c01b95762c60cb138f839cbf2bdff5894b4a84500e`). The lower peak leaves substantially more headroom on 8 GB machines and avoids the severe swap pressure that can make RF2 conversion appear to stall on slower storage.

Only the UK Monolith is benchmarked currently. The previous version of this page also carried UK Clinical Edition numbers; they've been dropped rather than left stale, since re-running them needs a fresh TRUD-authenticated download this environment didn't have to hand. Re-add if useful - Clinical is ~24x smaller and everything scales down accordingly.

---

## MCP server startup time

The `sct mcp` server should start fast enough to avoid a perceptible delay when an MCP client opens it. Benchmark the current stateless `server/discover` lifecycle with a complete 2026-07-28 metadata object:

```bash
time printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}' \
  | sct mcp --db snomed.db --codelist-root ./codelists >/dev/null
```

The recorded figures below are the pre-`rmcp` v0.18.2 baseline using the legacy initialization path. They remain useful evidence that database size does not force terminology loading at startup, but the SDK-backed 2026-07-28 path must be remeasured before publishing a current comparison.

| Database | Size | v0.18.2 response time |
|---|---|---|
| Synthetic test fixture (`tests/fixtures/rf2/`) | 136 KB, 22 concepts | ~2.5 ms (3 runs: 2.1 / 2.6 / 2.8 ms) |
| UK Monolith, with TCT | 2.6 GB, 837,930 concepts | ~2.3 ms (3 runs: 2.6 / 2.3 / 2.0 ms) |

The server opens the SQLite file without reading it into memory and reads provenance from a small keyed table. Current discovery responses advertise supported versions, capabilities, cache hints, and server identity; release provenance is carried in result `_meta`:

```json
{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2024-11-05","2025-03-26","2025-06-18","2025-11-25","2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"org.sct/provenance":{"release_id":"...","release_date":"..."},"io.modelcontextprotocol/serverInfo":{"name":"sct-mcp","version":"..."}}}}
```

**Note on an earlier regression:** a prior release briefly took ~370 ms to start against a full Monolith database, because its startup schema-version check ran `SELECT MAX(schema_version) FROM concepts` - a full-table scan of an unindexed column. Reading a single row instead (the value is uniform across concepts) restored the few-millisecond startup shown above, on databases of any size. See issue #32.

---

## How to benchmark yourself

### `sct ndjson`

`--rf2` accepts either an RF2 directory or a `.zip` file directly:

```bash
# Using a zip file
time sct ndjson --rf2 ~/downloads/SnomedCT_MonolithRF2_PRODUCTION_20260701T120000Z.zip

# Using a pre-extracted directory (warm the page cache first for a fair comparison)
find ~/downloads/SnomedCT_MonolithRF2_PRODUCTION_20260701T120000Z -type f -exec cat {} + > /dev/null 2>&1
time sct ndjson --rf2 ~/downloads/SnomedCT_MonolithRF2_PRODUCTION_20260701T120000Z/
```

### `sct sqlite`

```bash
time sct sqlite --ndjson snomedct-monolithrf2-production-20260701t120000z.ndjson --output snomed.db
ls -lh snomed.db
```

Verify FTS works:
```bash
sqlite3 snomed.db "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 5"
```

### `sct parquet`

```bash
time sct parquet --ndjson snomedct-monolithrf2-production-20260701t120000z.ndjson --output snomed.parquet
ls -lh snomed.parquet
```

Verify DuckDB can read it:
```bash
duckdb -c "SELECT hierarchy, COUNT(*) n FROM 'snomed.parquet' GROUP BY hierarchy ORDER BY n DESC LIMIT 5"
```

### `sct markdown`

```bash
time sct markdown --ndjson snomedct-monolithrf2-production-20260701t120000z.ndjson --output snomed-concepts/
du -sh snomed-concepts/
find snomed-concepts/ -name "*.md" | wc -l
```

### `sct tct`

Builds the transitive closure table (`concept_ancestors`) over an existing SQLite database - needed for subsumption-heavy workloads or the SCT-QL compiler, not built by default:

```bash
time sct tct --db snomed.db
ls -lh snomed.db
sqlite3 snomed.db "SELECT COUNT(*) FROM concept_ancestors"
```

### `sct fst build`

```bash
time sct fst build --ndjson snomedct-monolithrf2-production-20260701t120000z.ndjson --output snomed.fst
ls -lh snomed.fst
```

Verify search works:
```bash
sct fst search "myocardial infarction" --index snomed.fst
```
