# Transitive Closure Table - Feature Specification

## Overview

This spec describes the addition of an optional precomputed transitive closure table (TCT) to the `sct sqlite` build step. The TCT materialises all ancestor-descendant relationships in the SNOMED CT hierarchy, eliminating the need for recursive CTE queries at query time and significantly simplifying the SQL emitter in the SCT-QL query language compiler.

---

## Background

The current `sct sqlite` schema includes a `concept_isa` table containing immediate parent-child pairs only:

```sql
CREATE TABLE concept_isa (
    child_id  TEXT NOT NULL,
    parent_id TEXT NOT NULL
);
CREATE INDEX idx_concept_isa_parent ON concept_isa(parent_id);
CREATE INDEX idx_concept_isa_child  ON concept_isa(child_id);
```

Subsumption queries (finding all descendants of a concept) currently require a recursive CTE:

```sql
WITH RECURSIVE descendants(id) AS (
  SELECT DISTINCT child_id FROM concept_isa WHERE parent_id = '22298006'
  UNION
  SELECT ci.child_id FROM concept_isa ci JOIN descendants d ON ci.parent_id = d.id
)
SELECT c.preferred_term FROM concepts c JOIN descendants d ON c.id = d.id
```

This is fast in practice (~4ms on the UK Clinical Edition on a developer workstation) but grows with hierarchy depth and becomes more complex to generate correctly when composing multi-hop queries in the SCT-QL compiler.

---

## Proposed schema addition

```sql
CREATE TABLE concept_ancestors (
    ancestor_id   TEXT NOT NULL,
    descendant_id TEXT NOT NULL,
    depth         INTEGER NOT NULL  -- number of hops from ancestor to descendant
);

CREATE INDEX idx_ca_ancestor   ON concept_ancestors(ancestor_id);
CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id);
CREATE UNIQUE INDEX idx_ca_pair ON concept_ancestors(ancestor_id, descendant_id);
```

The `depth` column records how many IS-A hops separate the pair. Direct parent-child pairs have `depth = 1`. This enables queries like "find all concepts within 2 levels of X" which are useful for partial hierarchy exploration in the TUI and GUI.

### Why not include self-pairs?

A common TCT convention includes self-referential rows (`ancestor_id = descendant_id`, `depth = 0`) to simplify "descendants including self" queries. This is included as a build option (`--include-self`) but off by default to keep the table smaller. The SCT-QL compiler handles the `including self` case by unioning the base concept with the TCT result, which is trivial to generate.

---

## CLI

### Standalone command (preferred)

```bash
sct tct --db snomed.db
sct tct --db snomed.db --include-self
```

Because the TCT is derived entirely from the `concept_isa` table that already exists in every `sct sqlite` output, it can be computed against any existing database without re-reading the original NDJSON input. This makes `sct tct` a post-hoc optimisation step that can be added to an existing database at any time.

`--db` is required. `--include-self` controls whether self-referential rows (`ancestor_id = descendant_id`, `depth = 0`) are included (off by default — see below).

### Convenience flag on `sct sqlite`

```bash
sct sqlite --input snomed.ndjson --output snomed.db --transitive-closure
sct sqlite --input snomed.ndjson --output snomed.db --transitive-closure --include-self
```

This is a convenience shorthand that calls the same underlying TCT build function after the main load completes, within the same invocation. Both flags are opt-in; without `--transitive-closure`, `sct sqlite` behaves exactly as today and the `concept_ancestors` table is not created.

A `sct info snomed.db` report should indicate whether the TCT is present and how many rows it contains.

---

## Build algorithm

The TCT is computed from the `concept_isa` table after it has been fully populated. It does not require re-reading the NDJSON input.

### Algorithm

A breadth-first traversal from every concept upward through its ancestors, inserting a row for each (ancestor, descendant, depth) triple discovered.

Pseudocode:

```
for each concept C in concepts:
    queue = [(C, 0)]               -- (current_node, depth_from_C)
    visited = {}
    while queue not empty:
        (node, depth) = dequeue
        for each parent P of node (via concept_isa):
            if P not in visited:
                visited.add(P)
                insert (ancestor=P, descendant=C, depth=depth+1)
                enqueue (P, depth+1)
```

This visits every concept once per ancestor path. For a DAG with multiple parents, a concept may be reachable via multiple paths - the `UNIQUE INDEX` on `(ancestor_id, descendant_id)` handles deduplication. Because the traversal is BFS, the first time any ancestor is encountered for a given descendant is always via the shortest path, so `INSERT OR IGNORE` is sufficient and correct for recording minimum depth.

### Rust implementation notes

- Run the traversal after the main `concept_isa` insert loop completes
- Use a single SQLite transaction for all TCT inserts - this is critical for performance; individual transactions per concept will be orders of magnitude slower
- Batch inserts using prepared statements with parameter binding
- Progress reporting via the existing progress bar pattern in `sct sqlite`
- The traversal is embarrassingly parallelisable per concept but SQLite's write serialisation means parallelism helps only if you accumulate rows in memory and flush in batches - profile before adding complexity

### Expected size

For the UK Clinical Edition (~412,000 active concepts):

- `concept_isa` rows: ~500,000 (immediate pairs)
- `concept_ancestors` rows: estimated 5-20 million (all transitive pairs)

The wide range reflects uncertainty about SNOMED's actual hierarchy density. The Monolith will be larger. Measure on first implementation and record in `BENCHMARKS.md`.

---

## Query patterns enabled

### All descendants of a concept

