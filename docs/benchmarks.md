Timing measurements for `sct` commands run against a real SNOMED CT release:

- **UK Monolith** - `SnomedCT_MonolithRF2_PRODUCTION_20260701T120000Z` (837,930 active concepts)

**Machine**: Lenovo Yoga 9i Pro - Intel Core Ultra 9 185H (16 cores), 64 GB RAM, NVMe SSD.

**Last verified**: 2026-07-09, against `sct 0.18.2`. If you're reading this much later than that date, treat the numbers as a rough shape rather than gospel - re-run [How to benchmark yourself](#how-to-benchmark-yourself) below for your own hardware and release.

---

## Methodology

Each command was timed with `time` (wall-clock) on a warm filesystem (page cache pre-populated by `cat`-ing the source file first). Disk is NVMe SSD.

This is a single documented run, not an average over many iterations - treat the numbers as a real, reproducible order of magnitude rather than a precise statistical claim. Wall-clock time on a dev laptop is also sensitive to whatever else is running at the time; a quiet machine will do better than the numbers below, which were captured with the usual background load of an active dev environment (editors, a couple of local dev servers, Docker).

FHIR terminology server timings should be treated differently from command
timings. Run the FHIR conformance harness first, then benchmark only servers
that pass the relevant profile:

```bash
benchmarks/conformance.sh --server http://localhost:8080/fhir
benchmarks/bench.sh --db snomed.db --server http://localhost:8080/fhir --runs 20 --warmup 5
```

See [FHIR Conformance And Benchmarks](fhir-conformance-benchmarks.md) for the
full methodology.

```bash
time sct ndjson --rf2 ~/downloads/SnomedCT_MonolithRF2_PRODUCTION_20260701T120000Z/
time sct sqlite   --ndjson snomed.ndjson
time sct parquet  --ndjson snomed.ndjson
time sct markdown --ndjson snomed.ndjson
time sct tct      --db snomed.db
time sct fst build --ndjson snomed.ndjson
```

---

## Results - UK Monolith Edition (837,930 concepts)

| Command | Concepts | Output size | Wall time | Notes |
|---|---|---|---|---|
| `sct ndjson` | 837,930 | 1.3 GB | 51.6 s | RF2 parsing + join + sort + serialise |
| `sct sqlite` | 837,930 | 1.9 GB | 32.4 s | Stream NDJSON → WAL SQLite + FTS5 rebuild |
| `sct parquet` | 837,930 | 785 MB | 6.4 s | Batched Arrow writes (50k rows/batch) |
| `sct markdown` | 837,930 | 3.2 GB | 32.3 s | One file per concept (837,930 files) |
| `sct tct` | 837,930 | 2.6 GB *(db grows 1.9 → 2.6 GB)* | 42.1 s | 11.6M ancestor/descendant pairs over IS-A; INTEGER SCTID columns |
| `sct fst build` | 837,930 | 135 MB | 18.0 s | 1.25M distinct keys, 178k word tokens, 61 semantic tags |

`sct markdown` is the most I/O-bound stage here, not CPU-bound - most of its wall time is filesystem syscalls creating 837,930 individual small files, not computation.

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
