# `sct sqlite` — Load the NDJSON artefact into a portable SQLite database

Streams the canonical NDJSON artefact into a single `snomed.db` SQLite file with full-text
search via FTS5 and a fast IS-A traversal table. The resulting file is queryable with plain
`sqlite3` — no custom binary needed at query time.

---

## Synopsis

```bash
sct sqlite --input <ndjson> --output <db>
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--input <file>` | *(required)* | Input `.ndjson` file produced by `sct ndjson`. Use `-` for stdin. |
| `--output <file>` | `snomed.db` | Output SQLite database path. |

---

## Examples

```bash
sct sqlite --input snomed-20260311.ndjson --output snomed.db
ls -lh snomed.db

# Verify FTS works
sqlite3 snomed.db "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 5"
```

---

## Schema

```sql
CREATE TABLE concepts (
    id              TEXT PRIMARY KEY,
    fsn             TEXT NOT NULL,
    preferred_term  TEXT NOT NULL,
    synonyms        TEXT,    -- JSON array
    hierarchy       TEXT,
    hierarchy_path  TEXT,    -- JSON array
    parents         TEXT,    -- JSON array of {id, fsn}
    children_count  INTEGER,
    attributes      TEXT,    -- JSON object
    active          INTEGER,
    module          TEXT,
    effective_time  TEXT,
    schema_version  INTEGER
);

CREATE VIRTUAL TABLE concepts_fts USING fts5(
    id,
    preferred_term,
    synonyms,
    fsn,
    content='concepts',
    content_rowid='rowid'
);

-- Fast IS-A traversal without JSON parsing
CREATE TABLE concept_isa (
    child_id  TEXT NOT NULL,
    parent_id TEXT NOT NULL
);
```

---

## Example queries

```bash
# Free-text search
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 10"

# Exact concept lookup
sqlite3 snomed.db "SELECT json(attributes) FROM concepts WHERE id = '22298006'"

# All concepts in a hierarchy
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts WHERE hierarchy = 'Procedure' LIMIT 20"
```

---

## Design notes

- Writes in WAL mode; FTS5 content table is rebuilt once streaming finishes.
- The `concept_isa` table enables recursive CTE ancestor queries without JSON parsing.
- The output is a single portable file — commit to git-lfs, attach to a release, or `scp` to another machine.
- `sct mcp` and `sct tui` / `sct gui` all use this database.
