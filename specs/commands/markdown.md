# `sct markdown` — Export the NDJSON artefact to Markdown files

Produces Markdown output from the NDJSON artefact in one of two modes: one file per concept
(default), or one file per top-level hierarchy. Designed for RAG indexing and direct LLM file
reading via tools like Claude Code or the filesystem MCP.

---

## Synopsis

```bash
sct markdown --input <ndjson> --output <dir> [--mode concept|hierarchy]
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--input <file>` | *(required)* | Input `.ndjson` file. Use `-` for stdin. |
| `--output <dir>` | *(required)* | Output directory (created if absent). |
| `--mode <mode>` | `concept` | Output grouping: `concept` (one file per concept) or `hierarchy` (one file per top-level hierarchy). |

---

## `--mode concept` (default)

One `.md` file per concept, named by SCTID, organised into subdirectories by hierarchy:

```
snomed-concepts/
  clinical-finding/
    22298006.md
  procedure/
    80146002.md
  …
```

### Per-concept file format

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
- **Associated morphology:** Infarct [55641003]

## Hierarchy
- SNOMED CT concept
  - Clinical finding
    - Disorder of cardiovascular system
      - Ischemic heart disease
        - **Myocardial infarction** (this concept)
```

---

## `--mode hierarchy`

One `.md` file per top-level hierarchy (~19 files), each containing all concepts in that
hierarchy as H2 sections. Useful for bulk LLM ingestion where all related concepts should
share context.

```
snomed-concepts/
  clinical-finding.md
  procedure.md
  …
```

---

## Examples

```bash
# One file per concept
sct markdown --input snomed.ndjson --output ./snomed-concepts/

# One file per hierarchy
sct markdown --input snomed.ndjson --output ./snomed-concepts/ --mode hierarchy

# Check output
du -sh ./snomed-concepts/
find ./snomed-concepts/ -name "*.md" | wc -l
```
