# `sct` Walkthrough

A hands-on tour of the `sct` SNOMED-CT local-first toolchain.

---

## 0 — What is `sct`?

`sct` is a single Rust binary that transforms a SNOMED CT RF2 release into a set of
queryable, offline-first artefacts. No server required. No bloody Java.

It was initially created as an experiment in file-based data handling, offline-first tooling, and learning about the structure of SNOMED, but it turns out it's pretty fast and useful too, so I'm gradually adding features with the aim of creating something genuinely useful for practitioners, informaticians, and researchers working with SNOMED CT.

Data map:

```
SNOMED RF2 release
        │
        ▼
   sct ndjson          ← build once per release (~30 s for 831k concepts)
        │
        ├──▶ sct sqlite   → snomed.db                 SQL + full-text search
        │          │
        │          └──▶ sct tct   → snomed.db (+TCT)  O(1) subsumption queries (optional)
        ├──▶ sct parquet  → snomed.parquet            analytics with DuckDB / pandas
        ├──▶ sct markdown → snomed-concepts/          one file per concept (RAG)
        └──▶ sct embed    → snomed-embeddings.arrow   semantic vector search
                                  │
                            sct mcp                   AI tool use via Claude
```

### Key `sct` design principles

- **Offline** - everything happens on your local machine, no network calls or external servers required
- **Deterministic** - same RF2 + locale always produces identical output files, which can be version-controlled, diffed, and audited.
- **File based** - each artefact is a single portable file (or directory of files) that can be copied, versioned, and used with standard tools. No custom server or API needed at query time.
- **No special tools required** - query the SQLite database with `sqlite3`, do analytics with DuckDB or pandas, search with `jq` or `ripgrep`, read concept details in VSCode or a Markdown viewer, or use the MCP server to integrate with LLMs like Claude.

---

## 1 — Installation

```bash
git clone https://github.com/pacharanero/sct.git
cd sct
cargo install --path . --features "tui gui"
```

We're working on packaging binaries for the usual distribution channels (Homebrew, PyPI, etc.) but for now you need Rust and Cargo to build from source. Feedback in Issues will help us decide which platforms and formats to prioritise for pre-built binaries.

Verify installation:

```bash
sct --version
# sct 0.3.7
```

> Optionally, you can generate [shell completions](commands/completions.md) for your shell at this point.

---

## 2 — Get SNOMED RF2 Data

SNOMED CT is distributed as RF2 (Release Format 2) — a set of TSV files.

### Option A — Automated download with `sct trud` (recommended for UK users)

