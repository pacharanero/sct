# Semantic Search and LLMs

Markdown export for RAG, vector embeddings for semantic search, and an MCP server
for connecting SNOMED CT to Claude and other LLMs.

---

## Markdown Export for RAG

Export SNOMED CT as a directory of Markdown files — one per concept. Ideal for
retrieval-augmented generation (RAG), Claude Code file reading, or filesystem MCP.

!!! danger "CRASH WARNING"
    **Use with caution:** the resulting directory is about 3.2 GB with 831,000 files (nested in subdirectories), which can be unwieldy to manage and version-control. If you try to open the directory in a text editor, it may crash. Consider using `.gitignore` or a separate branch if you want to keep it in the same repository.

> **Docs**: [`sct markdown`](../commands/markdown.md)

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

## Vector Embeddings

Generate dense vector embeddings for semantic (nearest-neighbour) search.

!!! tip "Local AI required"
    Requires [Ollama](https://ollama.ai) running locally.

The embeddings take quite a while to generate for the whole release (about 40 minutes for the UK Monolith with 831k concepts), and the resulting Arrow IPC file is about 2.7 GB, but the resulting semantic search capabilities are pretty impressive — you can find relevant concepts even when there are no shared keywords between the query and the concept text.

> **Docs**: [`sct embed`](../commands/embed.md)

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

## Semantic Search `experimental!` :lucide-test-tube

Find conceptually similar concepts using cosine similarity over embeddings.
No keyword match needed.

> **Docs**: [`sct semantic`](../commands/semantic.md)

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

## MCP Server for LLMs

Expose SNOMED CT as a set of tools in Claude Code, Claude Desktop, or any other LLM harness or tool that supports the MCP (Model-Tool Communication Protocol) standard.

> **Docs**: [`sct mcp`](../commands/mcp.md)

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
| `snomed_refsets` | List all loaded refsets with member counts |
| `snomed_refset_members` | List concepts belonging to a refset |
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
