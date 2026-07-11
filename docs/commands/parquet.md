# sct parquet

Export a SNOMED CT NDJSON artefact to a Parquet file, directly queryable by DuckDB without any import step.

**When to use:** analytics, data science, columnar queries, or loading into pandas/Polars/R. For FTS and exact lookups, [`sct sqlite`](sqlite.md) is better.

---

## Usage

```
sct parquet --ndjson <NDJSON> [--output <PARQUET>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--ndjson <FILE>` | *(required)* | NDJSON file produced by `sct ndjson`. Use `-` for stdin. Accepts `--input` as an alias. |
| `--output <FILE>` | `snomed.parquet` | Output Parquet file path. |

---

## Example

```bash
sct parquet \
  --ndjson snomedct-monolithrf2-production-20260311t120000z.ndjson \
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

`sct parquet` carries over the core concept fields only - `ctv3_codes`, `read2_codes`, `refsets`, `relationships`, and `crossmaps` from the NDJSON record are not exported as columns. For crossmap lookups or ECL attribute refinement, use [`sct sqlite`](sqlite.md) instead.

Array/object columns are stored as JSON strings. DuckDB's `json_extract`, `json_extract_string`, and `unnest` can operate on them directly.

---

## Example queries (DuckDB)

Queries are ordered from simple to complex. For context on the JSON array columns
(`synonyms`, `hierarchy_path`, `parents`, `attributes`) - they are stored as
`VARCHAR` JSON strings, so use DuckDB's `json_extract`, `json_array_length`, and
`from_json` rather than list functions.

---

### Lookup and filter

#### Find a concept by preferred term

```bash
duckdb -c "
  SELECT id, preferred_term, hierarchy
  FROM 'snomed.parquet'
  WHERE preferred_term ILIKE '%myocardial infarction%'"
```

#### Concepts with a specific attribute present

```bash
duckdb -c "
  SELECT id, preferred_term
  FROM 'snomed.parquet'
  WHERE json_extract_string(attributes, '$.finding_site') IS NOT NULL
  LIMIT 10"
```

#### Concepts modified in a given release

```bash
duckdb -c "
  SELECT preferred_term, effective_time
  FROM 'snomed.parquet'
  WHERE effective_time = '20260311'
  ORDER BY preferred_term
  LIMIT 20"
```

#### Export a hierarchy to CSV

```bash
duckdb -c "
  COPY (
    SELECT id, preferred_term, fsn
    FROM 'snomed.parquet'
    WHERE hierarchy = 'Procedure'
    ORDER BY preferred_term
  ) TO 'procedures.csv' (HEADER, DELIMITER ',')"
```

---

### Aggregates and distributions

#### Concept count by top-level hierarchy

```bash
duckdb -c "
  SELECT hierarchy, COUNT(*) AS n
  FROM 'snomed.parquet'
  WHERE active = true
  GROUP BY hierarchy
  ORDER BY n DESC"
```

#### Leaf concepts per hierarchy

Leaf concepts have no children - the most specific, fully-refined terms in the
polyhierarchy.

```bash
duckdb -c "
  SELECT hierarchy, COUNT(*) AS leaf_count
  FROM 'snomed.parquet'
  WHERE children_count = 0 AND active = true
  GROUP BY hierarchy
  ORDER BY leaf_count DESC"
```

#### Release timeline - how many concepts were last updated per release date

```bash
duckdb -c "
  SELECT effective_time, COUNT(*) AS n
  FROM 'snomed.parquet'
  WHERE active = true
  GROUP BY effective_time
  ORDER BY effective_time DESC
  LIMIT 20"
```

#### Synonym count histogram

Shows how terminology richness is distributed across the concept space.

```bash
duckdb -c "
  SELECT json_array_length(synonyms) AS n_synonyms, COUNT(*) AS concepts
  FROM 'snomed.parquet'
  WHERE active = true
  GROUP BY n_synonyms
  ORDER BY n_synonyms"
```

#### Hierarchy depth profile

`hierarchy_path` is a JSON array of ancestor labels from root to concept.
Its length is the concept's depth in the polyhierarchy.

```bash
duckdb -c "
  SELECT hierarchy,
         ROUND(AVG(json_array_length(hierarchy_path)), 1) AS avg_depth,
         MAX(json_array_length(hierarchy_path)) AS max_depth
  FROM 'snomed.parquet'
  WHERE active = true
  GROUP BY hierarchy
  ORDER BY avg_depth DESC"
```

---

### JSON expansion

#### Concepts with the most synonyms

```bash
duckdb -c "
  SELECT id, preferred_term, json_array_length(synonyms) AS n_synonyms
  FROM 'snomed.parquet'
  WHERE active = true
  ORDER BY n_synonyms DESC
  LIMIT 10"
```

#### Concepts with the most parents (deepest polyhierarchy membership)

Each entry in `parents` is a direct IS-A parent. High parent counts indicate
concepts that sit at the intersection of multiple classification axes.

```bash
duckdb -c "
  SELECT id, preferred_term, json_array_length(parents) AS parent_count
  FROM 'snomed.parquet'
  WHERE active = true
  ORDER BY parent_count DESC
  LIMIT 10"
```

#### Expand synonyms to one row each

`from_json` parses the JSON array string into a typed list; `unnest` explodes it
into individual rows.

```bash
duckdb -c "
  SELECT id, preferred_term,
         unnest(from_json(synonyms, '[\"VARCHAR\"]')) AS synonym
  FROM 'snomed.parquet'
  WHERE active = true
    AND json_array_length(synonyms) > 0
    AND preferred_term ILIKE '%myocardial infarction%'
  ORDER BY id, synonym"
```

---

### Window functions

#### Most common finding sites across Clinical findings, with share percentage

```bash
duckdb -c "
  WITH sites AS (
    SELECT
      json_extract_string(attributes, '$.finding_site[0].fsn') AS site,
      COUNT(*) AS n,
      ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 2) AS pct
    FROM 'snomed.parquet'
    WHERE json_extract(attributes, '$.finding_site') IS NOT NULL
      AND active = true
    GROUP BY site
  )
  SELECT site, n, pct
  FROM sites
  ORDER BY n DESC
  LIMIT 15"
```

#### Top 2 most-synonymised concepts per hierarchy

`QUALIFY` filters rows after window functions are evaluated - equivalent to
wrapping in a subquery and filtering on the ranked result, but more concise.

```bash
duckdb -c "
  SELECT id, preferred_term, hierarchy,
         json_array_length(synonyms) AS n_synonyms,
         RANK() OVER (
           PARTITION BY hierarchy
           ORDER BY json_array_length(synonyms) DESC
         ) AS rank_in_hierarchy
  FROM 'snomed.parquet'
  WHERE active = true
  QUALIFY rank_in_hierarchy <= 2
  ORDER BY hierarchy, rank_in_hierarchy"
```

---

### Load into Python

```python
import polars as pl
df = pl.read_parquet("snomed.parquet")
df.filter(pl.col("hierarchy") == "Clinical finding").head(10)
```

```python
import pandas as pd
df = pd.read_parquet("snomed.parquet")
df[df.hierarchy == "Procedure"].preferred_term.head(20)
```

---

## Tips

- DuckDB reads Parquet files in-place with zero import overhead - just reference the file path directly in queries.
- The Parquet file is ~785 MB for the full UK Monolith (vs ~1.2 GB NDJSON), owing to columnar compression.
- Written in batches of 50,000 rows using Arrow for memory efficiency.
- For analytics workloads, Parquet is faster than SQLite; for FTS and exact lookups, prefer [`sct sqlite`](sqlite.md).