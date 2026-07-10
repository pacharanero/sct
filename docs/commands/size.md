# sct size `experimental!`

Estimate the output size of a subtree rooted at a concept. The command samples NDJSON row sizes, counts the subtree, and reports both NDJSON and SQLite size estimates for planning exports or downstream processing.

---

## Usage

```bash
sct size [--concept <SCTID>] [--sample <N>] [--tree] [--depth <N>] [--db <PATH>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--concept <SCTID>` | root concept | Starting concept ID. Falls back to the active root in filtered databases. |
| `--sample <N>` | `200` | Number of rows to sample when estimating average NDJSON row size. |
| `--tree` | *(flag)* | Also print a descendant-count tree. |
| `--depth <N>` | `2` | Maximum tree depth when `--tree` is enabled. |
| `--db <PATH>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |

---

## Example

```bash
# Estimate the size of the whole SNOMED CT tree.
sct size

# Inspect a specific subtree with a smaller sample and a tree view.
sct size --concept 404684003 --sample 100 --tree
```

---

## Output

The command reports:

- subtree concept count and percentage of the full database
- estimated NDJSON export size
- estimated proportional SQLite database size
- optional descendant counts for the subtree when `--tree` is set

---

## See also

- [`sct gui`](gui.md) - browser UI with the same size estimates in the concept detail panel
- [`sct tui`](tui.md) - keyboard UI with a toggleable size row# Concept Subtree Size Visualizer (`sct size`)

The `sct size` command displays a hierarchical tree of SNOMED CT concepts and their subtree sizes (number of transitive descendants), acting like a disk-usage analyzer (`du` / `ncdu`) for the terminology taxonomy.

---

## Usage

```bash
# Print the size distribution from the SNOMED CT root concept (default)
sct size --depth 2

# Inspect a specific sub-hierarchy (e.g. Clinical finding)
sct size --concept 404684003 --depth 3
```

---

## Limitations & Design Characteristics

When analyzing subtree sizes in SNOMED CT, keep the following characteristics and limitations in mind:

### 1. Polyhierarchy and Cumulative Math
SNOMED CT is a polyhierarchical taxonomy (a Directed Acyclic Graph, not a strict tree). This means a single concept can have multiple parent concepts (multiple inheritance).

*   **Behavior**: When printing the hierarchical size tree, a concept that is inherited via multiple parent paths will appear multiple times in the tree (under each of its parent branches).
*   **Result**: The sum of the subtree sizes of all children of a concept will usually be **larger** than the total subtree size reported on the parent concept itself. This is because any descendants with multiple parents are counted once in each path but deduplicated in the parent's absolute count.

### 2. Performance Fallback without Transitive Closure Table (TCT)
The size query utilizes a precomputed transitive closure table (`concept_ancestors`) if it exists in the SQLite database (built via `sct tct`).

*   **TCT Present (Recommended)**: Queries are extremely fast (sub-millisecond) because they leverage indexed lookups.
*   **TCT Absent (Fallback)**: If the `concept_ancestors` table is missing, `sct` falls back to running a recursive Common Table Expression (CTE) query against `concept_isa` to count descendants on the fly.
*   **Performance Impact**: Running recursive CTE queries for large hierarchies (such as the root concept or Clinical Finding) is resource-intensive and will take several seconds to complete, especially when building multiple levels of a tree. We recommend running `sct tct` on your SQLite database before exploring sizes.
