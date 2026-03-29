# `sct` Walkthrough — Feature Tour

A hands-on tour of the `sct` SNOMED CT local-first toolchain. Each section maps to a
distinct feature and is designed as a self-contained demo scene.

---

## 0 — What is `sct`?

`sct` is a single Rust binary that transforms a SNOMED CT RF2 release into a set of
queryable, offline-first artefacts. No server required. No licence server. No Java.

```
SNOMED RF2 release
        │
        ▼
   sct ndjson          ← build once per release (~30 s for 831k concepts)
        │
        ├──▶ sct sqlite   → snomed.db                SQL + full-text search
        ├──▶ sct parquet  → snomed.parquet            analytics with DuckDB / pandas
        ├──▶ sct markdown → snomed-concepts/          one file per concept (RAG)
        └──▶ sct embed    → snomed-embeddings.arrow   semantic vector search
                                  │
                            sct mcp                   AI tool use via Claude
```

**Key guarantees:**
- Offline at query time (Ollama only needed for embeddings)
- Deterministic — same RF2 + locale always produces identical output
- Single portable file for each artefact

---

## 1 — Installation

```bash
cargo install sct
```

Or download a pre-built binary from GitHub Releases.

Verify:

```bash
sct --version
# sct 0.3.0
```

Generate shell completions:

```bash
sct completions bash > ~/.local/share/bash-completion/completions/sct
# also: zsh, fish, powershell, elvish
```

---

## 2 — Getting SNOMED RF2 Data

SNOMED CT is distributed as RF2 (Release Format 2) — a set of TSV files.

