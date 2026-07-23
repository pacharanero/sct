# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

from __future__ import annotations

from pathlib import Path

import pytest

from snomed_sct import DatabaseError, SctError, Snomed, ValidationError


def test_context_manager_lookup_and_provenance(database_path: Path) -> None:
    with Snomed(database_path) as snomed:
        assert not snomed.closed
        concept = snomed.concept("22298006")
        assert concept is not None
        assert concept["preferred_term"] == "Myocardial infarction"
        assert "Heart attack" in concept["synonyms"]
        assert snomed.concept("999999999") is None
        provenance = snomed.provenance()
        assert provenance is not None
        assert provenance["release_date"] == "2026-01-01"
    assert snomed.closed
    with pytest.raises(RuntimeError, match="closed"):
        snomed.concept("22298006")


def test_batch_search_and_hierarchy(database_path: Path) -> None:
    with Snomed(database_path) as snomed:
        concepts = snomed.concepts(["22298006", "999999999", "46635009"])
        assert [item and item["id"] for item in concepts] == ["22298006", None, "46635009"]
        assert snomed.search("heart attack", limit=1)[0]["id"] == "22298006"
        assert snomed.children("73211009", limit=10)[0]["id"] == "46635009"
        assert [item["id"] for item in snomed.ancestors("46635009")] == [
            "73211009",
            "404684003",
            "138875005",
        ]
        assert len(snomed.descendants("73211009", limit=1)) == 1


def test_ecl_subsumption_and_mapping(database_path: Path) -> None:
    with Snomed(database_path) as snomed:
        assert snomed.expand("<<73211009") == ["44054006", "46635009", "73211009"]
        assert snomed.subsumes("73211009", "46635009") == "subsumes"
        mappings = snomed.map("snomed", "22298006", "ctv3")
        assert mappings[0]["target"] == "X200"
        batch = snomed.map_many("snomed", ["22298006", "46635009"], "ctv3")
        assert batch[0][0]["target"] == "X200"
        assert batch[1] == []


def test_refsets_and_history(database_path: Path) -> None:
    with Snomed(database_path) as snomed:
        refsets = snomed.refsets()
        assert refsets[0]["id"] == "991381000000107"
        refset = snomed.refset("991381000000107")
        assert refset is not None
        assert refset["member_count"] == 2
        assert len(snomed.refset_members("991381000000107", limit=1)) == 1
        assert snomed.history("22298006") == []


def test_specific_errors_and_limit_validation(database_path: Path, tmp_path: Path) -> None:
    with pytest.raises(DatabaseError, match="failed to open"):
        Snomed(tmp_path / "missing.db")
    with Snomed(database_path) as snomed:
        with pytest.raises(ValidationError, match="unsupported terminology"):
            snomed.map("nope", "22298006", "ctv3")
        with pytest.raises(SctError, match="unsupported terminology"):
            snomed.map("nope", "22298006", "ctv3")
        with pytest.raises(ValidationError, match="limit"):
            snomed.search("heart", limit=0)
