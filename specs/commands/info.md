# `sct info` — Inspect a `sct`-produced artefact

Prints a summary of any file produced by `sct`: concept counts, schema version, hierarchy
breakdown, file size — without needing to open a database or write a query.

---

## Synopsis

```bash
sct info <file>
```

## Arguments

| Argument | Description |
|---|---|
| `<file>` | Path to a `.ndjson`, `.db`, or `.arrow` file produced by `sct`. |

---

## Examples

```bash
sct info snomed-20260311.ndjson
sct info snomed.db
sct info snomed-embeddings.arrow
```

---

## Output by file type

### `.ndjson`

- Total concept count (active)
- Inactive concept count (if `--include-inactive` was used)
- `schema_version`
- Release date (inferred from filename)
- Hierarchy breakdown (count per top-level hierarchy)
- File size

### `.db`

- Concept count
- `schema_version`
- FTS5 row count
- IS-A edge count (`concept_isa` table)
- File size
- Hierarchy breakdown

### `.arrow`

- Embedding count
- Embedding dimension
- Arrow schema
- File size
