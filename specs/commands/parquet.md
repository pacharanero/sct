# `sct parquet` — Export the NDJSON artefact to a Parquet file

Converts the canonical NDJSON artefact to Apache Parquet, enabling columnar analytics via
DuckDB, Python/pandas, R, or Polars — with no server required at query time.

---

## Synopsis

```bash
sct parquet --input <ndjson> --output <parquet>
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--input <file>` | *(required)* | Input `.ndjson` file produced by `sct ndjson`. Use `-` for stdin. |
| `--output <file>` | `snomed.parquet` | Output Parquet file path. |

---

## Examples

```bash
sct parquet --input snomed-20260311.ndjson --output snomed.parquet
ls -lh snomed.parquet

# Verify with DuckDB
duckdb -c "SELECT hierarchy, COUNT(*) n FROM 'snomed.parquet' GROUP BY hierarchy ORDER BY n DESC LIMIT 5"
```

---

## Design notes

- Writes in batches of 50,000 rows using Arrow for memory efficiency.
- All fields from the NDJSON schema are preserved as columns; array/object fields (synonyms,
  hierarchy_path, parents, attributes) are stored as JSON strings.
- DuckDB's FTS extension can be applied on top of the Parquet file for free-text search.
- Parquet is the preferred format for data science and analytics workflows.
