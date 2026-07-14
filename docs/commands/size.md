# sct size `experimental!`

Estimate the output size of a concept subtree before you export it. `sct size` counts every concept in the subtree, samples NDJSON row sizes to project the `sct ndjson` export size, and estimates the proportional SQLite database size. It can also print a `du`-style descendant-count tree, acting like a disk-usage analyzer (`du` / `ncdu`) for the terminology taxonomy.

---

## Usage

```bash
sct size [--concept <SCTID>] [--sample <N>] [--tree] [--depth <N>] [--format <FMT>] [--build-tct] [--db <PATH>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--concept <SCTID>` | root concept | Starting concept ID. Falls back to the single active root detected in filtered/subset databases. |
| `--sample <N>` | `200` | Number of rows to sample when estimating the average NDJSON row size. |
| `--tree` | *(flag)* | Also print a `du`-style descendant-count tree. Text output only. |
| `--depth <N>` | `2` | Maximum tree depth when `--tree` is enabled. |
| `--format <FMT>` | `text` | Output format: `text`, `json`, or `yaml`. `--tree` is honoured for `text` only. |
| `--build-tct` | *(flag)* | Build a transitive closure table (TCT) without prompting, if one is missing. For scripts and non-interactive shells. |
| `--db <PATH>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |

---

## Transitive closure table (TCT)

Without a precomputed transitive closure table (`concept_ancestors`), the subtree count falls back to a recursive Common Table Expression (CTE) over the whole IS-A hierarchy. For large subtrees (especially the SNOMED CT root), this is unusably slow.

When `sct size` detects a missing TCT, it offers to build one interactively:

```text
`sct size` needs a transitive closure table (TCT) to perform adequately.
Build a TCT now (increases the database on disk by approx. ~2.1 MB)? [Y/n]
```

Answering "yes" (or just pressing Enter) builds the TCT in-place and proceeds with the fast estimate. Answering "no" continues with the slow recursive CTE.

The prompt is skipped (and the slow path used) when:

- `--format json` or `--format yaml` is given (machine output must not be polluted)
- stdin or stderr is not a terminal (CI, scripts, pipes)
- `--build-tct` is given (builds without prompting)

For non-interactive use, `--build-tct` skips the prompt and builds the TCT if missing. It has no effect if a TCT already exists.

---

## Examples

```bash
# Estimate the size of the whole SNOMED CT tree.
sct size

# Inspect a specific subtree with a smaller sample and a tree view.
sct size --concept 404684003 --sample 100 --tree

# Emit machine-readable estimates for scripting.
sct size --concept 404684003 --format json
```

---

## Output

The command reports:

- the subtree concept count and its percentage of the full database
- the estimated NDJSON export size
- the estimated proportional SQLite database size
- optional descendant counts for the subtree when `--tree` is set (text output only)

With `--format json` or `--format yaml`, the same figures are emitted as a structured record (including both raw byte counts and human-readable strings) and the tree view is skipped.

---

## Limitations and design characteristics

Keep the following characteristics in mind when interpreting the estimates.

### 1. NDJSON estimate is a deliberate lower bound

The NDJSON size is derived by sampling rows and approximating each line's byte length from the concept's stored text columns. It intentionally excludes `refsets`, `relationships`, and `crossmaps`, which live in separate tables, so a real `sct ndjson` line is somewhat larger than the per-row average reported here. Treat the NDJSON figure as a floor, not an exact size. Increasing `--sample` improves the average's stability but does not change what it measures.

### 2. Polyhierarchy and cumulative math

SNOMED CT is a polyhierarchical taxonomy (a directed acyclic graph, not a strict tree): a single concept can have multiple parents. When `--tree` prints the hierarchical view, a concept reached via multiple parent paths appears under each of those branches. As a result, the sum of the children's subtree sizes is usually **larger** than the subtree size reported on the parent itself, because descendants with multiple parents are counted once per path but deduplicated in the parent's absolute count.

### 3. Performance depends on the transitive closure table (TCT)

The subtree count uses the precomputed transitive closure table (`concept_ancestors`) when it exists, built via `sct tct`.

- **TCT present (recommended)**: counts are near-instant because they use indexed lookups.
- **TCT absent (fallback)**: `sct` runs a recursive Common Table Expression (CTE) against `concept_isa` to count descendants on the fly, and prints a warning suggesting you build the TCT.
- **Impact**: recursive CTE queries for large hierarchies (the root, or Clinical finding) can take several seconds, especially when `--tree` expands multiple levels. Run `sct tct --db <db>` once before exploring sizes.

---

## See also

- [`sct gui`](gui.md) - browser UI with the same size estimates in the concept detail panel
- [`sct tui`](tui.md) - keyboard UI with a toggleable size row
