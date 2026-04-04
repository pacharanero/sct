# sct tct

Build a **transitive closure table** (TCT) over the SNOMED CT IS-A hierarchy in an existing SQLite database.

**When to use:** you need fast subsumption queries — "give me all descendants of X" — and want to avoid recursive CTEs at query time. The TCT trades database size for query speed: a recursive CTE takes ~4 ms per root concept; the TCT collapses that to an indexed lookup under 1 ms regardless of hierarchy depth or fanout.

The TCT is entirely optional. Because it is derived from the `concept_isa` table already present in every `sct sqlite` output, it can be added to any existing database at any time without re-reading the original NDJSON artefact.

---

## Usage

```
sct tct --db <DB> [--include-self]
```

Or in a single build step:

```
sct sqlite --input <NDJSON> --output <DB> --transitive-closure [--include-self]
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
sct sqlite --input snomed.ndjson --output snomed.db --transitive-closure
```

Both produce identical output. The `--transitive-closure` flag is a convenience for pipelines that want everything in one invocation.

With self-pairs:

```bash
sct tct --db snomed.db --include-self
```

---

## Schema

```sql
CREATE TABLE concept_ancestors (
    ancestor_id   TEXT NOT NULL,
    descendant_id TEXT NOT NULL,
    depth         INTEGER NOT NULL   -- number of IS-A hops from ancestor to descendant
);

CREATE INDEX idx_ca_ancestor   ON concept_ancestors(ancestor_id);
CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id);
CREATE UNIQUE INDEX idx_ca_pair ON concept_ancestors(ancestor_id, descendant_id);
```

The `depth` column records the minimum number of IS-A hops separating the pair. Direct parent-child pairs have `depth = 1`. If `--include-self` was used, self-referential pairs have `depth = 0`.

---

## Checking TCT presence

```bash
sct info snomed.db
```

Without TCT:

```text
IS-A edges:        504,216
TCT:               not present  (run `sct tct --db <file>` to build)
```

After `sct tct`:

```text
IS-A edges:        504,216
TCT rows:          18,432,601
```

You can also query the schema directly:

```bash
sqlite3 snomed.db \
  "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='concept_ancestors'"
# 0 = not present, 1 = present
```

---

## Rebuilding

`sct tct` refuses to run if `concept_ancestors` already contains rows, to prevent accidental double-computation. To rebuild:

```bash
sqlite3 snomed.db "DROP TABLE concept_ancestors;"
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
  JOIN concept_ancestors a ON c.id = a.descendant_id AND a.ancestor_id = '22298006'

-- with --include-self
SELECT c.preferred_term FROM concepts c
  JOIN concept_ancestors a ON c.id = a.descendant_id AND a.ancestor_id = '22298006'
```

---

## Expected sizes

| Release | IS-A edges | TCT rows (no self) | TCT rows (with self) |
|---|---|---|---|
| UK Clinical Edition (~412k concepts) | ~500k | ~5–15 M | ~5–15 M + 412k |
| UK Monolith (~831k concepts) | ~1 M | ~10–30 M | ~10–30 M + 831k |

These are estimates; measure with `sct info` and record in `docs/benchmarks.md`.

---

## Query patterns

### All descendants of a concept

Without TCT — recursive CTE (~4 ms on UK Monolith):

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

With TCT — indexed lookup (<1 ms on UK Monolith):

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
JOIN concept_ancestors a ON c.id = a.descendant_id
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
JOIN concept_ancestors a ON c.id = a.descendant_id
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
JOIN concept_ancestors a ON c.id = a.ancestor_id
WHERE a.descendant_id = '22298006'
ORDER BY a.depth DESC;
EOF
```

This returns the full ancestor chain of Myocardial infarction ordered from immediate parent (depth 1) up to the root. Reversing `ORDER BY` gives root-first.

### Subsumption test — is A a descendant of B?

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT CASE WHEN EXISTS (
  SELECT 1 FROM concept_ancestors
  WHERE ancestor_id  = '22298006'
    AND descendant_id = '57054005'
) THEN 'yes — is a descendant' ELSE 'no' END;
EOF
```

O(1) via the unique composite index — the core operation of any subsumption check.

### Concepts within N hops

Useful for TUI/GUI neighbourhood exploration where you want concepts "nearby" but not the full subtree:

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT c.preferred_term, a.depth
FROM concepts c
JOIN concept_ancestors a ON c.id = a.descendant_id
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
  ON c.id = cf.descendant_id
 AND cf.ancestor_id = '404684003'
-- must have a finding_site attribute pointing into the cardiovascular system
JOIN json_each(json_extract(c.attributes, '$.finding_site')) fs
JOIN concept_ancestors cardio
  ON json_extract(fs.value, '$.id') = cardio.descendant_id
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
JOIN concepts c ON c.id = a1.ancestor_id
WHERE a1.descendant_id = '22298006'
  AND a2.descendant_id = '84114007'
ORDER BY combined_depth
LIMIT 5;
EOF
```

---

## Tips

- The SCT-QL compiler (see `specs/tct-spec.md`) detects TCT presence at compile time and automatically uses it. Queries generated by SCT-QL are always valid whether or not the TCT is present.
- Use `sct info snomed.db` to quickly check TCT status before running subsumption-heavy queries.
- The TCT covers all concepts (active and inactive) matching the coverage of `concept_isa`. Filter `WHERE c.active = 1` in your queries if you only want active descendants.
- The `depth` column enables "shallow" subsumption — restricting to direct children (`depth = 1`) is equivalent to querying `concept_isa` directly.

---

*See also: [`sct sqlite`](sqlite.md) — build the database, [`sct info`](info.md) — inspect artefact metadata.*
