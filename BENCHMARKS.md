# Benchmarks

Timing measurements for `sct` commands run against the UK SNOMED CT Monolith Edition
(`SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z`, 831,132 active concepts).

Machine: typical mid-range developer laptop (results will vary).

---

## Methodology

Each command was timed with `time` (wall-clock) on a warm filesystem (second run, after OS page-cache
is populated). Disk is NVMe SSD. NB the first cold run will be slower due to filesystem and page-cache
effects.

```bash
time sct ndjson --rf2 ~/downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/
time sct sqlite  --input snomed.ndjson
time sct parquet --input snomed.ndjson
time sct markdown --input snomed.ndjson
```

---

## Results (to be filled in on real hardware)

| Command | Concepts | Output size | Wall time | Notes |
|---|---|---|---|---|
| `sct ndjson` | 831,132 | ~1.2 GB | ~10s | RF2 parsing + join + sort + serialise |
| `sct sqlite` | 831,132 | ~800 MB | TBD | Stream NDJSON → WAL SQLite + FTS5 rebuild |
| `sct parquet` | 831,132 | ~250 MB | TBD | Batched Arrow writes (50k rows/batch) |
| `sct markdown` | 831,132 | ~2.5 GB | TBD | One file per concept (831k files) |

> Run on your own machine and PR the results.

---

## How to benchmark yourself

### `sct ndjson`

```bash
# Warm the page cache first
cat ~/.downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/**/*.txt > /dev/null 2>&1

# Time the conversion
time sct ndjson --rf2 ~/.downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/
```

### `sct sqlite`

```bash
time sct sqlite --input snomedct-monolithrf2-production-20260311t120000z.ndjson --output snomed.db
ls -lh snomed.db
```

Verify FTS works:
```bash
sqlite3 snomed.db "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 5"
```

### `sct parquet`

```bash
time sct parquet --input snomedct-monolithrf2-production-20260311t120000z.ndjson --output snomed.parquet
ls -lh snomed.parquet
```

Verify DuckDB can read it:
```bash
duckdb -c "SELECT hierarchy, COUNT(*) n FROM 'snomed.parquet' GROUP BY hierarchy ORDER BY n DESC LIMIT 5"
```

### `sct markdown`

```bash
time sct markdown --input snomedct-monolithrf2-production-20260311t120000z.ndjson --output snomed-concepts/
du -sh snomed-concepts/
find snomed-concepts/ -name "*.md" | wc -l
```

---

## MCP server startup time

The `sct mcp` server must start under 100ms to be usable in Claude Desktop without a perceptible delay.
Measure with:

```bash
time echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  | (stdbuf -o0 sct mcp --db snomed.db & sleep 0.3; kill %1) 2>/dev/null
```

Expected: the server prints a response within ~5ms of receiving the first message.
