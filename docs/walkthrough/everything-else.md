# Everything Else

Release comparison, artefact inspection, performance benchmarks, layered builds,
and the full command reference.

---

## Release Comparison `experimental` :lucide-test-tube

Compare two NDJSON artefacts to see what changed between SNOMED releases.

```bash
sct diff --old snomed-uk-20240901.ndjson \
         --new snomed-uk-20250301.ndjson \
         --format text
```

Reports:

- Concepts added
- Concepts inactivated
- Terms changed (preferred term or FSN updated)
- Hierarchy changed (concept moved in IS-A tree)

```bash
# Machine-readable NDJSON output for scripting
sct diff --old old.ndjson --new new.ndjson --format ndjson | \
  jq 'select(.change_type == "term_changed")'
```

---

## Artefact Inspection `experimental!` :lucide-test-tube:

Inspect any `sct`-produced file without needing to know its internals.

```bash
sct info snomed.ndjson
sct info snomed.db
sct info snomed-embeddings.arrow
```

Output includes:

- Concept count
- Schema version
- Hierarchy breakdown (concept counts per top-level hierarchy)
- File size
- Release date (if present)

---

## Performance

All timings below are for the **UK Monolith (837,930 active concepts)** on NVMe SSD.

| Operation | Time | Output size |
|---|---|---|
| RF2 → NDJSON | ~52 s | 1.3 GB |
| NDJSON → SQLite | ~32 s | 1.9 GB |
| NDJSON → Parquet | ~6 s | 785 MB |
| NDJSON → Markdown | ~32 s | 3.2 GB (837,930 files) |
| MCP server startup (v0.18.2 pre-SDK baseline) | ~2.3 ms; current `rmcp` path awaiting remeasurement | - |

**vs. remote FHIR terminology server (benchmark results):**

Local SQLite queries are **50–2700× faster** than equivalent FHIR R4 operations over the
network. See `benchmarks.md` for full methodology and results.

Run the benchmarking suite yourself:

```bash
benchmarks/bench.sh \
  --server https://your-fhir-server/fhir \
  --db snomed.db \
  --runs 10 \
  --format table
```

---

## UK Clinical Edition: Layered Builds

The UK SNOMED CT Clinical Edition is built by layering three RF2 releases:

```bash
sct ndjson \
  --rf2 SnomedCT_InternationalRF2_PRODUCTION_20250101T120000Z.zip \
  --rf2 SnomedCT_UKClinicalRF2_PRODUCTION_20250401T000001Z.zip \
  --rf2 SnomedCT_UKDrugRF2_PRODUCTION_20250401T000001Z.zip \
  --locale en-GB \
  --output snomed-uk-20250401.ndjson
```

Later `--rf2` flags override earlier ones for the same concept. The `--locale en-GB`
flag selects GB English preferred terms from the UK language reference set.

---

## Command Reference Summary

| Command | Description |
|---|---|
| `sct ndjson` | RF2 → canonical NDJSON (build once per release) |
| `sct sqlite` | NDJSON → SQLite + FTS5 (SQL + full-text search) |
| `sct tct` | Add transitive closure table to an existing SQLite database |
| `sct parquet` | NDJSON → Parquet (DuckDB / analytics) |
| `sct markdown` | NDJSON → Markdown files (RAG / file reading) |
| `sct embed` | NDJSON → Arrow embeddings (requires Ollama) |
| `sct mcp` | Stdio MCP server for Claude (wraps SQLite) |
| `sct lexical` | Keyword search via FTS5 |
| `sct semantic` | Semantic search via cosine similarity |
| `sct diff` | Compare two NDJSON releases |
| `sct info` | Inspect any sct-produced artefact |
| `sct tui` | Terminal UI (in the default build) |
| `sct gui` | Browser UI (requires `--features gui`) |
| `sct completions` | Generate shell completion scripts |
| `sct codelist` | Build and validate code lists (alias: `sct valueset`) |
| `sct refset` | Browse reference sets loaded into the SQLite database |
| `sct lookup` | Look up a single concept by SCTID or CTV3 code |
| `sct ecl` | Evaluate an ECL expression (`expand`) or refactor SCTIDs into ECL (`compress`) |
| `sct diagram` | Draw a concept's definition, ancestors, or descendants (tree/DOT/Mermaid) |
| `sct fst` | Build and query an FST-backed lexical index (exact/prefix/fuzzy/word search) |
| `sct map` | Map codes between terminologies (SNOMED/Read v2/CTV3/ICD-10/OPCS-4) |
| `sct read2` | Import final Read v2 maps from NHS Data Migration TRUD item 9 |

---

## Next Steps

- `sct trud` - automated download from NHS TRUD API
- `sct serve` - drop-in FHIR R4 terminology server backed by SQLite
- `sct codelist publish` - publish back to OpenCodelists (coming)

See `spec/roadmap.md` for the full list of planned features.
