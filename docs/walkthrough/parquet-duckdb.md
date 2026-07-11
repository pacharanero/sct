# Parquet and DuckDB

Export SNOMED CT to Apache Parquet for analytics with DuckDB, pandas, Polars, R, or Spark.

---

## Parquet for Analytics

> **Docs**: [`sct parquet`](../commands/parquet.md)

```bash
sct parquet --ndjson snomed-uk-20250301.ndjson --output snomed.parquet

# ~6 s for 838k concepts → 785 MB
```

### Query with DuckDB

Install DuckDB: <https://duckdb.org/install/>

Then run queries directly on the Parquet file:

```bash
duckdb -c "
  SELECT hierarchy, COUNT(*) AS n
  FROM 'snomed.parquet'
  GROUP BY hierarchy
  ORDER BY n DESC
  LIMIT 10"
```

> **Docs**: For more DuckDB examples, see the [`sct parquet` documentation](../commands/parquet.md)
