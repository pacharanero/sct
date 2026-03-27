# sct markdown

Export a SNOMED CT NDJSON artefact to per-concept Markdown files, organised by hierarchy.

Designed for RAG (retrieval-augmented generation) indexing, filesystem MCP tools, and direct LLM file reading.

---

## Usage

```
sct markdown --input <NDJSON> [--output <DIR>] [--mode <MODE>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--input <FILE>` | *(required)* | NDJSON file produced by `sct ndjson`. Use `-` for stdin. |
| `--output <DIR>` | `snomed-concepts` | Output directory. |
| `--mode <MODE>` | `concept` | Output grouping: `concept` (one file per concept) or `hierarchy` (one file per top-level hierarchy). |

---

## Modes

### `--mode concept` (default)

One `.md` file per SNOMED CT concept, named by SCTID. Output directory is partitioned by top-level hierarchy:

```
snomed-concepts/
  clinical-finding/
    22298006.md
    57054005.md
    ...
  procedure/
    173171007.md
    ...
  substance/
    ...
```

Best for:
- Fine-grained RAG indexing (one chunk per concept)
- `grep` / `ripgrep` / `fzf` searching
- Filesystem MCP tools that can browse individual files

### `--mode hierarchy`

One `.md` file per top-level hierarchy (~19 files), each containing all concepts in that hierarchy.

```
snomed-concepts/
  clinical-finding.md
  procedure.md
  substance.md
  ...
```

Best for:
- Upload to LLM context windows
- RAG pipelines that struggle with very large numbers of small files
- Quick browsing of an entire hierarchy

---

## Examples

```bash
# One file per concept (default)
sct markdown \
  --input snomed.ndjson \
  --output snomed-concepts/

# One file per hierarchy
sct markdown \
  --input snomed.ndjson \
  --output snomed-by-hierarchy/ \
  --mode hierarchy
```

---

## Per-concept file format (`--mode concept`)

```markdown
# Heart attack
**SCTID:** 22298006
**FSN:** Myocardial infarction (disorder)
**Hierarchy:** SNOMED CT Concept > Clinical finding > ... > Ischemic heart disease

## Synonyms
- Cardiac infarction
- Infarction of heart
- MI - Myocardial infarction

## Relationships
- **Finding site:** Entire heart [302509004]
- **Associated morphology:** Infarct [55641003]

## Hierarchy
- SNOMED CT Concept
  - Clinical finding
    - ...
      - **Myocardial infarction** *(this concept)*

## Parents
- Ischemic heart disease (disorder) `414795007`
```

---

## Searching concept files

```bash
# Find files mentioning a term
grep -r "heart attack" snomed-concepts/ -l

# Full-text search with ripgrep
rg "myocardial" snomed-concepts/

# Fuzzy-find concept files by SCTID
fzf < <(find snomed-concepts/ -name "*.md")

# Find concepts with a specific attribute
grep -r "Finding site" snomed-concepts/clinical-finding/ -l | wc -l
```

---

## Use with filesystem MCP

The Markdown output pairs well with a filesystem MCP server (e.g. the [MCP filesystem server](https://github.com/modelcontextprotocol/servers)):

```json
{
  "mcpServers": {
    "snomed-files": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/snomed-concepts"]
    }
  }
}
```

This allows an LLM to browse and read individual concept files directly.

---

## Notes on scale

The full UK Monolith produces ~831,000 files in `--mode concept`. This is handled fine by:
- `ripgrep`, `grep`, `find`
- Standard filesystem tools on Linux/macOS (ext4, APFS)
- Most RAG indexing pipelines

Some tools that may struggle with 800k+ files:
- Windows Explorer
- Certain filesystem MCP servers with directory listing limits
- Git (do not commit the output directory — add it to `.gitignore`)
