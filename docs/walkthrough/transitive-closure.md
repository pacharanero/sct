# Transitive Closure Table (TCT)

Precompute every ancestor-descendant pair in the SNOMED hierarchy for O(1) subsumption queries.

> **Docs**: [`sct tct`](../commands/tct.md)

---

By default, `sct sqlite` stores only direct IS-A parent-child pairs in `concept_isa`. Subsumption queries ("give me all descendants of X") require a recursive CTE at query time. The **transitive closure table** (TCT) precomputes every ancestor-descendant pair in the hierarchy so these queries become a single indexed JOIN.

The TCT is entirely optional. Because it is derived from `concept_isa` — which is already in every `sct sqlite` output — it can be added to any existing database at any time without re-reading the original NDJSON artefact.

## Build the TCT

Apply to an existing database:

```bash
sct tct --db snomed.db
# spinner: Building TCT for 831,132 concepts (5000/831132)...
# Done. 18,432,601 ancestor-descendant pairs in concept_ancestors.
```

Or build it in a single step alongside the main load:

```bash
sct sqlite --input snomed.ndjson --output snomed.db --transitive-closure
```

Both call the same underlying algorithm and produce identical output. The `--transitive-closure` flag is a convenience shorthand for pipelines that want everything in one command.

To include self-referential rows (`depth = 0`, `ancestor_id = descendant_id`) — useful if your queries always want "descendants including self":

```bash
sct tct --db snomed.db --include-self
```

## Verify with `sct info`

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

## Performance comparison

The queries below are equivalent — both return all descendants of Myocardial infarction (`22298006`) in the IS-A hierarchy. The TCT version replaces a full recursive tree-walk with a single index seek.

**Without TCT — recursive CTE (~4 ms on UK Monolith):**

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

**With TCT — indexed lookup (<1 ms on UK Monolith):**

```bash
sqlite3 snomed.db <<EOF
.timer on
SELECT COUNT(*) FROM concept_ancestors WHERE ancestor_id = '22298006';
EOF
```

Both return the same count. The TCT version is faster because the index on `ancestor_id` gives SQLite a direct range scan over a single column, with no recursion.

The performance gap grows sharply with hierarchy depth and fanout. For large ancestors (e.g. `Clinical finding` with ~300k descendants), recursive CTEs can take hundreds of milliseconds; the TCT lookup stays under 1 ms regardless of hierarchy size.

## Full subsumption query with preferred terms

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

## Subsumption test (is A a descendant of B?)

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

This is O(1) with the unique composite index — the core operation of any SNOMED subsumption check.
