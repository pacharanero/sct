# sct tct

Build a **transitive closure table** (TCT) over the SNOMED CT IS-A hierarchy in an existing SQLite database.

**When to use:** you need fast subsumption queries - "give me all descendants of X" - and want to avoid recursive CTEs at query time. The TCT trades database size for query speed: a recursive CTE takes ~4 ms per root concept; the TCT collapses that to an indexed lookup under 1 ms regardless of hierarchy depth or fanout.

The TCT is entirely optional. Because it is derived from the `concept_isa` table already present in every `sct sqlite` output, it can be added to any existing database at any time without re-reading the original NDJSON artefact.

---

## Usage

```
sct tct --db <DB> [--include-self]
```

Or in a single build step:

```
sct sqlite --ndjson <NDJSON> --output <DB> --transitive-closure [--include-self]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--db <FILE>` | *(required)* | SQLite database produced by `sct sqlite`. |
| `--include-self` | off | Also insert self-referential rows (`ancestor_id = descendant_id`, `depth = 0`). See below. |

---

## Examples

Apply to an existing database:

```bash
sct tct --db snomed.db
```

Build TCT as part of the initial load:

```bash
sct sqlite --ndjson snomed.ndjson --output snomed.db --transitive-closure
```

Both produce identical output. The `--transitive-closure` flag is a convenience for pipelines that want everything in one invocation.

TCT publication is transactional: readers see either the previous usable table or the new table, all three indexes, its completion marker, and invalidation triggers on concept IDs, the source hierarchy, and closure rows. Those triggers invalidate the marker if any source changes while preserving whether the table was built with `--include-self`, so stale or manually damaged closures fail closed and automatic repairs retain their self-pair mode. Legacy tables without the marker and triggers cannot prove that every transitive pair was written, so readers fall back safely and rerunning `sct tct --db <DB>` rebuilds them. A marked table with missing or malformed generated indexes is repaired in place; adding `--include-self` also inserts missing self-pairs and republishes the marker. To avoid replacing known-good derived data accidentally, `sct tct` refuses an already usable table with a non-zero exit.

With self-pairs:

```bash
sct tct --db snomed.db --include-self
```

---

## Schema

```sql
CREATE TABLE concept_ancestors (
    ancestor_id   INTEGER NOT NULL,
    descendant_id INTEGER NOT NULL,
    depth         INTEGER NOT NULL   -- number of IS-A hops from ancestor to descendant
);

CREATE INDEX idx_ca_ancestor   ON concept_ancestors(ancestor_id);
CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id);
CREATE UNIQUE INDEX idx_ca_pair ON concept_ancestors(ancestor_id, descendant_id);

CREATE TABLE concept_ancestors_meta (
    schema_version INTEGER NOT NULL,
    include_self   INTEGER NOT NULL CHECK (include_self IN (0, 1))
);
```

The `depth` column records the minimum number of IS-A hops separating the pair. Direct parent-child pairs have `depth = 1`. If `--include-self` was used, self-referential pairs have `depth = 0`. `concept_ancestors_meta` is an internal one-row completion marker written in the same transaction as the closure and nine invalidation triggers; do not create it or those triggers by hand to bless an independently populated table.

`ancestor_id` and `descendant_id` are `INTEGER`, not `TEXT` like `concepts.id` and `concept_isa.child_id`/`parent_id`. SCTIDs are numeric, so an INTEGER-typed index sorts far more cheaply than the equivalent TEXT index - this was the single largest cost in building the TCT over the full UK Monolith. `concept_ancestors` is an internal derived table that nothing else JOINs to the TEXT `concepts.id` column, so the INTEGER affinity stays self-contained.

**This matters when you hand-write a JOIN back to `concepts`.** A bare `ON c.id = a.descendant_id` mixes a TEXT column with an INTEGER column across a join, and SQLite's planner cannot use the index on either side - it falls back to scanning `concepts` (837k rows) and probing the TCT per row, which turns a sub-millisecond lookup into several seconds. Wrap the `concept_ancestors` side in `CAST(... AS TEXT)` so the comparison stays index-friendly: `ON c.id = CAST(a.descendant_id AS TEXT)`. A plain `WHERE ancestor_id = '22298006'` (comparing the INTEGER column directly to a string literal, no join) is unaffected - SQLite coerces the literal automatically and the query patterns below that don't join back to `concepts` are already index-friendly as written.

---

## Checking TCT health

```bash
sct info snomed.db
```

Without TCT:

```text
IS-A edges:        1,605,202
TCT:               not present
```

After `sct tct`:

```text
IS-A edges:        1,605,202
TCT rows:          11,607,152
```

`sct info` checks the exact table schema, the transactionally published completion marker, all three required index definitions (table, columns, order, and uniqueness), and all nine source/closure invalidation trigger definitions. Structured output includes both `tct_row_count` and `tct_usable`; build-or-repair guidance goes to stderr so stdout remains composable.

You can inspect the generated schema objects directly:

