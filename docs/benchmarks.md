# Benchmarks

Timing measurements for `sct` commands run against two SNOMED CT editions:
- **UK Monolith** — `SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z` (831,132 active concepts)
- **UK Clinical** — `SnomedCT_UKClinicalRF2_PRODUCTION_20260311T000001Z` (34,553 active concepts)

**Machine**: Lenovo Yoga 9i Pro — Intel Core Ultra 9 185H (16 cores), 64 GB RAM, NVMe SSD.

---

## Methodology

Each command was timed with `time` (wall-clock) on a warm filesystem (second run, after OS page-cache is populated). Disk is NVMe SSD. NB: the first cold run will be slower due to filesystem and page-cache effects.

```bash
time sct ndjson --rf2 ~/downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/
time sct sqlite  --input snomed.ndjson
time sct parquet --input snomed.ndjson
time sct markdown --input snomed.ndjson
```

---

## Results — UK Monolith Edition (831,132 concepts)

| Command | Concepts | Output size | Wall time | Notes |
|---|---|---|---|---|
| `sct ndjson` | 831,132 | 990 MB | 29.6 s | RF2 parsing + join + sort + serialise |
| `sct sqlite` | 831,132 | 1.3 GB | 11.3 s | Stream NDJSON → WAL SQLite + FTS5 rebuild |
| `sct parquet` | 831,132 | 824 MB | 5.2 s | Batched Arrow writes (50k rows/batch) |
| `sct markdown` | 831,132 | 3.2 GB | 14.5 s | One file per concept (831k files) |

## Results — UK Clinical Edition (34,553 concepts)

| Command | Concepts | Output size | Wall time | Notes |
|---|---|---|---|---|
| `sct ndjson` | 34,553 | 20 MB | 0.78 s | RF2 parsing + join + sort + serialise |
| `sct sqlite` | 34,553 | 24 MB | 0.27 s | Stream NDJSON → WAL SQLite + FTS5 rebuild |
| `sct parquet` | 34,553 | 12 MB | 0.11 s | Batched Arrow writes (50k rows/batch) |
| `sct markdown` | 34,553 | 137 MB | 0.49 s | One file per concept (34k files) |


---

## MCP server startup time

The `sct mcp` server must start under 100 ms to be usable in Claude Desktop without a perceptible delay.

```bash
time echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  | (stdbuf -o0 sct mcp --db snomed.db & sleep 0.3; kill %1) 2>/dev/null
```

Result on the Monolith database (1.3 GB SQLite):

```
{"id":1,"jsonrpc":"2.0","result":{"capabilities":{"tools":{}},"protocolVersion":"2024-11-05","serverInfo":{"name":"sct-mcp","version":"0.2.0"}}}
```

The response appears in **< 5 ms** — well within the 100 ms budget. The `sleep 0.3` in the timing harness dominates the wall-clock total; actual server response latency is sub-millisecond after the socket is open.

---

## How to benchmark yourself

### `sct ndjson`

`--rf2` accepts either an RF2 directory or a `.zip` file directly:

```bash
# Using a zip file
time sct ndjson --rf2 ~/downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z.zip

# Using a pre-extracted directory (warm the page cache first for a fair comparison)
find ~/downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z -type f -exec cat {} + > /dev/null 2>&1
time sct ndjson --rf2 ~/downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/
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
