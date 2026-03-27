# sct

A local-first SNOMED CT toolchain. Converts an RF2 Snapshot release into a canonical NDJSON artefact, then loads it into SQLite, Parquet, or per-concept Markdown — all from a single binary with subcommands.

This is the reference implementation of the [SNOMED local-first toolchain](spec.md). The NDJSON artefact is the stable interface between the build stage and all downstream consumers.

---

## Why

SNOMED CT is distributed as a set of tab-separated RF2 files that require joining across multiple tables to get anything useful. The entire healthcare industry relies on remote terminology servers to do this work — with the added overhead of TLS handshakes and REST APIs. `sct` performs the join once, deterministically, and writes the result to a flat file you can grep, commit to git-lfs, and pass to any downstream tool without running a server.

---

## Quick start

```bash
# 1. Install
cargo install --path sct

# 2. Convert RF2 → NDJSON (one-time, ~10s for 831k concepts)
sct ndjson --rf2 ~/.downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/

# 3. Load into SQLite with FTS5
sct sqlite --input snomedct-monolithrf2-production-20260311t120000z.ndjson

# 4. Query with standard tools — no custom binary needed
sqlite3 snomed.db "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 5"

# 5. Start the MCP server (for Claude Desktop / AI tool use)
sct mcp --db snomed.db
```

---

## Subcommands

| Subcommand | Input | Output | Purpose |
|---|---|---|---|
| `sct ndjson` | RF2 Snapshot directory | `.ndjson` file | Convert RF2 to canonical artefact |
| `sct sqlite` | NDJSON file | `snomed.db` | SQLite + FTS5 for queries |
| `sct parquet` | NDJSON file | `snomed.parquet` | Parquet for DuckDB / analytics |
| `sct markdown` | NDJSON file | `snomed-concepts/` directory | Per-concept Markdown for RAG/LLM |
| `sct mcp` | SQLite database | stdio MCP server | AI tool use via Claude Desktop |
| `sct embed` | NDJSON file | LanceDB directory | *(coming soon)* Vector embeddings |

---

## Installation

