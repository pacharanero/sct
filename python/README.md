# sct-py

Python bindings for the local-first [`sct`](https://github.com/pacharanero/sct) SNOMED CT engine.

Requires CPython 3.11 or newer. Python 3.9 and 3.10 are no longer supported following their end of life; earlier published wheels retain their original compatibility.

```sh
pip install sct-py
```

The package performs no network calls and contains no terminology content. Supply a `snomed.db` created from SNOMED CT content that you are licensed to use:

```python
from sct_py import Snomed

with Snomed("snomed.db") as snomed:
    concept = snomed.concept("22298006")
    hits = snomed.search("heart attack", limit=20)
    descendants = snomed.expand("<<73211009")
```

For local development:

```sh
python -m venv .venv
. .venv/bin/activate
python -m pip install maturin pytest
maturin develop --locked
pytest
```

The code is licensed under AGPL-3.0-or-later. SNOMED CT content has separate licensing requirements and is never bundled in the wheel.