```bash
sqlite3 snomed.db \
  "SELECT type, name FROM sqlite_master
   WHERE name LIKE 'tct_invalidate_%'
      OR name IN ('concept_ancestors', 'concept_ancestors_meta',
                  'idx_ca_ancestor', 'idx_ca_descendant', 'idx_ca_pair')
   ORDER BY type DESC, name"
```

---

## Rebuilding

`sct tct` refuses to recompute a usable `concept_ancestors` table. It repairs missing or malformed generated indexes, upgrades a usable table when `--include-self` is newly requested, and automatically rebuilds rows when a legacy or incomplete table has no valid completion marker. No manual `DROP TABLE` step is needed:

```bash
sct tct --db snomed.db
```

---

## Self-pairs (`--include-self`)

By default the TCT contains only strict ancestor-descendant pairs (`depth >= 1`). This keeps the table smaller and is sufficient for most queries.

When `--include-self` is set, a row `(ancestor_id = C, descendant_id = C, depth = 0)` is also inserted for every concept C. This simplifies "descendants **including self**" queries from a UNION to a single JOIN:

```sql
-- without --include-self
SELECT c.preferred_term FROM concepts c WHERE c.id = '22298006'
UNION
SELECT c.preferred_term FROM concepts c
  JOIN concept_ancestors a ON c.id = CAST(a.descendant_id AS TEXT) AND a.ancestor_id = '22298006'

-- with --include-self
SELECT c.preferred_term FROM concepts c
  JOIN concept_ancestors a ON c.id = CAST(a.descendant_id AS TEXT) AND a.ancestor_id = '22298006'
```

---

## Expected sizes

| Release | IS-A edges | TCT rows (no self) | TCT rows (with self) |
|---|---|---|---|
| UK Clinical Edition (~412k concepts) | ~500k | ~5–15 M | ~5–15 M + 412k |
| UK Monolith (837,930 concepts) | ~1 M | 11.6 M (measured) | 11.6 M + 837,930 |

The UK Monolith row is a real measurement (`docs/benchmarks.md`); the UK Clinical Edition row is still an estimate pending a fresh run. Measure with `sct info` and record in `docs/benchmarks.md`.

---

## Query patterns

### All descendants of a concept

Without TCT - recursive CTE (~4 ms on UK Monolith):

```bash
sqlite3 snomed.db <<EOF
.timer on
WITH RECURSIVE descendants(id) AS (
  SELECT child_id FROM concept_isa WHERE parent_id = '22298006'
  UNION
  SELECT ci.child_id FROM concept_isa ci
    JOIN descendants d ON ci.parent_id = d.id
)
SELECT COUNT(*) FROM descendants;
EOF
```

With TCT - indexed lookup (<1 ms on UK Monolith):

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT COUNT(*) FROM concept_ancestors WHERE ancestor_id = '22298006';
EOF
```

Both return the count of all descendants of Myocardial infarction (`22298006`). The TCT version uses the `idx_ca_ancestor` index for a direct range scan with no recursion.

### All descendants with preferred terms

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT c.preferred_term
FROM concepts c
JOIN concept_ancestors a ON c.id = CAST(a.descendant_id AS TEXT)
WHERE a.ancestor_id = '22298006'
ORDER BY c.preferred_term;
EOF
```

### Descendants including self

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT c.preferred_term
FROM concepts c
WHERE c.id = '22298006'
UNION
SELECT c.preferred_term
FROM concepts c
JOIN concept_ancestors a ON c.id = CAST(a.descendant_id AS TEXT)
WHERE a.ancestor_id = '22298006'
ORDER BY preferred_term;
EOF
```

### All ancestors of a concept (root → leaf order)

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT c.preferred_term, a.depth
FROM concepts c
JOIN concept_ancestors a ON c.id = CAST(a.ancestor_id AS TEXT)
WHERE a.descendant_id = '22298006'
ORDER BY a.depth DESC;
EOF
```

This returns the full ancestor chain of Myocardial infarction ordered from the root down to the immediate parent (depth 1). Changing to `ORDER BY a.depth ASC` gives immediate-parent-first order.

### Subsumption test - is A a descendant of B?

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT CASE WHEN EXISTS (
  SELECT 1 FROM concept_ancestors
  WHERE ancestor_id  = '22298006'
    AND descendant_id = '57054005'
) THEN 'yes - is a descendant' ELSE 'no' END;
EOF
```

O(1) via the unique composite index - the core operation of any subsumption check.

### Concepts within N hops

Useful for TUI/GUI neighbourhood exploration where you want concepts "nearby" but not the full subtree:

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT c.preferred_term, a.depth
FROM concepts c
JOIN concept_ancestors a ON c.id = CAST(a.descendant_id AS TEXT)
WHERE a.ancestor_id = '22298006'
  AND a.depth <= 2
ORDER BY a.depth, c.preferred_term;
EOF
```

### Attribute-refined subsumption

Find active Clinical findings whose `finding_site` attribute is a descendant of Structure of cardiovascular system (`113257007`). With the TCT, both subsumption expansions are simple indexed JOINs rather than nested recursive CTEs:

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT DISTINCT c.preferred_term
FROM concepts c
-- must be a descendant of 'Clinical finding'
JOIN concept_ancestors cf
  ON c.id = CAST(cf.descendant_id AS TEXT)
 AND cf.ancestor_id = '404684003'
