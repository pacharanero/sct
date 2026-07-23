# Python SDK

Status: in development under roadmap item `R2`. The package is not on PyPI yet.

The Python package wraps the same synchronous, read-only Rust SDK as the CLI. It performs no network calls, ships no terminology content, and opens a user-supplied `snomed.db` directly.

## Naming

The distribution and import names are `snomed-sct` and `snomed_sct`:

```sh
pip install snomed-sct
```

```python
from snomed_sct import Snomed
```

The shorter `sct` PyPI distribution and import name belongs to the unrelated SAR Calibration Toolbox, so this project does not claim or shadow it.

## Example

Create `snomed.db` from terminology content you are entitled to use, following [Data and licensing](data-licensing.md), then query it:

```python
from snomed_sct import Snomed

with Snomed("snomed.db") as snomed:
    concept = snomed.concept("22298006")
    hits = snomed.search("heart attack", limit=20)
    descendants = snomed.expand("<<73211009")
    relationship = snomed.subsumes("73211009", "46635009")
```

`Snomed` is a context manager and can also be closed explicitly. Query methods release the Python GIL while the Rust engine is running. Results are ordinary typed Python dictionaries and lists whose fields match the serialisable Rust SDK records.

## Initial API

- `concept()` and `concepts()` for single and batch lookup.
- `search()` with limits, hierarchy filtering, and literal-query mode.
- `expand()` for ECL.
- `children()`, `ancestors()`, `descendants()`, and `subsumes()` for hierarchy operations.
- `refsets()`, `refset()`, and `refset_members()` for reference sets.
- `map()` and `map_many()` with optional history forwarding, plus `history()`.
- `provenance()` for release identity and build metadata.

The package includes a `py.typed` marker and explicit type stubs. `DatabaseError`, `QueryError`, and `ValidationError` derive from `SctError`, so callers can handle specific failures or catch the package-wide base exception.

## Platforms

The extension uses Python's ABI3 stable ABI with a minimum of CPython 3.9. Release wheels are planned for Linux x86-64 and ARM64, macOS Intel and Apple Silicon, and Windows x86-64. The initial release is wheel-only: maturin must vendor the Python crate's path dependency when building an sdist, which would include the entire repository rather than a focused source package. Source builds remain available from the repository; revisit the sdist after the native engine is a separately packageable leaf crate.

## Local development

From the repository root:

```sh
python3 -m venv python/.venv
. python/.venv/bin/activate
python -m pip install maturin==1.14.1 pytest
maturin develop --manifest-path python/Cargo.toml --locked
SCT_PYTHON_TEST_DB=/path/to/synthetic.db pytest python/tests
```

The Python tests use the repository's licence-free synthetic RF2 fixture. They do not require or download a real SNOMED CT release.