Requires Rust stable 1.70+: [rustup.rs](https://rustup.rs)

```bash
git clone https://github.com/pacharanero/sct
cd sct
cargo install --path sct
```

Or build without installing:

```bash
cargo build --release --manifest-path sct/Cargo.toml
# Binary at: sct/target/release/sct
```

---

## Getting SNOMED CT

SNOMED CT is licensed. Download the RF2 Snapshot for your region:

- **UK users:** [NHS Digital TRUD](https://isd.digital.nhs.uk/) → SNOMED CT Monolith Edition, RF2: Snapshot. Covered by the NHS England national licence.
- **International users:** [MLDS](https://mlds.ihtsdotools.org/) or [NLM](https://www.nlm.nih.gov/healthit/snomedct/us_edition.html).

For most purposes, **download the Monolith Snapshot** — it contains the international base, UK clinical extension, and dm+d drug extension in a single directory.

---

## `sct ndjson` — RF2 to NDJSON

```
sct ndjson --rf2 <DIR> [--rf2 <DIR>...] [OPTIONS]
```

| Flag | Default | Description |
|---|---|---|
| `--rf2 <DIR>` | *(required)* | RF2 Snapshot directory. Repeat to layer extensions. |
| `--locale <LOCALE>` | `en-GB` | BCP-47 locale for preferred term selection. |
| `--output <FILE>` | *(derived from RF2 dir name)* | Output NDJSON path. Use `-o -` for stdout. |
| `--include-inactive` | off | Include inactive concepts. |

```bash
# UK Monolith (single directory, everything included)
sct ndjson --rf2 ./SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/

# US release
sct ndjson --rf2 ./SnomedCT_USEditionRF2_PRODUCTION_20250301T120000Z/ --locale en-US

# Two-directory UK edition (clinical + drug extension)
sct ndjson \
  --rf2 ./SnomedCT_UKClinicalRF2_PRODUCTION_20250401T000001Z/ \
  --rf2 ./SnomedCT_UKDrugRF2_PRODUCTION_20250401T000001Z/

# Write to stdout
sct ndjson --rf2 ./SnomedCT_Release/ -o - | grep '"22298006"'
```

### Output format

One JSON object per line, sorted by concept SCTID (ascending). Every record includes `schema_version: 1`.

```json
{
  "id": "22298006",
  "fsn": "Myocardial infarction (disorder)",
  "preferred_term": "Heart attack",
  "synonyms": ["Cardiac infarction", "Infarction of heart", "MI - Myocardial infarction"],
  "hierarchy": "Clinical finding",
  "hierarchy_path": ["SNOMED CT Concept", "Clinical finding", "Disorder of cardiovascular system", "Ischemic heart disease", "Myocardial infarction"],
  "parents": [{"id": "414795007", "fsn": "Ischemic heart disease (disorder)"}],
  "children_count": 47,
  "active": true,
  "module": "900000000000207008",
  "effective_time": "20020131",
  "attributes": {
    "finding_site": [{"id": "302509004", "fsn": "Entire heart (body structure)"}],
    "associated_morphology": [{"id": "55641003", "fsn": "Infarct (morphologic abnormality)"}]
  },
  "schema_version": 1
}
```

### Querying with jq

```bash
# Look up a concept by name
jq 'select(.preferred_term | test("myocardial infarction"; "i"))' snomed.ndjson \
  | head -1 | jq '{id, preferred_term, hierarchy, synonyms}'

# All direct children of a concept
jq 'select(.parents[].id == "22298006") | .preferred_term' snomed.ndjson

# Hierarchy path for a concept
jq 'select(.id == "22298006") | .hierarchy_path' snomed.ndjson

# Count by top-level hierarchy
jq -r '.hierarchy' snomed.ndjson | sort | uniq -c | sort -rn | head -10

# Concepts with a specific attribute
jq 'select(.attributes.finding_site != null) | {id, preferred_term}' snomed.ndjson
```

---

## `sct sqlite` — NDJSON to SQLite

```
sct sqlite --input <NDJSON> [--output <DB>]
```

| Flag | Default | Description |
|---|---|---|
| `--input <FILE>` | *(required)* | NDJSON file or `-` for stdin. |
| `--output <FILE>` | `snomed.db` | Output SQLite database path. |

```bash
sct sqlite --input snomed.ndjson --output snomed.db
```

### Schema

```sql
concepts(id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
         parents, children_count, attributes, active, module, effective_time, schema_version)

concept_isa(child_id, parent_id)   -- indexed for fast IS-A traversal

concepts_fts USING fts5(id, preferred_term, synonyms, fsn,
                        content='concepts', content_rowid='rowid')
```

Array/object columns (`synonyms`, `hierarchy_path`, `parents`, `attributes`) are JSON strings, queryable with `json_extract()` and `json_each()`.

### Example queries

```bash
# Free-text search
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 10"

# Exact concept lookup
sqlite3 snomed.db \
  "SELECT id, preferred_term, hierarchy FROM concepts WHERE id = '22298006'"

# All concepts in a hierarchy
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts WHERE hierarchy = 'Procedure' LIMIT 20"

# Children of a concept
sqlite3 snomed.db \
  "SELECT c.id, c.preferred_term FROM concepts c
   JOIN concept_isa ci ON ci.child_id = c.id
   WHERE ci.parent_id = '22298006'"

# Count by hierarchy
sqlite3 snomed.db \
  "SELECT hierarchy, COUNT(*) n FROM concepts GROUP BY hierarchy ORDER BY n DESC LIMIT 10"
```

---

## `sct parquet` — NDJSON to Parquet

```
sct parquet --input <NDJSON> [--output <PARQUET>]
```

| Flag | Default | Description |
|---|---|---|
| `--input <FILE>` | *(required)* | NDJSON file or `-` for stdin. |
| `--output <FILE>` | `snomed.parquet` | Output Parquet file path. |

```bash
sct parquet --input snomed.ndjson --output snomed.parquet
```

DuckDB can query the file directly without any import step:

```bash
# Count by hierarchy
duckdb -c "SELECT hierarchy, COUNT(*) n FROM 'snomed.parquet' GROUP BY hierarchy ORDER BY n DESC"

# Search by preferred term
duckdb -c "SELECT id, preferred_term FROM 'snomed.parquet' WHERE preferred_term ILIKE '%myocardial%'"

# Concepts active since a given release
duckdb -c "SELECT preferred_term FROM 'snomed.parquet' WHERE effective_time = '20260301'"
```

---

## `sct markdown` — NDJSON to Markdown

```
sct markdown --input <NDJSON> [--output <DIR>]
```

| Flag | Default | Description |
|---|---|---|
| `--input <FILE>` | *(required)* | NDJSON file or `-` for stdin. |
| `--output <DIR>` | `snomed-concepts` | Output directory. |

```bash
sct markdown --input snomed.ndjson --output snomed-concepts/
```

Output structure:

```
snomed-concepts/
  clinical-finding/22298006.md
  procedure/173171007.md
  substance/372687004.md
  ...
```

Each file is human-readable and LLM-friendly, designed for RAG indexing and direct file reading via filesystem MCP tools:

```bash
grep -r "heart attack" snomed-concepts/ -l
rg "22298006" snomed-concepts/
```

---

## `sct mcp` — MCP Server

```
sct mcp --db <SQLITE_DB>
```

Starts a local MCP server over stdio (JSON-RPC 2.0, Content-Length framed, protocol version 2024-11-05) backed by the SQLite database from `sct sqlite`.

### Tools

| Tool | Arguments | Description |
|---|---|---|
| `snomed_search` | `query`, `limit` | FTS5 free-text search. Returns id, preferred_term, fsn, hierarchy. |
| `snomed_concept` | `id` | Full detail for a concept by SCTID. |
| `snomed_children` | `id`, `limit` | Immediate IS-A children of a concept. |
| `snomed_ancestors` | `id` | Full ancestor chain from concept to root. |
| `snomed_hierarchy` | `hierarchy`, `limit` | All concepts in a named top-level hierarchy. |

### Claude Desktop configuration

```json
{
  "mcpServers": {
    "snomed": {
      "command": "sct",
      "args": ["mcp", "--db", "/path/to/snomed.db"]
    }
  }
}
```

Config file location:
- **macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Linux:** `~/.config/Claude/claude_desktop_config.json`

---

## Which TRUD download to use

| TRUD item | Use it? | Notes |
|---|---|---|
| **Monolith Edition, RF2: Snapshot** | ✅ Recommended | International + UK clinical + dm+d in one directory. |
| **Clinical Edition, RF2: Full, Snapshot & Delta** | ✅ Works | Only Snapshot files are used; Full and Delta ignored. |
| **Drug Extension, RF2: Full, Snapshot & Delta** | ⚠️ Supplement | Use as second `--rf2` alongside Clinical Edition. |
| **Clinical Edition, RF2: Delta** | ❌ Won't work | No Snapshot files. |
| **Cross-map Historical Files** | ❌ Not needed | Ignored by `sct`. |

---

## Determinism

Given the same RF2 Snapshot directory and `--locale`, `sct ndjson` always produces byte-for-byte identical output:

```bash
sha256sum snomed-uk-20260311.ndjson
```

The NDJSON artefact can be checksummed, committed to git-lfs, and used as a pinned dependency.

---

## See also

- [spec.md](spec.md) — full technical specification
- [roadmap.md](roadmap.md) — implementation progress
- [BENCHMARKS.md](BENCHMARKS.md) — timing measurements on real data
