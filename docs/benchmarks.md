Timing measurements for `sct` commands run against a real SNOMED CT release:

- **UK Monolith** - `SnomedCT_MonolithRF2_PRODUCTION_20260701T120000Z` (837,930 active concepts)

**Last verified**: 2026-07-28, against `sct 0.20.1`. If you're reading this much later than that date, treat the numbers as a rough shape rather than gospel - re-run [How to benchmark yourself](#how-to-benchmark-yourself) below for your own hardware and release.

---

## Benchmark machines

| Machine | CPU | RAM | Storage | Cores |
|---|---|---|---|---|
| Lenovo Yoga 9i Pro | Intel Core Ultra 9 185H | 64 GB | NVMe SSD | 22 |
| Raspberry Pi 5 | Broadcom BCM2712 (ARM Cortex-A76) | 8 GB | microSD | 4 |
| OnePlus 13 (Termux) | Qualcomm Snapdragon 8 Elite (Oryon, 4.32 GHz) | 12 GB | UFS 4.0 | 8 |

---

## Methodology

Each command was timed as a single run (not averaged) with peak RSS captured via `/usr/bin/time -v` (GNU time `getrusage`) on the Raspberry Pi and `/proc/PID/status` VmHWM polling on the Lenovo (which lacks GNU time). Wall time is wall-clock seconds. Both methods report the maximum resident set size of the process in GiB.

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

### Pipeline timings and peak memory

| Command | Output size | Lenovo 9i - wall | Lenovo 9i - RSS | RPi 5 - wall | RPi 5 - RSS | Notes |
|---|---|---:|---:|---:|---:|---|
| `sct ndjson` | 1.3 GB | 42.1 s | 3.73 GiB | 103.1 s | 3.69 GiB | RF2 parsing + join + stream serialise |
| `sct sqlite` | 2.4-2.7 GB | 26.3 s | 0.28 GiB | 273.8 s | 0.29 GiB | Stream NDJSON to WAL SQLite + FTS5 rebuild |
| `sct parquet` | 785 MB | 6.0 s | 0.99 GiB | 12.9 s | 0.98 GiB | Batched Arrow writes (50k rows/batch) |
| `sct tct` | db grows to 2.6-2.7 GB | 39.1 s | 0.74 GiB | 156.2 s | 0.80 GiB | 11.6M ancestor/descendant pairs over IS-A |
| `sct fst build` | 135 MB | 18.1 s | 0.77 GiB | 34.5 s | 0.79 GiB | 1.25M distinct keys, 178k word tokens |

The SQLite database size difference (2.4 GB Lenovo vs 2.7 GB RPi) reflects minor allocator and WAL checkpointing differences between the two platforms; both ran the same `sct tct` which grows the database by ~600 MB via the `concept_ancestors` table.

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

The `sct mcp` server should start fast enough to avoid a perceptible delay when a client like Claude Desktop opens it. It answers the `initialize` handshake in a few milliseconds regardless of database size, because it opens the SQLite file rather than loading it into memory:

```bash
time echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  | (stdbuf -o0 sct mcp --db snomed.db & sleep 1; kill %1) 2>/dev/null
```

| Database | Size | Response time |
|---|---|---|
| Synthetic test fixture (`tests/fixtures/rf2/`) | 136 KB, 22 concepts | ~2.5 ms (3 runs: 2.1 / 2.6 / 2.8 ms) |
| UK Monolith, with TCT | 2.6 GB, 837,930 concepts | ~2.3 ms (3 runs: 2.6 / 2.3 / 2.0 ms) |

Startup is a few milliseconds regardless of database size: the server opens the SQLite file (near-instant, it does not read it into memory) and reads provenance from a small keyed table. The response carries a `serverInfo` block embedding a `_provenance` object describing the loaded release:

```json
{"id":1,"jsonrpc":"2.0","result":{"capabilities":{"tools":{}},"protocolVersion":"2024-11-05","serverInfo":{"_provenance":{"created_at":"2026-07-09T16:18:53Z","edition_label":"uk_sct2mo_42.3.0_20260701000001Z","release_date":"2026-07-01","release_id":"uk_sct2mo_42.3.0_20260701000001Z","sct_version":"0.18.2","source_paths":["..."]},"name":"sct-mcp","version":"0.18.2"}}}
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
