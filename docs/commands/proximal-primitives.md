# sct proximal-primitives

Compute a concept's **proximal primitive supertypes**: the most specific primitive concepts that subsume it (or the concept itself, if it is primitive).

**When to use:** classification and post-coordination QA, where a concept's necessary normal form is expressed in terms of its proximal primitive supertypes plus refinements. Every concept has at least one, since the root concept (`138875005`) is primitive and subsumes everything. For the full ancestor chain instead, use [`sct diagram --view ancestors`](diagram.md); for a raw subsumption test between two concepts, use the SDK's `Snomed::subsumes`.

---

## Usage

```
sct proximal-primitives <CONCEPT> [--db <FILE>] [-f text|json|yaml]
```

## Options

| Argument / Flag | Default | Description |
|---|---|---|
| `<CONCEPT>` | *(required)* | Focus concept SCTID. |
| `--db <FILE>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |
| `-f, --format <FMT>` | `text` | Output format: `text` (human), `json`, or `yaml`. |

**Requires schema v6 or later** (the `definition_status` column, added alongside RF2 `definitionStatusId`). Rebuild an older database with `sct ndjson` then `sct sqlite` to add it.

---

## Examples

```bash
# Most specific primitive ancestors of a fully-defined concept
sct proximal-primitives 22298006

# Raw JSON for scripting
sct proximal-primitives 22298006 --format json

# Explicit database
sct proximal-primitives 73211009 --db /data/snomed.db
```

Text output writes one `id<TAB>preferred_term` line per result, followed by a stderr summary line. A concept that is itself primitive returns only itself, since nothing in its ancestor set is more specific. A concept with multiple incomparable primitive ancestors (multiple inheritance from unrelated primitive branches) returns all of them.

## Algorithm

1. Collect the concept itself plus all of its transitive ancestors (using the transitive-closure table when present, otherwise a recursive CTE - see [`sct tct`](tct.md)).
2. Filter to those with `definition_status = 900000000000074008` (primitive).
3. Drop any primitive that is itself a proper ancestor of another primitive still in the set, leaving only the most specific ones.

An error means either the database predates the `definition_status` column, or (for a database with the column but incomplete data - e.g. records carried over from schema v5) no primitive ancestor could be found for the concept.
