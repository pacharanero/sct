# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

"""Guard the supported Python floor in both wheel tags and package metadata."""

from pathlib import Path
import runpy
from zipfile import ZipFile

import pytest


VERIFY = runpy.run_path(str(Path(__file__).resolve().parents[1] / "verify-wheel.py"))


@pytest.mark.parametrize(
    ("requires_python", "tag", "error"),
    [
        (">=3.11", "cp311-abi3-linux_x86_64", None),
        (">=3.9", "cp311-abi3-linux_x86_64", "Requires-Python"),
        (">=3.10", "cp311-abi3-linux_x86_64", "Requires-Python"),
        (None, "cp311-abi3-linux_x86_64", "Requires-Python"),
        (">=3.11", "cp39-abi3-linux_x86_64", "wheel tags"),
        (">=3.11", "cp310-abi3-linux_x86_64", "wheel tags"),
    ],
)
def test_wheel_python_floor(
    tmp_path: Path, requires_python: str | None, tag: str, error: str | None
) -> None:
    wheel = tmp_path / "fixture.whl"
    dist_info = "sct_py-0.0.0.dist-info"
    metadata = (
        "Name: sct-py\nVersion: 0.0.0\n"
        "License-Expression: AGPL-3.0-or-later\nLicense-File: LICENSE\n"
    )
    if requires_python is not None:
        metadata += f"Requires-Python: {requires_python}\n"
    with ZipFile(wheel, "w") as archive:
        archive.writestr(f"{dist_info}/METADATA", metadata)
        archive.writestr(f"{dist_info}/WHEEL", f"Tag: {tag}\n")
        archive.writestr(f"{dist_info}/licenses/LICENSE", b"fixture licence\n")
        for name in ["__init__.py", "__init__.pyi", "py.typed", "_sct_py.abi3.so"]:
            archive.writestr(f"sct_py/{name}", b"")
    validate = VERIFY["validate_wheel"]
    if error is None:
        validate(wheel, "0.0.0", b"fixture licence\n")
    else:
        with pytest.raises(ValueError, match=error):
            validate(wheel, "0.0.0", b"fixture licence\n")
