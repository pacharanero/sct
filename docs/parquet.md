+++
title = "sct parquet"
weight = 10
+++

Export a SNOMED CT NDJSON artefact to a Parquet file, directly queryable by DuckDB without any import step.

---

## Usage

```
sct parquet --input <NDJSON> [--output <PARQUET>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--input <FILE>` | *(required)* | NDJSON file produced by `sct ndjson`. Use `-` for stdin. |
| `--output <FILE>` | `snomed.parquet` | Output Parquet file path. |

---

## Example

```bash
sct parquet \
  --input snomedct-monolithrf2-production-20260311t120000z.ndjson \
  --output snomed.parquet
```

---

## Schema

| Column | Type | Notes |
|---|---|---|
| `id` | `VARCHAR` | SCTID |
| `fsn` | `VARCHAR` | Fully Specified Name |
| `preferred_term` | `VARCHAR` | Preferred term for selected locale |
| `synonyms` | `VARCHAR` | JSON array of strings |
| `hierarchy` | `VARCHAR` | Top-level hierarchy label |
| `hierarchy_path` | `VARCHAR` | JSON array of strings |
| `parents` | `VARCHAR` | JSON array of `{id, fsn}` |
| `children_count` | `BIGINT` | |
| `active` | `BOOLEAN` | |
| `module` | `VARCHAR` | SNOMED module identifier |
| `effective_time` | `VARCHAR` | `YYYYMMDD` |
| `attributes` | `VARCHAR` | JSON object of attribute groups |
| `schema_version` | `BIGINT` | Artefact schema version |

Array/object columns are stored as JSON strings. DuckDB's `json_extract`, `json_extract_string`, and `unnest` can operate on them directly.

---

## Example queries (DuckDB)

### Count concepts by hierarchy

```bash
duckdb -c "
  SELECT hierarchy, COUNT(*) n
  FROM 'snomed.parquet'
  GROUP BY hierarchy
  ORDER BY n DESC"
```

### Find a concept by preferred term

```bash
duckdb -c "
  SELECT id, preferred_term, hierarchy
  FROM 'snomed.parquet'
  WHERE preferred_term ILIKE '%myocardial infarction%'"
```

### Concepts with a specific attribute (using JSON functions)

```bash
duckdb -c "
  SELECT id, preferred_term
  FROM 'snomed.parquet'
  WHERE json_extract_string(attributes, '$.finding_site') IS NOT NULL
  LIMIT 10"
```

### Concepts modified in a given release

```bash
duckdb -c "
  SELECT preferred_term, effective_time
  FROM 'snomed.parquet'
  WHERE effective_time = '20260301'
  ORDER BY preferred_term
  LIMIT 20"
```

### Export a hierarchy to CSV

```bash
duckdb -c "
  COPY (
    SELECT id, preferred_term, fsn
    FROM 'snomed.parquet'
    WHERE hierarchy = 'Procedure'
    ORDER BY preferred_term
  ) TO 'procedures.csv' (HEADER, DELIMITER ',')"
```

### Load into a Python dataframe (Polars)

```python
import polars as pl
df = pl.read_parquet("snomed.parquet")
df.filter(pl.col("hierarchy") == "Clinical finding").head(10)
```

### Load into a Python dataframe (pandas)

```python
import pandas as pd
df = pd.read_parquet("snomed.parquet")
df[df.hierarchy == "Procedure"].preferred_term.head(20)
```

---

## Tips

- DuckDB reads Parquet files in-place with zero import overhead — just reference the file path directly in queries.
- The Parquet file is ~250 MB for the full UK Monolith (vs ~1.2 GB NDJSON), owing to columnar compression.
- For analytics workloads, Parquet is faster than SQLite; for FTS and exact lookups, prefer `sct sqlite`.
