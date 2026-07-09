# Concept Subtree Size Visualizer (`sct size`)

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
