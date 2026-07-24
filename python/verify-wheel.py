# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

"""Validate sct-py wheel identity, contents, version, and licence metadata."""

from __future__ import annotations

import argparse
from email.parser import Parser
from pathlib import Path
import re
import sys
from zipfile import ZipFile


PROJECT_ROOT = Path(__file__).resolve().parent.parent
EXPECTED_DISTRIBUTION = "sct-py"
EXPECTED_IMPORT = "sct_py"
FORBIDDEN_SUFFIXES = {
    ".arrow",
    ".db",
    ".fst",
    ".ndjson",
    ".parquet",
    ".rf2",
    ".sqlite",
    ".sqlite3",
    ".tsv",
    ".zip",
}


def fail(message: str) -> None:
    raise ValueError(message)


def cargo_package_value(manifest: Path, key: str) -> str:
    match = re.search(
        rf"(?ms)^\[package\]\s*.*?^{re.escape(key)}\s*=\s*\"([^\"]+)\"",
        manifest.read_text(encoding="utf-8"),
    )
    if match is None:
        fail(f"could not read package {key} from {manifest}")
    return match.group(1)


def pyproject_name(pyproject: Path) -> str:
    match = re.search(
        r'(?ms)^\[project\]\s*.*?^name\s*=\s*"([^"]+)"',
        pyproject.read_text(encoding="utf-8"),
    )
    if match is None:
        fail(f"could not read project name from {pyproject}")
    return match.group(1)


def normalized_name(value: str) -> str:
    return re.sub(r"[-_.]+", "-", value).lower()


def normalized_line_endings(value: bytes) -> bytes:
    return value.replace(b"\r\n", b"\n").replace(b"\r", b"\n")


def one_matching(names: list[str], suffix: str) -> str:
    matches = [name for name in names if name.endswith(suffix)]
    if len(matches) != 1:
        fail(f"expected one {suffix} file, found {len(matches)}")
    return matches[0]


def validate_wheel(wheel: Path, expected_version: str, licence: bytes) -> None:
    with ZipFile(wheel) as archive:
        names = archive.namelist()
        metadata_path = one_matching(names, ".dist-info/METADATA")
        wheel_path = one_matching(names, ".dist-info/WHEEL")
        metadata = Parser().parsestr(archive.read(metadata_path).decode("utf-8"))
        wheel_metadata = Parser().parsestr(archive.read(wheel_path).decode("utf-8"))

        if normalized_name(metadata["Name"]) != EXPECTED_DISTRIBUTION:
            fail(f"{wheel}: unexpected distribution name {metadata['Name']!r}")
        if metadata["Version"] != expected_version:
            fail(f"{wheel}: version {metadata['Version']} != {expected_version}")
        if metadata["License-Expression"] != "AGPL-3.0-or-later":
            fail(f"{wheel}: missing AGPL-3.0-or-later License-Expression")
        if metadata.get_all("License-File", []) != ["LICENSE"]:
            fail(f"{wheel}: expected License-File: LICENSE")

        required = {
            f"{EXPECTED_IMPORT}/__init__.py",
            f"{EXPECTED_IMPORT}/__init__.pyi",
            f"{EXPECTED_IMPORT}/py.typed",
        }
        missing = required.difference(names)
        if missing:
            fail(f"{wheel}: missing package files: {', '.join(sorted(missing))}")
        if not any(
            name.startswith(f"{EXPECTED_IMPORT}/_sct_py.")
            and name.endswith((".so", ".pyd"))
            for name in names
        ):
            fail(f"{wheel}: missing private _sct_py native extension")

        tags = wheel_metadata.get_all("Tag", [])
        if not tags or not all(tag.startswith("cp39-abi3-") for tag in tags):
            fail(f"{wheel}: expected only cp39-abi3 wheel tags, found {tags}")

        licence_path = one_matching(names, ".dist-info/licenses/LICENSE")
        packaged_licence = normalized_line_endings(archive.read(licence_path))
        if packaged_licence != normalized_line_endings(licence):
            fail(f"{wheel}: packaged licence does not match the repository LICENSE")

        forbidden = [
            name
            for name in names
            if Path(name).suffix.lower() in FORBIDDEN_SUFFIXES
            or Path(name).name.lower().startswith(("sct2_", "der2_"))
        ]
        if forbidden:
            fail(f"{wheel}: terminology or generated data found: {', '.join(forbidden)}")


def validate_release_platforms(wheels: list[Path]) -> None:
    categories = {
        "linux-x86_64": lambda name: "manylinux" in name and "x86_64" in name,
        "linux-aarch64": lambda name: "manylinux" in name and "aarch64" in name,
        "macos-x86_64": lambda name: "macosx" in name and "x86_64" in name,
        "macos-arm64": lambda name: "macosx" in name and "arm64" in name,
        "windows-x86_64": lambda name: name.endswith("win_amd64.whl"),
    }
    missing = [
        category
        for category, matches in categories.items()
        if not any(matches(wheel.name) for wheel in wheels)
    ]
    if len(wheels) != len(categories) or missing:
        fail(
            f"release requires exactly five platform wheels; missing: "
            f"{', '.join(missing) if missing else 'none'}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="release tag, e.g. v0.20.0")
    parser.add_argument("--release", action="store_true", help="require the five release platforms")
    parser.add_argument("--project-root", type=Path, default=PROJECT_ROOT)
    parser.add_argument("wheels", nargs="+", type=Path)
    args = parser.parse_args()

    project_root = args.project_root.resolve()
    root_version = cargo_package_value(project_root / "Cargo.toml", "version")
    binding_manifest = project_root / "python" / "Cargo.toml"
    binding_version = cargo_package_value(binding_manifest, "version")
    binding_name = cargo_package_value(binding_manifest, "name")
    distribution_name = pyproject_name(project_root / "python" / "pyproject.toml")

    if root_version != binding_version:
        fail(f"root version {root_version} != Python binding version {binding_version}")
    if args.tag is not None and args.tag != f"v{root_version}":
        fail(f"tag {args.tag} != v{root_version}")
    if normalized_name(binding_name) != EXPECTED_DISTRIBUTION:
        fail(f"binding crate name {binding_name!r} != {EXPECTED_DISTRIBUTION!r}")
    if normalized_name(distribution_name) != EXPECTED_DISTRIBUTION:
        fail(f"distribution name {distribution_name!r} != {EXPECTED_DISTRIBUTION!r}")

    wheels = sorted(args.wheels)
    if not wheels or any(not wheel.is_file() for wheel in wheels):
        fail("every wheel argument must be an existing file")
    if args.release:
        validate_release_platforms(wheels)

    licence = (project_root / "LICENSE").read_bytes()
    for wheel in wheels:
        validate_wheel(wheel, root_version, licence)
        print(f"validated {wheel}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(f"wheel validation failed: {error}", file=sys.stderr)
        sys.exit(1)