-- must have a finding_site attribute pointing into the cardiovascular system
JOIN json_each(json_extract(c.attributes, '$.finding_site')) fs
JOIN concept_ancestors cardio
  ON json_extract(fs.value, '$.id') = CAST(cardio.descendant_id AS TEXT)
 AND cardio.ancestor_id = '113257007'
WHERE c.active = 1
ORDER BY c.preferred_term
LIMIT 20;
EOF
```

Without the TCT, both the `cf` and `cardio` joins would require separate recursive CTEs, making the query significantly harder to compose and slower to execute.

### Lowest common ancestor (TCT version)

Find the most specific concept that is an ancestor of both Myocardial infarction (`22298006`) and Heart failure (`84114007`):

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT c.preferred_term, a1.depth + a2.depth AS combined_depth
FROM concept_ancestors a1
JOIN concept_ancestors a2
  ON a1.ancestor_id = a2.ancestor_id
JOIN concepts c ON c.id = CAST(a1.ancestor_id AS TEXT)
WHERE a1.descendant_id = '22298006'
  AND a2.descendant_id = '84114007'
ORDER BY combined_depth
LIMIT 5;
EOF
```

---

## Tips

- Use `sct info snomed.db` to quickly check TCT status before running subsumption-heavy queries.
- The TCT covers all concepts (active and inactive) matching the coverage of `concept_isa`. Filter `WHERE c.active = 1` in your queries if you only want active descendants.
- The `depth` column enables "shallow" subsumption - restricting to direct children (`depth = 1`) is equivalent to querying `concept_isa` directly.
- A planned SCT-QL compiler can reuse the same usable-TCT probe - see below.

---

## Reference

### Planned SCT-QL compiler integration

The planned SCT-QL query compiler (see `spec/sct-ql-spec.md`) can detect TCT usability before choosing its SQL path. A table-existence probe alone is illustrative but insufficient; production integration must reuse the engine's completion-marker, schema, index, and trigger checks:

```sql
SELECT name FROM sqlite_master WHERE type='table' AND name='concept_ancestors'
```

When implemented, the SQL emitter can replace recursive CTEs with direct JOINs when the TCT is usable and fall back transparently when it is not, keeping SCT-QL queries valid regardless of whether `--transitive-closure` was used at build time.

The intended dual-path pattern looks like this:

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

The TCT is an optimisation, not a requirement. SCT-QL queries composed of multiple `descendants of` / `ancestors of` expressions benefit most, since each expression would otherwise need its own recursive CTE.

### Build algorithm

The TCT is computed by a breadth-first traversal from every concept upward through its ancestors:

```
for each concept C in concepts:
    visited = {C}
    queue   = [(C, depth=0)]
    while queue not empty:
        (node, depth) = dequeue
        for each parent P of node (via concept_isa):
            if P not in visited:
                visited.add(P)
                insert (ancestor=P, descendant=C, depth=depth+1)
                enqueue (P, depth+1)
```

Because the traversal is BFS, the first time any ancestor is encountered for a given descendant is always via the shortest path - no `MIN(depth)` deduplication is needed. SNOMED CT is a DAG (no cycles), so the visited set purely prevents redundant work in polyhierarchies where a concept has multiple parents.

All inserts are batched inside a single SQLite transaction. Committing per-concept would be orders of magnitude slower.

### Benchmarking

Measure these after building the TCT and record results in `docs/benchmarks.md`.

Build time and database size:

```bash
# Time the TCT build
time sct tct --db snomed.db

# File size before and after
ls -lh snomed.db
```

Query timings (the `.timer on` output appears inline with results):

```bash
# Subsumption count
sqlite3 snomed.db <<EOF
.timer on
SELECT COUNT(*) FROM concept_ancestors WHERE ancestor_id = '22298006';
EOF

# Point subsumption test (should be near-instant)
sqlite3 snomed.db <<EOF
.timer on
SELECT 1 FROM concept_ancestors
WHERE ancestor_id = '22298006' AND descendant_id = '57054005'
LIMIT 1;
EOF

# Attribute-refined query (the most demanding benchmark)
sqlite3 snomed.db <<EOF
.timer on
SELECT COUNT(DISTINCT c.id)
FROM concepts c
JOIN concept_ancestors cf ON c.id = CAST(cf.descendant_id AS TEXT) AND cf.ancestor_id = '404684003'
JOIN json_each(json_extract(c.attributes, '$.finding_site')) fs
JOIN concept_ancestors cardio
  ON json_extract(fs.value, '$.id') = CAST(cardio.descendant_id AS TEXT)
 AND cardio.ancestor_id = '113257007'
WHERE c.active = 1;
EOF
```

Record: TCT row count, `snomed.db` file size with and without TCT, build time, and each query time.

---

*See also: [`sct sqlite`](sqlite.md) - build the database, [`sct info`](info.md) - inspect artefact metadata.*
