# `sct diff` — Compare two SNOMED CT NDJSON artefacts

Compares two releases of the canonical NDJSON artefact (e.g. 2025-01 vs 2026-01) and reports
what changed between them.

---

## Synopsis

```bash
sct diff --old <ndjson> --new <ndjson> [--format summary|ndjson] [--output <file>]
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--old <file>` | *(required)* | The older NDJSON artefact (baseline). |
| `--new <file>` | *(required)* | The newer NDJSON artefact (comparison target). |
| `--format <fmt>` | `summary` | Output format: `summary` (human-readable) or `ndjson` (one diff record per change). |
| `--output <file>` | stdout | Output file for `--format ndjson`. |

---

## Change categories reported

- **Added** — concept present in NEW, absent from OLD
- **Inactivated** — concept active in OLD, absent or inactive in NEW
- **Term changed** — preferred term changed between releases
- **Hierarchy changed** — concept moved to a different top-level hierarchy

---

## Examples

```bash
# Human-readable summary
sct diff --old snomed-20250101.ndjson --new snomed-20260311.ndjson

# NDJSON diff records (one per changed concept)
sct diff --old snomed-20250101.ndjson --new snomed-20260311.ndjson \
  --format ndjson --output changes-2025-to-2026.ndjson
```

---

## NDJSON diff record format

```json
{"change": "added",      "id": "...", "preferred_term": "...", "hierarchy": "..."}
{"change": "inactivated","id": "...", "preferred_term": "...", "hierarchy": "..."}
{"change": "term_changed","id": "...", "old_term": "...", "new_term": "...", "hierarchy": "..."}
{"change": "hierarchy_changed","id": "...", "preferred_term": "...", "old_hierarchy": "...", "new_hierarchy": "..."}
```