```sql
-- current (recursive CTE, ~4ms)
WITH RECURSIVE descendants(id) AS (
  SELECT DISTINCT child_id FROM concept_isa WHERE parent_id = '22298006'
  UNION
  SELECT ci.child_id FROM concept_isa ci JOIN descendants d ON ci.parent_id = d.id
)
SELECT c.preferred_term FROM concepts c JOIN descendants d ON c.id = d.id

-- with TCT (simple JOIN, expected <1ms)
SELECT c.preferred_term
FROM concepts c
JOIN concept_ancestors a ON c.id = a.descendant_id
WHERE a.ancestor_id = '22298006'
```

### Descendants including self

```sql
SELECT c.preferred_term
FROM concepts c
WHERE c.id = '22298006'

UNION

SELECT c.preferred_term
FROM concepts c
JOIN concept_ancestors a ON c.id = a.descendant_id
WHERE a.ancestor_id = '22298006'
```

### All ancestors of a concept

```sql
SELECT c.preferred_term
FROM concepts c
JOIN concept_ancestors a ON c.id = a.ancestor_id
WHERE a.descendant_id = '22298006'
ORDER BY a.depth
```

### Subsumption test (is A a descendant of B?)

```sql
-- returns a row if true, no rows if false
SELECT 1 FROM concept_ancestors
WHERE ancestor_id = '22298006'   -- B
AND descendant_id = '57054005'   -- A (Acute myocardial infarction)
LIMIT 1
```

This is O(1) with the index - the core operation of any subsumption test.

### Concepts within N hops

```sql
-- all concepts within 2 IS-A hops of Myocardial infarction
-- (useful for TUI/GUI neighbourhood exploration)
SELECT c.preferred_term, a.depth
FROM concepts c
JOIN concept_ancestors a ON c.id = a.descendant_id
WHERE a.ancestor_id = '22298006'
AND a.depth <= 2
ORDER BY a.depth, c.preferred_term
```

### Attribute-refined subsumption (the 'gnarly' query, simplified)

Without TCT, the complex multi-hop query requires multiple nested recursive CTEs. With TCT, it becomes straightforward JOINs:

```sql
-- descendants of "Pharmaceutical product" [373873005]
-- where finding-site is a descendant of "Cardiovascular finding" [57809008]

SELECT DISTINCT c.preferred_term
FROM concepts c

-- must be a descendant of Pharmaceutical product
JOIN concept_ancestors pharma
  ON c.id = pharma.descendant_id
  AND pharma.ancestor_id = '373873005'

-- must have a finding-site relationship
JOIN concept_relationships r
  ON r.source_id = c.id
  AND r.type_id = '363698007'   -- finding-site

-- where that finding site is a descendant of Cardiovascular finding
JOIN concept_ancestors cardio
  ON r.destination_id = cardio.descendant_id
  AND cardio.ancestor_id = '57809008'

ORDER BY c.preferred_term
```

This is the query that motivated the TCT addition - it is difficult to generate correctly as nested recursive CTEs and straightforward to generate as a chain of JOINs.

---

## Impact on SCT-QL compiler

The SQL emitter in the SCT-QL compiler (see `sct-ql-spec.md`) currently needs to generate recursive CTEs for every `descendants of` or `ancestors of` expression. With the TCT available, the emitter simplifies dramatically:

```rust
fn emit_sql(expr: &Expr, has_tct: bool) -> String {
    match expr {
        Expr::Descendants { of, including_self } if has_tct => {
            // simple JOIN against concept_ancestors
            emit_tct_descendants(of, *including_self)
        }
        Expr::Descendants { of, including_self } => {
            // fallback: recursive CTE
            emit_recursive_descendants(of, *including_self)
        }
        // ...
    }
}
```

The compiler checks for TCT presence at compile time by querying the database schema:

```sql
SELECT name FROM sqlite_master WHERE type='table' AND name='concept_ancestors'
```

If the TCT is absent, the compiler falls back to recursive CTEs transparently. This means SCT-QL queries are always valid regardless of whether `--transitive-closure` was used at build time - the TCT is an optimisation, not a requirement.

---

## Benchmarking plan

Run the following before and after TCT addition and record results in `docs/benchmarks.md`:

```bash
# Simple subsumption - all descendants
time sqlite3 snomed.db "
  SELECT COUNT(*) FROM concepts c
  JOIN concept_ancestors a ON c.id = a.descendant_id
  WHERE a.ancestor_id = '22298006'"

# Subsumption test (point query)
time sqlite3 snomed.db "
  SELECT 1 FROM concept_ancestors
  WHERE ancestor_id = '22298006'
  AND descendant_id = '57054005'
  LIMIT 1"

# Complex attribute-refined query (the gnarly benchmark)
time sqlite3 snomed.db "
  SELECT COUNT(DISTINCT c.id) FROM concepts c
  JOIN concept_ancestors pharma ON c.id = pharma.descendant_id
    AND pharma.ancestor_id = '373873005'
  JOIN concept_relationships r ON r.source_id = c.id
    AND r.type_id = '363698007'
  JOIN concept_ancestors cardio ON r.destination_id = cardio.descendant_id
    AND cardio.ancestor_id = '57809008'"
```

Also record:
- TCT row count (`SELECT COUNT(*) FROM concept_ancestors`)
- `snomed.db` file size with and without TCT
- Time taken to build the TCT (via `sct tct` or `sct sqlite --transitive-closure`)

Record results in `docs/benchmarks.md`.

---

## Implementation priority

Medium. The recursive CTE approach works correctly and performs well enough for current use cases. The TCT becomes higher priority when:

- The SCT-QL compiler is being built (simplifies the SQL emitter significantly)
- The TUI or GUI needs sub-millisecond hierarchy navigation for responsive UI
- Batch subsumption testing over large datasets (e.g. validating a codelist against a hierarchy) becomes a use case

The `sct tct` command and `--transitive-closure` flag should be added to the CLI surface now even if the implementation follows later, so users can plan for it in their build pipelines.
