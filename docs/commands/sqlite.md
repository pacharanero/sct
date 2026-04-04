# sct sqlite

Load a SNOMED CT NDJSON artefact into a SQLite database with full-text search (FTS5).

**When to use:** you want keyword/phrase search, SQL queries, or to run the MCP server or UIs. For meaning-based search, see [`sct embed`](embed.md) + [`sct semantic`](semantic.md).

The resulting `snomed.db` is a single portable file queryable with `sqlite3` or any SQLite library — no custom binary needed at query time.

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
    ctv3_codes     TEXT,            -- JSON array of strings (UK edition only)
    read2_codes    TEXT,            -- JSON array of strings (UK edition only)
    schema_version INTEGER NOT NULL DEFAULT 2
);
```

### `concept_isa` table

Flat IS-A relationship table; indexed for fast children/ancestor queries without JSON parsing.

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

### `concept_maps` table

Reverse index for fast legacy code → SNOMED lookup (UK edition only).

```sql
CREATE TABLE concept_maps (
    concept_id  TEXT NOT NULL,
    code        TEXT NOT NULL,
    terminology TEXT NOT NULL   -- 'ctv3' or 'read2'
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

### SNOMED → CTV3 crossmap (UK edition)

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term, ctv3_codes FROM concepts WHERE id = '22298006'"
```

### CTV3 → SNOMED reverse lookup (UK edition)

```bash
sqlite3 snomed.db "
  SELECT c.id, c.preferred_term, c.hierarchy
  FROM concepts c
  JOIN concept_maps m ON c.id = m.concept_id
  WHERE m.code = 'X200E' AND m.terminology = 'ctv3'"
```

---

## Tips

- Use `sqlite3 -readonly snomed.db` to prevent accidental writes.
- JSON columns can be queried with `json_extract(col, '$.key')` and iterated with `json_each(col)`.
- The FTS5 `rank` column gives BM25 relevance ordering: `ORDER BY rank`.
- For Python: `import sqlite3; con = sqlite3.connect("snomed.db")`.
- The database is read-only safe — `sct mcp`, `sct tui`, and `sct gui` all open it read-only.

---

*Next: search with [`sct lexical`](lexical.md) or connect an AI assistant with [`sct mcp`](mcp.md).*

## Gnarly SQL query examples

These queries run directly against the SQLite database produced by `sct sqlite`.
They demonstrate the kind of terminology reasoning that ECL (Expression Constraint Language) servers are typically benchmarked on, in an effort to show that you can do the same things with standard SQL queries against the `snomed.db` artefact, and to test the capabilities of this toolset.

> **Note on `UNION` vs `UNION ALL` in recursive CTEs**
>
> SNOMED CT is a *polyhierarchy* — a concept can have more than one parent.
> Recursive CTEs with `UNION ALL` will visit the same ancestor or descendant
> multiple times (once per path), causing exponential row explosion on large
> hierarchies. Always use `UNION` in recursive CTEs so that visited nodes are
> deduplicated and the query terminates promptly.

---

## 1. Descendant count (subsumption benchmark)

**In plain English:** "How many concepts in SNOMED CT are a type of *Diabetes mellitus*?"

This is the most fundamental ECL operation — `<<73211009|Diabetes mellitus|` — the
double-chevron meaning *self plus all descendants*. The recursive CTE walks the
`concept_isa` table downward from the seed concept, following every IS-A
relationship until no new children are found.

```bash
sqlite3 /home/marcus/code/sct/snomed.db "
WITH RECURSIVE descendants AS (
  SELECT '73211009' AS id
  UNION
  SELECT ci.child_id FROM concept_isa ci
  JOIN descendants d ON ci.parent_id = d.id
)
SELECT COUNT(*) AS total_descendants FROM descendants;
"
```

---

## 2. Lowest common ancestor

**In plain English:** "What is the most specific concept that both *Myocardial infarction*
and *Heart failure* are a type of?"

Finding the Lowest Common Ancestor (LCA) of two concepts is a classic terminology
server operation. It answers questions like "how closely related are these two
diagnoses?" and underpins similarity scoring, query optimisation, and subsumption
testing. Two separate ancestor chains are walked upward to the root, then
intersected; the result is ordered by depth (using the pre-computed
`hierarchy_path` length stored on each concept) so the most specific shared
ancestor appears first.

```bash
sqlite3 /home/marcus/code/sct/snomed.db "
WITH RECURSIVE
  ancestors_mi AS (
    SELECT parent_id FROM concept_isa WHERE child_id = '22298006'
    UNION
    SELECT ci.parent_id FROM concept_isa ci
    JOIN ancestors_mi a ON ci.child_id = a.parent_id
  ),
  ancestors_hf AS (
    SELECT parent_id FROM concept_isa WHERE child_id = '84114007'
    UNION
    SELECT ci.parent_id FROM concept_isa ci
    JOIN ancestors_hf a ON ci.child_id = a.parent_id
  )
SELECT c.id, c.preferred_term,
       json_array_length(c.hierarchy_path) AS depth
FROM ancestors_mi a
JOIN ancestors_hf b ON a.parent_id = b.parent_id
JOIN concepts c ON c.id = a.parent_id
ORDER BY depth DESC
LIMIT 5;
"
```

---

## 3. Attribute refinement with subsumption

**In plain English:** "Find clinical findings whose finding site is somewhere in the
cardiovascular system (but not necessarily the heart specifically)."

This is ECL with an attribute refinement:

```
<<404684003|Clinical finding| :
  363698007|Finding site| = <<113257007|Structure of cardiovascular system|
```

The recursive CTE expands the value side of the attribute constraint (`<<113257007`)
into the full set of cardiovascular structures. The `hierarchy` column filter
replaces what would otherwise be an equally expensive recursive expansion of the
entire Clinical finding hierarchy. The `EXISTS` clause walks the JSON array stored
in `attributes` and checks each value's SCTID against that set.

```bash
sqlite3 /home/marcus/code/sct/snomed.db "
WITH RECURSIVE cardio_structure AS (
  SELECT '113257007' AS id
  UNION
  SELECT ci.child_id FROM concept_isa ci
  JOIN cardio_structure cs ON ci.parent_id = cs.id
)
SELECT c.id, c.preferred_term
FROM concepts c
WHERE c.active = 1
  AND c.hierarchy = 'Clinical finding'
  AND EXISTS (
    SELECT 1
    FROM json_each(json_extract(c.attributes, '$.finding_site')) fs
    WHERE json_extract(fs.value, '$.id') IN (SELECT id FROM cardio_structure)
  )
LIMIT 50;
"
```
