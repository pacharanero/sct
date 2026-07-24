# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

from pathlib import Path
from typing import Any, Dict, List, Optional

from sct_py import Snomed


def query_database(path: Path) -> None:
    with Snomed(path) as snomed:
        concept: Optional[Dict[str, Any]] = snomed.concept("22298006")
        hits: List[Dict[str, Any]] = snomed.search("heart attack", limit=20)
        descendants: List[str] = snomed.expand("<<73211009")
        relationship: str = snomed.subsumes("73211009", "46635009")

        if concept is not None:
            print(concept["preferred_term"])
        print(hits, descendants, relationship)
