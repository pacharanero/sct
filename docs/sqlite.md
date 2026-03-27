# sct sqlite

Load a SNOMED CT NDJSON artefact into a SQLite database with full-text search (FTS5).

The resulting `snomed.db` is a single portable file queryable with `sqlite3` or any SQLite library.

---

## Usage

```
sct sqlite --input <NDJSON> [--output <DB>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--input <FILE>` | *(required)* | NDJSON file produced by `sct ndjson`. Use `-` for stdin. |
| `--output <FILE>` | `snomed.db` | Output SQLite database path. |

---

## Example

```bash
sct sqlite \
  --input snomedct-monolithrf2-production-20260311t120000z.ndjson \
  --output snomed.db
```

---

## Schema

### `concepts` table

```sql
CREATE TABLE concepts (
    id             TEXT PRIMARY KEY,
    fsn            TEXT NOT NULL,
    preferred_term TEXT NOT NULL,
    synonyms       TEXT,            -- JSON array of strings
    hierarchy      TEXT,
    hierarchy_path TEXT,            -- JSON array of strings
    parents        TEXT,            -- JSON array of {id, fsn}
    children_count INTEGER,
    attributes     TEXT,            -- JSON object
    active         INTEGER NOT NULL,
    module         TEXT,
    effective_time TEXT,
    schema_version INTEGER NOT NULL DEFAULT 1
);
```

### `concept_isa` table

Flat IS-A relationship table; indexed for fast children/ancestor queries.

```sql
CREATE TABLE concept_isa (
    child_id  TEXT NOT NULL,
    parent_id TEXT NOT NULL
);
CREATE INDEX idx_concept_isa_parent ON concept_isa(parent_id);
CREATE INDEX idx_concept_isa_child  ON concept_isa(child_id);
```

### `concepts_fts` FTS5 virtual table

Full-text search over `id`, `preferred_term`, `synonyms`, and `fsn`.

```sql
CREATE VIRTUAL TABLE concepts_fts USING fts5(
    id,
    preferred_term,
    synonyms,
    fsn,
    content='concepts',
    content_rowid='rowid'
);
```

---

## Example queries

### Free-text search

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 10"
```

### Exact concept lookup

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term, hierarchy FROM concepts WHERE id = '22298006'"
```

### All concepts in a hierarchy

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts WHERE hierarchy = 'Procedure' LIMIT 20"
```

### Children of a concept

```bash
sqlite3 snomed.db \
  "SELECT c.id, c.preferred_term
   FROM concepts c
   JOIN concept_isa ci ON ci.child_id = c.id
   WHERE ci.parent_id = '22298006'
   ORDER BY c.preferred_term"
```

### Ancestors of a concept (recursive, root → concept)

```bash
sqlite3 snomed.db "
  WITH RECURSIVE anc(id, depth) AS (
    SELECT parent_id, 1 FROM concept_isa WHERE child_id = '22298006'
    UNION ALL
    SELECT ci.parent_id, a.depth + 1
    FROM concept_isa ci JOIN anc a ON a.id = ci.child_id
    WHERE a.depth < 25
  )
  SELECT DISTINCT c.id, c.preferred_term, MAX(a.depth) depth
  FROM anc a JOIN concepts c ON c.id = a.id
  GROUP BY c.id ORDER BY depth DESC"
```

### Top-level hierarchy counts

```bash
sqlite3 snomed.db \
  "SELECT hierarchy, COUNT(*) n FROM concepts GROUP BY hierarchy ORDER BY n DESC LIMIT 10"
```

### Concepts with a specific attribute

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts
   WHERE json_extract(attributes, '$.finding_site') IS NOT NULL
   LIMIT 10"
```

### Concepts modified in a specific release

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts WHERE effective_time = '20260301' LIMIT 20"
```

---

## Tips

- The database is read-only safe — `sqlite3 snomed.db` opens in read-write mode by default; use `sqlite3 -readonly snomed.db` to prevent accidental writes.
- JSON columns can be queried with `json_extract(col, '$.key')` and iterated with `json_each(col)`.
- The FTS5 `rank` column can be used for relevance ordering: `ORDER BY rank`.
- For programmatic access from Python: `import sqlite3; con = sqlite3.connect("snomed.db")`.
