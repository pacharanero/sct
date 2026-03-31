Inspect any file produced by `sct` and print a summary — without needing to open a database or write a query.

Accepts `.ndjson`, `.db`, and `.arrow` files.

---

## Usage

```
sct info <FILE>
```

---

## Examples

```bash
sct info snomed-uk-20260311.ndjson
sct info snomed.db
sct info snomed-embeddings.arrow
```

---

## Output by file type

### `.ndjson`

```
File:       snomed-uk-20260311.ndjson
Size:       1.1 GB
Release:    20260311 (inferred from filename)
Concepts:   831,042 active
Schema:     version 2

By hierarchy:
  Clinical finding    437,219
  Procedure           155,823
  Body structure       76,441
  ...
```

Reports:
- Total active concept count
- Inactive concept count (if `--include-inactive` was used at build time)
- `schema_version`
- Release date (inferred from filename)
- Hierarchy breakdown

### `.db`

```
File:       snomed.db
Size:       1.3 GB
Concepts:   831,042
Schema:     version 2
FTS rows:   831,042
IS-A edges: 1,847,331

By hierarchy:
  Clinical finding    437,219
  ...
```

Reports:
- Concept count
- `schema_version`
- FTS5 row count
- IS-A edge count (`concept_isa` table)
- Hierarchy breakdown

### `.arrow`

```
File:        snomed-embeddings.arrow
Size:        2.4 GB
Embeddings:  831,042
Dimensions:  768
Model:       (not stored — check how you built it)
```

Reports:
- Embedding count
- Embedding dimension
- Arrow schema
- File size

---

## See also

- [`sct ndjson`](ndjson.md) — build the artefact
- [`sct diff`](diff.md) — compare two NDJSON artefacts
