# sct info

Inspect any file produced by `sct` and print a summary - without needing to open a database or write a query.

Accepts `.ndjson`, `.db`, and `.arrow` files.

---

## Usage

```
sct info <FILE> [--format text|json|yaml]
```

`--format` (`-f`) defaults to `text`. Use `json` or `yaml` for scripts and agents - the field names below (`concept_count`, `hierarchies`, `tct_row_count`, etc.) are stable across all three file types where they apply.

---

## Examples

```bash
sct info snomed-uk-20260311.ndjson
sct info snomed.db
sct info snomed-embeddings.arrow
sct info snomed.db --format json | jq .tct_row_count
```

---

## Output by file type

### `.ndjson`

```
File:           snomed-uk-20260701.ndjson
Size:           1.2 GB
Format:         NDJSON
Schema version: 5
Edition:        uk_sct2mo_42.3.0_20260701000001Z
Release date:   2026-07-01
Release id:     uk_sct2mo_42.3.0_20260701000001Z
Built by:       sct 0.18.2
Concepts:       837,930

Hierarchy breakdown (20 top-level):
  Pharmaceutical / biologic product             231,194
  Physical object                               224,797
  Clinical finding                              137,830
  ...
```

Reports:
- `Format` (`NDJSON`)
- Total concept count - active only by default; a separate `(inactive)` line appears when the file was built with `--include-inactive`
- `schema_version`
- Provenance, when present in the file's header: edition label, release date, release id, and the `sct` version that built it. Falls back to a filename-inferred release date for older pre-provenance NDJSONs.
- Hierarchy breakdown

### `.db`

```
File:              snomed.db
Size:              2.6 GB
Format:            SQLite (sct sqlite)
Schema version:    5
Edition:           uk_sct2mo_42.3.0_20260701000001Z
Release date:      2026-07-01
Release id:        uk_sct2mo_42.3.0_20260701000001Z
Built by:          sct 0.18.2
Concepts:          837,930
FTS5 rows:         837,930
IS-A edges:        1,605,202
TCT rows:          11,607,152

Hierarchy breakdown (20 top-level):
  Pharmaceutical / biologic product             231,194
  Physical object                               224,797
  Clinical finding                              137,830
  ...
```

Reports:
- `Format` (`SQLite (sct sqlite)`)
- Provenance, when present: edition label, release date, release id, and the `sct` version that built it
- Concept count
- `schema_version`
- FTS5 row count
- IS-A edge count (`concept_isa` table)
- TCT row count (`concept_ancestors` table), or a note that the transitive closure table is absent and how to build it (`sct tct`)
- Hierarchy breakdown

### `.arrow`

```
File:             snomed-embeddings.arrow
Size:             2.6 GB
Format:           Arrow IPC (sct embed)
Edition:          uk_sct2mo_42.3.0_20260701000001Z
Release date:     2026-07-01
Release id:       uk_sct2mo_42.3.0_20260701000001Z
Built by:         sct 0.18.2
Embeddings:       837,930
Dimension:        768

Schema:
  id                   Utf8
  preferred_term       Utf8
  hierarchy            Utf8
  embedding            FixedSizeList(768 x non-null Float32)
```

Reports:
- `Format` (`Arrow IPC (sct embed)`)
- Provenance, when present: edition label, release date, release id, and the `sct` version that built it
- Embedding count
- Embedding dimension
- Arrow schema (field names and types)
- File size

The Ollama embedding model name itself (e.g. `nomic-embed-text`) is not captured in the file or reported by `sct info` - track it separately if you build embeddings with more than one model.

---

## See also

- [`sct ndjson`](ndjson.md) - build the artefact
- [`sct diff`](diff.md) - compare two NDJSON artefacts