# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

from __future__ import annotations

import os
from pathlib import Path

import pytest


@pytest.fixture(scope="session")
def database_path() -> Path:
    value = os.environ.get("SCT_PYTHON_TEST_DB")
    if not value:
        pytest.skip("set SCT_PYTHON_TEST_DB to an sct database built from the synthetic fixture")
    path = Path(value)
    assert path.is_file()
    return path