`sct trud` authenticates with [NHS TRUD](https://isd.digital.nhs.uk/trud), downloads the
correct release zip, verifies its SHA-256 checksum, and can optionally run the full build
pipeline in one command.

**Full details:** [`sct trud`](commands/trud.md)

#### 1. Get your TRUD API key

1. Register or sign in at [isd.digital.nhs.uk/trud](https://isd.digital.nhs.uk/trud/users/guest/filters/0/account/form)
2. Subscribe to the **UK Monolith** (item 1799) — includes International + UK Clinical + UK Drug (dm+d) + UK Pathology, pre-merged
3. Your API key is shown on your [TRUD account page](https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/account/manage)

The key is a plain string. It is tied to your account credentials: if you change your TRUD
email or password, the key is regenerated and the old one stops working immediately.

#### 2. Store the key safely

The conventional location is `~/.config/sct/trud-api-key` — a plain text file with the key
on the first line and no other content:

```bash
mkdir -p ~/.config/sct
echo "your-key-here" > ~/.config/sct/trud-api-key
chmod 600 ~/.config/sct/trud-api-key
```

> **Do not commit this file to version control.** If you accidentally expose the key,
> regenerate it immediately from your TRUD account page.

Alternatively, export it as an environment variable — useful for CI/CD and automation:

```bash
export TRUD_API_KEY=your-key-here
```

#### 3. Download and build

```bash
# Download the latest UK Monolith and immediately build the SQLite database
sct trud download --api-key-file ~/.config/sct/trud-api-key \
                  --edition uk_monolith \
                  --pipeline

# Zip saved to:  ~/.local/share/sct/releases/
# SQLite at:     ~/.local/share/sct/data/uk_sct2mo_…SNAPSHOT.db
```

See [`sct trud`](commands/trud.md) for the full options reference, config file format,
automation examples (launchd, systemd, cron, GitHub Actions), and troubleshooting.

---

### Option B — Manual download

If you prefer to download the zip yourself:

- **UK edition:** [NHS TRUD](https://isd.digital.nhs.uk/trud) — subscribe to item **1799** (UK Monolith, recommended)
  - Includes: International + UK Clinical + UK Drug (dm+d) + UK Pathology, pre-merged
  - Note: item 101 is the Clinical Edition (no dm+d); item 105 is the Drug Extension on its own
- **International edition:** [SNOMED MLDS](https://mlds.ihtsdotools.org/)
- **IPS Free Set:** Available without affiliate membership from SNOMED MLDS

`sct` accepts the zip directly or an already-extracted directory.

> **Confused by the NHS TRUD download options?** See [UK Edition structure](uk-edition-structure.md)
> for a plain-English guide to the different release types, what's in each zip, and how to decode the filenames.

---

## 3 — Build the NDJSON Artefact

The first step is always `sct ndjson`. This joins the RF2 tables and produces the
canonical intermediate artefact that everything else is built from.

**Docs**: [`sct ndjson`](commands/ndjson.md)

```bash
sct ndjson --rf2 .downloads/uk_sct2mo_41.6.0_20260311000001Z.zip \
           --output snomed.ndjson

# ~30 s for 831k concepts → snomed.ndjson (1.1 GB)
```

If you pass it a `.zip` it will automatically extract and parse the RF2 files within. If you pass it a directory containing extracted RF2 files, it will parse them directly.

The output is a single `.ndjson` file — one JSON object per line, each representing a SNOMED concept with all its details (ID, preferred term, synonyms, hierarchy, relationships, attributes, etc.)

Testing on my laptop, this takes about 30 seconds for the UK Monolith release with 831k active concepts. The resulting NDJSON file is about 1.1 GB. Incredibly, because NDJSON is easier to handle in memory than JSON, **you can load the whole 1.1 GB file into VSCode** (takes less than 5 seconds) and play around with it there, great for getting to understand what data is available and how it's structured.

You can now query the NDJSON file with `jq` or any tool that can handle line-delimited JSON. For example, to get the full details of Myocardial infarction (disorder)":

```bash
jq 'select(.id == "22298006")' snomed.ndjson
```

Which should return something similar to the below:

```json
{
  "id": "22298006",
  "fsn": "Myocardial infarction (disorder)",
  "preferred_term": "Myocardial infarction",
  "synonyms": [
    "Infarction of heart",
    "Cardiac infarction",
    "Heart attack",
    "Myocardial infarct",
    "MI - myocardial infarction"
  ],
  "hierarchy": "Clinical finding",
  "hierarchy_path": [
    "SNOMED CT Concept",
    "Clinical finding",
    "Finding of trunk structure",
    "Finding of upper trunk",
    "Finding of thoracic region",
    "Disorder of thorax",
    "Disorder of mediastinum",
    "Heart disease",
    "Structural disorder of heart",
    "Myocardial lesion",
    "Myocardial necrosis",
    "Myocardial infarction"
  ],
  "parents": [
    {
      "id": "251061000",
      "fsn": "Myocardial necrosis (disorder)"
    },
    {
      "id": "414545008",
      "fsn": "Ischemic heart disease (disorder)"
    }
  ],
  "children_count": 14,
  "active": true,
  "module": "900000000000207008",
  "effective_time": "20020131",
  "attributes": {
    "associated_morphology": [
      {
        "id": "55641003",
        "fsn": "Infarct (morphologic abnormality)"
      }
    ],
    "finding_site": [
      {
        "id": "74281007",
        "fsn": "Myocardium structure (body structure)"
      }
    ]
  },
  "ctv3_codes": [
    "X200E"
  ],
  "read2_codes": [],
  "schema_version": 2
}
```

The NDJSON artefact is the stable interface. Version-controlled. Copyable. Diffable. (Remember though, it is still copyright SNOMED International and subject to the SNOMED CT licence terms, so **don't share it publicly**.)

Here are some examples of mini-queries you can run directly on the NDJSON file with `jq` or `grep`:

Get all Procedures:

```bash
grep '"hierarchy":"Procedure"' snomed.ndjson | wc -l
```

Get all concepts with "heart" in the preferred term:

```bash
jq 'select(.preferred_term | test("heart"; "i")) | {id, preferred_term}' snomed.ndjson
```

Get a concept via CTV3 code (UK edition only):

```bash
jq 'select(.ctv3_codes | index("X200E"))' snomed.ndjson
```

### Programmatic access

You can look inside the NDJSON file with any language that can read it eg. Python:

Prints all concepts that have CTV3 codes, with their preferred term and list of CTV3 codes:

```python
import json
with open('snomed.ndjson') as f:
    for line in f:
        rec = json.loads(line)
        if rec['ctv3_codes']:
            print(f"{rec['id']}\t{rec['preferred_term']}\t{rec['ctv3_codes']}")
```

NDJSON is great for quick exploration and ad-hoc queries, but for more complex querying and analytics, the next step is to load it into SQLite or export to Parquet.

---

## 4 — SQLite + Full-Text Search

Load the NDJSON artefact into a SQLite database with FTS5 full-text search.

```bash
sct sqlite --input snomed.ndjson --output snomed.db
```

**Docs**: [`sct sqlite`](commands/sqlite.md)

On my machine this takes about 45 seconds for the UK Monolith release with 831k active concepts. The resulting `snomed.db` file is about 2 GB.

**Now you can query SNOMED CT with standard `sqlite3`:** The following examples should all work out of the box on the resulting database, running in the terminal.

LLMs are excellent at generating SQL queries, so you can also use any LLM to generate custom SQL queries for you on demand. `sct` includes an MCP server that exposes the database as 'tools' to LLMs in a standard way for interactive querying — see below.

Free-text search (FTS5)

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 5"
```

Direct concept lookup

```bash
sqlite3 snomed.db \
  "SELECT preferred_term, json(attributes) FROM concepts WHERE id = '22298006'"
```

Browse by hierarchy

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts WHERE hierarchy = 'Procedure' LIMIT 10"
```

Recursive subsumption via IS-A table

```bash
sqlite3 snomed.db \
  "WITH RECURSIVE descendants(id) AS (
    SELECT DISTINCT child_id FROM concept_isa WHERE parent_id = '22298006'
    UNION
    SELECT ci.child_id FROM concept_isa ci JOIN descendants d ON ci.parent_id = d.id
  )
  SELECT DISTINCT c.preferred_term FROM concepts c JOIN descendants d ON c.id = d.id LIMIT 20"
```

For simple lexical searches, I added a `sct lexical` subcommand that generates SQL queries for you, so you don't have to write the raw SQL yourself. It supports free-text search, hierarchy filtering, and prefix search:

```bash
sct lexical --db snomed.db "heart attack" --limit 10
sct lexical --db snomed.db "diabetes" --hierarchy "Clinical finding"
sct lexical --db snomed.db "amox*"      # prefix search
```

> **:lucide-book-text: Docs**: [`sct lexical`](commands/lexical.md)

> For more advanced and interesting SQL queries, see the [`sct sqlite` documentation](sqlite.md)

---

## 4a — UK Crossmaps: CTV3

**UK edition only.** The CTV3 (Clinical Terms Version 3) crossmaps are available when building from a
UK NHS SNOMED CT release (UK Monolith or UK Clinical Edition from [NHS TRUD](https://isd.digital.nhs.uk/trud)).
They are parsed automatically from the `der2_sRefset_SimpleMap` reference set (refset ID `900000000000497000`).

CTV3 is the legacy NHS terminology used in GP and secondary care systems before SNOMED CT. Having
SNOMED → CTV3 mappings is useful for:

- **Migrating data** from legacy systems that recorded CTV3 codes
- **Interoperability** with older clinical records
- **Reporting** to systems that still consume CTV3
- **Learning and exploration** — see how concepts were mapped from CTV3 to SNOMED CT

Over 524,000 concepts have CTV3 mappings in the UK Monolith release. Read v2 codes are not distributed
as a separate refset in current UK releases.

**Data structure:**

The SQLite database includes:

- `concepts.ctv3_codes` — JSON array of CTV3 codes for each concept
- `concept_maps` table — reverse index for fast CTV3 code → SNOMED lookup

**Example queries:**

Forward: SNOMED → CTV3 code

```bash
sqlite3 snomed.db "SELECT id, preferred_term, ctv3_codes FROM concepts WHERE id = '22298006'"

# 22298006|Myocardial infarction|["X200E"]
```

Reverse: CTV3 code → SNOMED concept

```bash
sqlite3 snomed.db "
  SELECT c.id, c.preferred_term, c.hierarchy
  FROM concepts c
  JOIN concept_maps m ON c.id = m.concept_id
  WHERE m.code = 'X200E' AND m.terminology = 'ctv3'"

# 22298006|Myocardial infarction|Clinical finding
```

---

## 4b — Transitive Closure Table (TCT)

> **Docs**: [`sct tct`](commands/tct.md)

By default, `sct sqlite` stores only direct IS-A parent-child pairs in `concept_isa`. Subsumption queries ("give me all descendants of X") require a recursive CTE at query time. The **transitive closure table** (TCT) precomputes every ancestor-descendant pair in the hierarchy so these queries become a single indexed JOIN.

The TCT is entirely optional. Because it is derived from `concept_isa` — which is already in every `sct sqlite` output — it can be added to any existing database at any time without re-reading the original NDJSON artefact.

### Build the TCT

Apply to an existing database:

```bash
sct tct --db snomed.db
# spinner: Building TCT for 831,132 concepts (5000/831132)...
# Done. 18,432,601 ancestor-descendant pairs in concept_ancestors.
```

Or build it in a single step alongside the main load:

```bash
sct sqlite --input snomed.ndjson --output snomed.db --transitive-closure
```

Both call the same underlying algorithm and produce identical output. The `--transitive-closure` flag is a convenience shorthand for pipelines that want everything in one command.

To include self-referential rows (`depth = 0`, `ancestor_id = descendant_id`) — useful if your queries always want "descendants including self":

```bash
sct tct --db snomed.db --include-self
```

### Verify with `sct info`

```bash
sct info snomed.db
```

Without TCT:

```text
IS-A edges:        504,216
TCT:               not present  (run `sct tct --db <file>` to build)
```

After `sct tct`:

```text
IS-A edges:        504,216
TCT rows:          18,432,601
```

### Performance comparison

The queries below are equivalent — both return all descendants of Myocardial infarction (`22298006`) in the IS-A hierarchy. The TCT version replaces a full recursive tree-walk with a single index seek.

**Without TCT — recursive CTE (~4 ms on UK Monolith):**

```bash
sqlite3 snomed.db <<EOF
.timer on
WITH RECURSIVE descendants(id) AS (
  SELECT child_id FROM concept_isa WHERE parent_id = '22298006'
  UNION
  SELECT ci.child_id FROM concept_isa ci
    JOIN descendants d ON ci.parent_id = d.id
)
SELECT COUNT(*) FROM descendants;
EOF
```

**With TCT — indexed lookup (<1 ms on UK Monolith):**

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT COUNT(*) FROM concept_ancestors WHERE ancestor_id = '22298006';
EOF
```

Both return the same count. The TCT version is faster because the index on `ancestor_id` gives SQLite a direct range scan over a single column, with no recursion.

The performance gap grows sharply with hierarchy depth and fanout. For large ancestors (e.g. `Clinical finding` with ~300k descendants), recursive CTEs can take hundreds of milliseconds; the TCT lookup stays under 1 ms regardless of hierarchy size.

### Full subsumption query with preferred terms

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT c.preferred_term
FROM concepts c
JOIN concept_ancestors a ON c.id = a.descendant_id
WHERE a.ancestor_id = '22298006'
ORDER BY c.preferred_term;
EOF
```

### Subsumption test (is A a descendant of B?)

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT CASE WHEN EXISTS (
  SELECT 1 FROM concept_ancestors
  WHERE ancestor_id  = '22298006'
    AND descendant_id = '57054005'
) THEN 'yes — is a descendant' ELSE 'no' END;
EOF
```

This is O(1) with the unique composite index — the core operation of any SNOMED subsumption check.

---

## 5 — Parquet for Analytics

Export to Apache Parquet for use with DuckDB, pandas, Polars, R, or Spark.

> **:lucide-book-text: Docs**: [`sct parquet`](commands/parquet.md)

```bash
sct parquet --input snomed-uk-20250301.ndjson --output snomed.parquet

# ~5 s for 831k concepts → 824 MB
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

> **:lucide-book-text: Docs**: For more DuckDB examples, see the [`sct parquet` documentation](commands/parquet.md)

---

## 6 — Markdown Export for RAG

Export SNOMED CT as a directory of Markdown files — one per concept. Ideal for
retrieval-augmented generation (RAG), Claude Code file reading, or filesystem MCP.

!!! danger "CRASH WARNING"
    **Use with caution:** the resulting directory is about 3.2 GB with 831k files (nested in subdirectories)which can be unwieldy to manage and version-control. If you try to open the directory in a text editor, it may crash. Consider using `.gitignore` or a separate branch if you want to keep it in the same repository.

> **:lucide-book-text: Docs**: [`sct markdown`](commands/markdown.md)

```bash
sct markdown --input snomed.ndjson --output ./snomed-concepts/

# ~14.5 s for ~831k .md files, ~1 GB total
```

**Example output** (`cat snomed-concepts/clinical-finding/22298006.md`):

```markdown
# Myocardial infarction

**SCTID:** 22298006
**FSN:** Myocardial infarction (disorder)
**Hierarchy:** SNOMED CT Concept > Clinical finding > Finding of trunk structure > Finding of upper trunk > Finding of thoracic region > Disorder of thorax > Disorder of mediastinum > Heart disease > Structural disorder of heart > Myocardial lesion > Myocardial necrosis

## Synonyms

- Infarction of heart
- Cardiac infarction
- Heart attack
- Myocardial infarct
- MI - myocardial infarction

## Relationships

- **Associated morphology:** Infarct [55641003]
- **Finding site:** Myocardium structure [74281007]

## Hierarchy

- SNOMED CT Concept
  - Clinical finding
    - Finding of trunk structure
      - Finding of upper trunk
        - Finding of thoracic region
          - Disorder of thorax
            - Disorder of mediastinum
              - Heart disease
                - Structural disorder of heart
                  - Myocardial lesion
                    - Myocardial necrosis
                      - **Myocardial infarction** *(this concept)*

## Parents

- Myocardial necrosis (disorder) `251061000`
- Ischemic heart disease (disorder) `414545008`
```

**Hierarchy-mode** (one file per top-level hierarchy, ~19 files):

```bash
sct markdown --input snomed.ndjson --output ./snomed-hierarchies/ --mode hierarchy

# ~ 3 s for ~ 20 .md files, total ~ 380 MB
```

These human-readable files can be quite helpful for just getting an understanding of how concepts are structured, what their preferred terms and synonyms are, and what relationships they have. They can be used as context documents for retrieval-augmented generation (RAG) with LLMs, or simply for browsing in a Markdown viewer or VSCode.

---

## 7 — Vector Embeddings

Generate dense vector embeddings for semantic (nearest-neighbour) search.

!!! tip "Local AI required"
    Requires [Ollama](https://ollama.ai) running locally.

The embeddings take quite a while to generate for the whole release (about 40 minutes for the UK Monolith with 831k concepts), and the resulting Arrow IPC file is about 2.7 GB, but the resulting semantic search capabilities are pretty impressive — you can find relevant concepts even when there are no shared keywords between the query and the concept text.

> **:lucide-book-text: Docs**: [`sct embed`](commands/embed.md)

Pull the embedding model

```bash
ollama pull nomic-embed-text

# ~
```

Generate embeddings (streams SNOMED into Arrow IPC file)

```bash
sct embed --input snomed.ndjson \
          --output snomed-embeddings.arrow \
          --model nomic-embed-text

# ~65 mins for ~831k concepts → snomed-embeddings.arrow (2.7 GB)
```

Each concept is embedded using a rich text template:

```text
"Heart attack. Myocardial infarction (disorder).
 Synonyms: Cardiac infarction, Infarction of heart, MI.
 Hierarchy: SNOMED CT concept > Clinical finding > ... > Myocardial infarction"
```

The Arrow IPC file can be queried in DuckDB or PyArrow, and is the input for
`sct semantic`.

---

## 8 — Semantic Search `experimental!` :lucide-test-tube

Find conceptually similar concepts using cosine similarity over embeddings.
No keyword match needed.

> **:lucide-book-text: Docs**: [`sct semantic`](commands/semantic.md)

```bash
sct semantic --embeddings snomed-embeddings.arrow \
             "blocked coronary artery" \
             --limit 5
```

Example output:

```
5 closest concepts to "blocked coronary artery":

  0.9340  [22298006] Myocardial infarction
  0.9210  [44771008] Coronary artery occlusion
  0.9080  [394659003] Acute coronary syndrome
  0.8970  [414795007] Ischaemic heart disease
  0.8810  [53741008] Coronary artery atherosclerosis
```

The first column is the **cosine similarity** between the query vector and the concept
embedding — a value between 0 and 1 where 1 means identical direction in vector space.
In practice, scores above ~0.85 indicate strong semantic relevance; scores below ~0.70
are usually noise. There is no hard threshold — results are always returned ranked, so
the top few are what matter.

Semantic search finds concepts even when the exact terms don't match — useful for
natural-language queries, typos, and synonym gaps.

The same search is also available to Claude via the `snomed_semantic_search` MCP tool
when `sct mcp` is started with `--embeddings`.

---

## 9 — MCP Server for LLMs

Expose SNOMED CT as a set of tools in Claude Code, Claude Desktop, or any other LLM harness or tool that supports the MCP (Model-Tool Communication Protocol) standard.

> **:lucide-book-text: Docs**: [`sct mcp`](commands/mcp.md)

Start `stdio` MCP server; add to Claude Desktop config

```bash
sct mcp --db snomed.db
```

With semantic search enabled:

```bash
sct mcp --db snomed.db --embeddings snomed-embeddings.arrow
```

### Claude Desktop configuration

Depending on your platform, the configuration file is located at `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows, and `~/.config/claude/claude_desktop_config.json` on Linux.

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

With semantic search:

```json
{
  "mcpServers": {
    "snomed": {
      "command": "sct",
      "args": ["mcp", "--db", "/path/to/snomed.db",
               "--embeddings", "/path/to/snomed-embeddings.arrow"]
    }
  }
}
```

### Tools available in the MCP server

| Tool | Description |
|---|---|
| `snomed_search` | Free-text search — returns top matching concepts |
| `snomed_concept` | Full concept detail by SCTID |
| `snomed_children` | Immediate IS-A children of a concept |
| `snomed_ancestors` | Full ancestor chain to SNOMED root |
| `snomed_hierarchy` | All concepts within a top-level hierarchy |
| `snomed_map` | Cross-map between SNOMED CT and CTV3 (UK only) |
| `snomed_semantic_search` | Nearest-neighbour semantic search (requires `--embeddings`) |

**Example MCP interaction:**

> "What are the subtypes of type 2 diabetes mellitus?"

LLM calls `snomed_children` with SCTID `44054006`, receives the list, and answers
with accurate SNOMED-grounded terminology.

### UK edition: CTV3 cross-mapping

If your database was built from a UK NHS SNOMED CT release, the MCP server also has access to
`snomed_map` — a bidirectional lookup tool for CTV3 legacy codes.

Example MCP interaction:

> "What's the CTV3 code for myocardial infarction?"

LLM calls `snomed_map` with SCTID `22298006` and terminology `snomed`, receives:

```json
{
  "snomed_id": "22298006",
  "ctv3_codes": ["X200E"],
  "read2_codes": []
}
```

Or in reverse:

> "I have a legacy CTV3 code X200E. What's the current SNOMED concept?"

LLM calls `snomed_map` with code `X200E` and terminology `ctv3`, receives full
SNOMED concept details and provides context with the modern terminology.

**MCP server properties:**

- Startup time < 5 ms (well under the 100 ms MCP budget)
- Read-only and stateless
- Dual-mode transport: supports both Claude Desktop (Content-Length framing) and
  Claude Code 2.1.86+ (newline-delimited JSON)
- Schema version validation on startup

---

## 10 — Interactive UIs

### Terminal UI  `experimental!` :lucide-test-tube:

To reduce the size of the default `sct` binary, the interactive terminal UI is an optional feature that needs to be enabled at build time with the `tui` feature flag. If you built `sct` without it, you can rebuild with: `cargo install --path . --features tui`

> **:lucide-book-text: Docs**: [`sct tui`](commands/tui.md)

```bash
sct tui --db snomed.db
```

Three-panel layout:

- **Top-left:** Hierarchy browser
- **Bottom-left:** Search box + results
- **Right:** Full concept detail

Keybindings: `/` search, `Tab` switch panels, `↑↓` navigate, `Enter` select, `q` quit.

### Browser UI `experimental!` :lucide-test-tube:

> **:lucide-book-text: Docs**: [`sct gui`](commands/gui.md)

The browser-based UI is another optional feature that needs to be enabled at build time with the `gui` feature flag. If you built `sct` without it, you can rebuild with: `cargo install --path . --features gui`

```bash
sct gui --db snomed.db
# Opens http://127.0.0.1:8420 in your browser

sct gui                  # --db defaults to ./snomed.db or $SCT_DB
sct gui --port 9000      # custom port
sct gui --no-open        # start server but don't open browser
```

Single-page app with three tabs:

- **Detail** — full concept view: preferred term, FSN, synonyms, attributes, parents, children count
- **Graph** — D3 force-directed graph showing the focal concept (centre), its parents (above), and up to 50 children (below). Draggable nodes, zoom/pan, click any node to navigate.
- **Hierarchy** — browse the 19 top-level SNOMED hierarchies

Bound to localhost only — never accessible from the network.

---

## 11 — Release Comparison `experimental` :lucide-test-tube

Compare two NDJSON artefacts to see what changed between SNOMED releases.

```bash
sct diff --old snomed-uk-20240901.ndjson \
         --new snomed-uk-20250301.ndjson \
         --format summary
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

## 12 — Artefact Inspection `experimental!` :lucide-test-tube:

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
network. See `benchmarks.md` for full methodology and results.

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

## 15 — Code Lists

Manage curated collections of clinical codes as plain-text `.codelist` files with YAML
front-matter — designed to live in version control and be reviewed like source code.

Also accessible as `sct refset` and `sct valueset`.

### Scaffold a new codelist

```bash
sct codelist new codelists/asthma-diagnosis.codelist \
  --title "Asthma diagnosis" \
  --author "Marcus Baw" \
  --terminology "SNOMED CT"
```

Creates the file with full YAML front-matter (id, title, description, licence, warnings, etc.)
and opens it in `$EDITOR`. Pass `--no-edit` to skip the editor.

### Add concepts

```bash
# Add single concepts by SCTID (resolved against the database)
sct codelist add codelists/asthma-diagnosis.codelist 195967001 389145006 --db snomed.db

# Add a concept plus all its active descendants
sct codelist add codelists/asthma-diagnosis.codelist 195967001 \
  --db snomed.db \
  --include-descendants
```

### Remove (exclude) a concept

```bash
sct codelist remove codelists/asthma-diagnosis.codelist 41553006 \
  --comment "occupational asthma — separate pathway"
```

Moves the line to a commented exclusion record, preserving the audit trail:

```
# 41553006      Occupational asthma  # occupational asthma — separate pathway
```

### Validate (CI-ready)

```bash
sct codelist validate codelists/asthma-diagnosis.codelist --db snomed.db
```

Checks: all SCTIDs exist and are active, preferred terms match the database (warns on
drift), pending review items, required fields, duplicate SCTIDs.

Exit code 0 = warnings only. Exit code 1 = errors. Suitable for CI.

### Stats

```bash
sct codelist stats codelists/asthma-diagnosis.codelist --db snomed.db
```

Prints concept count, hierarchy breakdown, leaf vs. intermediate ratio, excluded count,
and SNOMED release age.

### Diff two codelists

```bash
sct codelist diff codelists/asthma-v1.codelist codelists/asthma-v2.codelist
```

Reports added, removed, moved-to-excluded, and preferred-term-changed concepts.

### Export

```bash
sct codelist export codelists/asthma-diagnosis.codelist --format csv
sct codelist export codelists/asthma-diagnosis.codelist --format opencodelists-csv
sct codelist export codelists/asthma-diagnosis.codelist --format markdown --output asthma.md
```

### Typical git workflow

```bash
sct codelist new codelists/asthma-diagnosis.codelist
git add codelists/asthma-diagnosis.codelist
git commit -m "codelist: scaffold asthma-diagnosis"

sct codelist add codelists/asthma-diagnosis.codelist 195967001 266361008 389145006 --db snomed.db
git commit -m "codelist: add core asthma concepts"

sct codelist validate codelists/asthma-diagnosis.codelist --db snomed.db
git tag codelist/asthma-diagnosis/v1
```

---

## 15 — Command Reference Summary

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
| `sct tui` | Terminal UI (requires `--features tui`) |
| `sct gui` | Browser UI (requires `--features gui`) |
| `sct completions` | Generate shell completion scripts |
| `sct codelist` | Build, validate, publish code lists (also: `sct refset`, `sct valueset`) |

---

## Next Steps

- `sct trud` — automated download from NHS TRUD API
- `sct serve` — drop-in FHIR R4/R5 terminology server backed by SQLite
- `sct codelist search` — interactive FTS5 search → include/exclude (coming)
- `sct codelist import` / `sct codelist publish` — import from OpenCodelists, publish back (coming)

See `specs/roadmap.md` for the full list of planned features.
