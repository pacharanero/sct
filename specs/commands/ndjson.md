# `sct ndjson` — Build the canonical NDJSON artefact from an RF2 snapshot

Reads one or more RF2 Snapshot directories (or `.zip` archives) and produces a single `.ndjson`
file where each line is a self-contained JSON object representing one active SNOMED CT concept.
This is the **Layer 1 deterministic transform** — the stable intermediate that every other
`sct` command consumes.

---

## Synopsis

```bash
sct ndjson --rf2 <path> [--rf2 <path> …] [--locale <bcp47>] [--output <file>] [--include-inactive]
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--rf2 <path>` | *(required)* | RF2 Snapshot directory or `.zip` archive. Repeatable — supply base release + extensions in order. |
| `--locale <bcp47>` | `en-GB` | BCP-47 locale for preferred term selection. |
| `--output <file>` | slugified RF2 dir name | Output `.ndjson` path. Use `-` for stdout. |
| `--include-inactive` | false | Include inactive concepts (omitted by default). |

---

## Examples

```bash
# Single release
sct ndjson --rf2 ./SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/

# From a zip (extracted to a temp dir automatically)
sct ndjson --rf2 ./SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z.zip

# UK base + clinical extension layered
sct ndjson \
  --rf2 ./SnomedCT_InternationalRF2_PRODUCTION_20260301T120000Z/ \
  --rf2 ./SnomedCT_UKClinicalRF2_PRODUCTION_20260311T000001Z/ \
  --locale en-GB \
  --output snomed-uk-20260311.ndjson
```

---

## Per-concept JSON schema

```json
{
  "id": "22298006",
  "fsn": "Myocardial infarction (disorder)",
  "preferred_term": "Heart attack",
  "synonyms": ["Cardiac infarction", "Infarction of heart", "MI - Myocardial infarction"],
  "hierarchy": "Clinical finding",
  "hierarchy_path": ["SNOMED CT concept", "Clinical finding", "Disorder of cardiovascular system", "Ischemic heart disease", "Myocardial infarction"],
  "parents": [{"id": "414795007", "fsn": "Ischemic heart disease (disorder)"}],
  "children_count": 47,
  "active": true,
  "module": "900000000000207008",
  "effective_time": "20020131",
  "attributes": {
    "finding_site": [{"id": "302509004", "fsn": "Entire heart (body structure)"}],
    "associated_morphology": [{"id": "55641003", "fsn": "Infarct (morphologic abnormality)"}]
  },
  "schema_version": 1
}
```

---

## Schema versioning

Every record includes a `schema_version` integer. Consumers detect incompatible format changes
using this field. Current version: `1`. Consumers encountering an unknown version must warn or
refuse to start rather than silently misinterpreting data.

## Determinism guarantee

Given the same RF2 snapshot and locale, `sct ndjson` always produces byte-for-byte identical
output. The artefact can be checksummed, versioned alongside code, and used in reproducible
pipelines.

## Properties of the artefact

- One line per active concept (inactive omitted unless `--include-inactive`)
- Stable ordering by concept ID
- Locale-aware preferred terms
- Self-contained: each line is independently interpretable
- Greppable: `grep "22298006" snomed.ndjson`