- **UK edition (recommended):** Download the UK Monolith from [NHS TRUD](https://isd.digital.nhs.uk/trud)
  - Includes: International release + UK clinical extension + dm+d drugs extension
- **International edition:** Download from [SNOMED MLDS](https://mlds.ihtsdotools.org/)
- **IPS Free Set:** Available without affiliate membership from SNOMED MLDS

`sct` accepts ZIP files or extracted directories.

---

## 3 — Layer 1: Build the Canonical Artefact

The first step is always `sct ndjson`. This joins the RF2 tables and produces the
canonical intermediate artefact that everything else is built from.

```bash
# Single release (International)
sct ndjson --rf2 SnomedCT_InternationalRF2_PRODUCTION_*.zip \
           --output snomed-20250301.ndjson

# UK edition: layer International + UK Clinical + dm+d
sct ndjson --rf2 SnomedCT_InternationalRF2_PRODUCTION_*.zip \
           --rf2 SnomedCT_UKClinicalRF2_PRODUCTION_*.zip \
           --rf2 SnomedCT_UKDrugRF2_PRODUCTION_*.zip \
           --locale en-GB \
           --output snomed-uk-20250301.ndjson
```

**Timing:**
| Edition | Concepts | Time |
|---|---|---|
| UK Clinical only | 34k | ~0.8 s |
| UK Monolith (all) | 831k | ~30 s |

**What you get:** One `.ndjson` file. One JSON object per line. Each concept looks like:

```json
{
  "id": "22298006",
  "fsn": "Myocardial infarction (disorder)",
  "preferred_term": "Heart attack",
  "synonyms": ["Cardiac infarction", "Infarction of heart", "MI - Myocardial infarction"],
  "hierarchy": "Clinical finding",
  "hierarchy_path": ["SNOMED CT concept", "Clinical finding", "Disorder of cardiovascular system",
                     "Ischemic heart disease", "Myocardial infarction"],
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

The NDJSON artefact is the stable interface. Version-controlled. Copyable. Diffable.

```bash
# Inspect with standard tools — no custom binary needed
grep '"hierarchy":"Procedure"' snomed.ndjson | wc -l
jq '.preferred_term' snomed.ndjson | head -5
```

---

## 4 — Layer 2a: SQLite + Full-Text Search

Load the NDJSON artefact into a SQLite database with FTS5 full-text search.

```bash
sct sqlite --input snomed-uk-20250301.ndjson --output snomed.db
# ~11 s for 831k concepts → 1.3 GB SQLite file
```

**Query with standard `sqlite3`:**

```bash
# Free-text search (FTS5)
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 5"

# Direct concept lookup
sqlite3 snomed.db \
  "SELECT preferred_term, json(attributes) FROM concepts WHERE id = '22298006'"

# Browse by hierarchy
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts WHERE hierarchy = 'Procedure' LIMIT 10"

# Recursive subsumption via IS-A table
sqlite3 snomed.db "
  WITH RECURSIVE descendants(id) AS (
    SELECT child_id FROM concept_isa WHERE parent_id = '22298006'
    UNION ALL
    SELECT ci.child_id FROM concept_isa ci JOIN descendants d ON ci.parent_id = d.id
  )
  SELECT c.preferred_term FROM concepts c JOIN descendants d ON c.id = d.id LIMIT 20"
```

**Or use `sct lexical` directly:**

```bash
sct lexical --db snomed.db --query "heart attack" --limit 10
sct lexical --db snomed.db --query "diabetes" --hierarchy "Clinical finding"
sct lexical --db snomed.db --query "amox*"      # prefix search
```

The `.db` file is a single portable file. Copy it, version it with git-lfs, share it via
scp. No installation required at query time — `sqlite3` is sufficient.

---

## 5 — Layer 2b: Parquet for Analytics

Export to Apache Parquet for use with DuckDB, pandas, Polars, R, or Spark.

```bash
sct parquet --input snomed-uk-20250301.ndjson --output snomed.parquet
# ~5 s for 831k concepts → 824 MB
```

**Query with DuckDB:**

```bash
duckdb -c "
  SELECT hierarchy, COUNT(*) AS n
  FROM 'snomed.parquet'
  GROUP BY hierarchy
  ORDER BY n DESC
  LIMIT 10"
```

```bash
duckdb -c "
  SELECT id, preferred_term, hierarchy_path
  FROM 'snomed.parquet'
  WHERE list_contains(synonyms, 'BP')
  LIMIT 5"
```

**Python / pandas:**

```python
import pandas as pd
df = pd.read_parquet("snomed.parquet")
procedures = df[df["hierarchy"] == "Procedure"]
print(procedures["preferred_term"].head(10))
```

---

## 6 — Layer 2c: Markdown Export for RAG

Export SNOMED CT as a directory of Markdown files — one per concept. Ideal for
retrieval-augmented generation (RAG), Claude Code file reading, or filesystem MCP.

```bash
sct markdown --input snomed-uk-20250301.ndjson --output ./snomed-concepts/
# ~14.5 s → 831k .md files, 3.2 GB total
```

**Example output** (`snomed-concepts/clinical-finding/22298006.md`):

```markdown
# Heart attack
**SCTID:** 22298006
**FSN:** Myocardial infarction (disorder)
**Hierarchy:** Clinical finding > Disorder of cardiovascular system > Ischemic heart disease

## Synonyms
- Cardiac infarction
- Infarction of heart
- MI - Myocardial infarction

## Relationships
- **Finding site:** Entire heart (body structure) [302509004]
- **Associated morphology:** Infarct (morphologic abnormality) [55641003]

## Parents
- Ischemic heart disease [414795007]
```

**Hierarchy-mode** (one file per top-level hierarchy, ~19 files):

```bash
sct markdown --input snomed.ndjson --output ./snomed-hierarchies/ --mode hierarchy
```

Files can be read directly by Claude Code, indexed by your own RAG pipeline, or
searched with `ripgrep`:

```bash
rg "finding_site.*heart" snomed-concepts/ -l | head -5
```

---

## 7 — Layer 3: Vector Embeddings

Generate dense vector embeddings for semantic (nearest-neighbour) search.
Requires [Ollama](https://ollama.ai) running locally.

```bash
# Pull an embedding model
ollama pull nomic-embed-text

# Generate embeddings (streams to Arrow IPC file)
sct embed --input snomed.ndjson \
          --output snomed-embeddings.arrow \
          --model nomic-embed-text
```

Each concept is embedded using a rich text template:

```
"Heart attack. Myocardial infarction (disorder).
 Synonyms: Cardiac infarction, Infarction of heart, MI.
 Hierarchy: SNOMED CT concept > Clinical finding > ... > Myocardial infarction"
```

The Arrow IPC file can be queried in DuckDB or PyArrow, and is the input for
`sct semantic`.

---

## 8 — Semantic Search

Find conceptually similar concepts using cosine similarity over embeddings.
No keyword match needed.

```bash
sct semantic --embeddings snomed-embeddings.arrow \
             --query "blocked coronary artery" \
             --limit 5
```

Example output:

```
1. Myocardial infarction                22298006   similarity: 0.934
2. Coronary artery occlusion            44771008   similarity: 0.921
3. Acute coronary syndrome             394659003   similarity: 0.908
4. Ischemic heart disease              414795007   similarity: 0.897
5. Coronary artery atherosclerosis      53741008   similarity: 0.881
```

Semantic search finds concepts even when the exact terms don't match — useful for
natural-language queries, typos, and synonym gaps.

---

## 9 — Layer 4: MCP Server for Claude

Expose SNOMED CT as a set of tools in Claude Desktop or Claude Code via the
Model Context Protocol.

```bash
sct mcp --db snomed.db
# Starts stdio MCP server; add to Claude Desktop config
```

**Claude Desktop configuration** (`~/Library/Application Support/Claude/claude_desktop_config.json`):

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

**Tools available to Claude:**

| Tool | Description |
|---|---|
| `snomed_search` | Free-text search — returns top matching concepts |
| `snomed_concept` | Full concept detail by SCTID |
| `snomed_children` | Immediate IS-A children of a concept |
| `snomed_ancestors` | Full ancestor chain to SNOMED root |
| `snomed_hierarchy` | All concepts within a top-level hierarchy |

**Example Claude interaction:**

> "What are the subtypes of type 2 diabetes mellitus?"

Claude calls `snomed_children` with SCTID `44054006`, receives the list, and answers
with accurate SNOMED-grounded terminology.

**MCP server properties:**
- Startup time < 5 ms (well under the 100 ms MCP budget)
- Read-only and stateless
- Dual-mode transport: supports both Claude Desktop (Content-Length framing) and
  Claude Code 2.1.86+ (newline-delimited JSON)
- Schema version validation on startup

---

## 10 — Interactive UIs

### Terminal UI (`--features tui`)

```bash
sct tui --db snomed.db
```

Three-panel layout:
- **Top-left:** Hierarchy browser
- **Bottom-left:** Search box + results
- **Right:** Full concept detail

Keybindings: `/` search, `Tab` switch panels, `↑↓` navigate, `Enter` select, `q` quit.

### Browser UI (`--features gui`)

```bash
sct gui --db snomed.db
# Opens http://127.0.0.1:8420 in your browser
```

Single-page app with search, hierarchy browsing, and concept detail view.
Bound to localhost only — never accessible from the network.

---

## 11 — Release Comparison: `sct diff`

Compare two NDJSON artefacts to see what changed between SNOMED releases.

```bash
sct diff --old snomed-uk-20240901.ndjson \
         --new snomed-uk-20250301.ndjson \
         --output summary
```

Reports:
- Concepts added
- Concepts inactivated
- Terms changed (preferred term or FSN updated)
- Hierarchy changed (concept moved in IS-A tree)

```bash
# Machine-readable NDJSON output for scripting
sct diff --old old.ndjson --new new.ndjson --output ndjson | \
  jq 'select(.change_type == "term_changed")'
```

---

## 12 — Artefact Inspection: `sct info`

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

## 13 — Performance

All timings below are for the **UK Monolith (831k active concepts)** on NVMe SSD.

| Operation | Time | Output size |
|---|---|---|
| RF2 → NDJSON | ~30 s | ~1.1 GB |
| NDJSON → SQLite | ~11 s | 1.3 GB |
| NDJSON → Parquet | ~5 s | 824 MB |
| NDJSON → Markdown | ~15 s | 3.2 GB (831k files) |
| MCP server startup | < 5 ms | — |

**vs. remote FHIR terminology server (benchmark results):**

Local SQLite queries are **50–2700× faster** than equivalent FHIR R4 operations over the
network. See `docs/benchmarks.md` for full methodology and results.

Run the benchmarking suite yourself:

```bash
bench/bench.sh \
  --server https://your-fhir-server/fhir \
  --db snomed.db \
  --runs 10 \
  --format table
```

---

## 14 — UK Clinical Edition: Layered Builds

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

## 15 — Command Reference Summary

| Command | Description |
|---|---|
| `sct ndjson` | RF2 → canonical NDJSON (build once per release) |
| `sct sqlite` | NDJSON → SQLite + FTS5 (SQL + full-text search) |
| `sct parquet` | NDJSON → Parquet (DuckDB / analytics) |
| `sct markdown` | NDJSON → Markdown files (RAG / file reading) |
| `sct embed` | NDJSON → Arrow embeddings (requires Ollama) |
| `sct mcp` | Stdio MCP server for Claude (wraps SQLite) |
| `sct lexical` | Keyword search via FTS5 |
| `sct semantic` | Semantic search via cosine similarity |
| `sct diff` | Compare two NDJSON releases |
| `sct info` | Inspect any sct-produced artefact |
| `sct tui` | Terminal UI (requires `--features tui`) |
| `sct gui` | Browser UI (requires `--features gui`) |
| `sct completions` | Generate shell completion scripts |
| `sct codelist` | Build, validate, publish code lists *(planned)* |

---

## Next Steps

- `sct codelist` — clinical code list management (spec complete, implementation in progress)
- `sct trud` — automated download from NHS TRUD API
- `sct serve` — drop-in FHIR R4/R5 terminology server backed by SQLite
- Semantic search via `snomed_semantic_search` MCP tool (combining embed + mcp)

See `specs/roadmap.md` for the full list of planned features.
