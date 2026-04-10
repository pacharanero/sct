# sct ndjson

Convert an RF2 Snapshot directory into the canonical SNOMED CT NDJSON artefact.

**This is the required first step — all other `sct` subcommands consume this output.** It joins the RF2 files once, deterministically, and writes each active concept as a single line of JSON.

---

## Usage

```
sct ndjson --rf2 <DIR|ZIP> [--rf2 <DIR|ZIP>...] [OPTIONS]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `--rf2 <DIR\|ZIP>` | *(required)* | RF2 Snapshot directory **or** a `.zip` release archive. Repeat to layer extensions. |
| `--locale <LOCALE>` | `en-GB` | BCP-47 locale for preferred term selection. |
| `--output <FILE>` | *(derived from RF2 dir name)* | Output NDJSON path. Use `-o -` for stdout. |
| `--include-inactive` | off | Include inactive concepts (omitted by default). |
| `--refsets <MODE>` | `simple` | Which reference sets to load. `simple` loads concept-level Simple refsets (SCR exclusion, care connect, etc.); `none` skips them; `all` is reserved for complex/map/association refsets (not yet implemented). |

---

## Examples

### UK Monolith from a downloaded zip (no manual extraction needed)

```bash
sct ndjson --rf2 SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z.zip
# Output: snomedct-monolithrf2-production-20260311t120000z.ndjson
```

### UK Monolith from an already-extracted directory

```bash
sct ndjson --rf2 SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/
```

### International release with explicit output name

```bash
sct ndjson \
  --rf2 SnomedCT_InternationalRF2_PRODUCTION_20250101T120000Z.zip \
  --locale en-US \
  --output snomed-international-20250101.ndjson
```

### Two-release UK edition (clinical + drug extension)

```bash
sct ndjson \
  --rf2 SnomedCT_UKClinicalRF2_PRODUCTION_20250401T000001Z.zip \
  --rf2 SnomedCT_UKDrugRF2_PRODUCTION_20250401T000001Z.zip \
  --locale en-GB \
  --output snomed-uk-full-20250401.ndjson
```

### Write to stdout (pipe into another tool)

```bash
sct ndjson --rf2 ./SnomedCT_Release/ -o - | jq 'select(.id == "22298006")'
```

---

## Output format

One JSON object per line, sorted by concept SCTID. Every line is a standalone JSON object — the file is valid NDJSON.

```json
{
  "id": "22298006",
  "fsn": "Myocardial infarction (disorder)",
  "preferred_term": "Heart attack",
  "synonyms": ["Cardiac infarction", "Infarction of heart", "MI - Myocardial infarction"],
  "hierarchy": "Clinical finding",
  "hierarchy_path": [
    "SNOMED CT Concept",
    "Clinical finding",
    "Disorder of cardiovascular system",
    "Ischemic heart disease",
    "Myocardial infarction"
  ],
  "parents": [{"id": "414795007", "fsn": "Ischemic heart disease (disorder)"}],
  "children_count": 47,
  "active": true,
  "module": "900000000000207008",
  "effective_time": "20020131",
  "attributes": {
    "finding_site": [{"id": "302509004", "fsn": "Entire heart (body structure)"}],
    "associated_morphology": [{"id": "55641003", "fsn": "Infarct (morphologic abnormality)"}]
  },
  "ctv3_codes": ["X200E"],
  "read2_codes": [],
  "schema_version": 2
}
```

### Fields

| Field | Type | Description |
|---|---|---|
| `id` | string | SNOMED CT concept identifier (SCTID) |
| `fsn` | string | Fully Specified Name — unique, includes semantic tag in parentheses |
| `preferred_term` | string | Preferred synonym for the requested locale |
| `synonyms` | string[] | All other active synonyms (preferred term excluded) |
| `hierarchy` | string | Top-level hierarchy label (e.g. `Clinical finding`, `Procedure`) |
| `hierarchy_path` | string[] | Ancestor chain from root to this concept (semantic tags stripped) |
| `parents` | `{id, fsn}`[] | Direct IS-A parents, sorted by SCTID |
| `children_count` | integer | Number of direct IS-A children in this release |
| `active` | boolean | Always `true` unless `--include-inactive` is used |
| `module` | string | SNOMED module identifier |
| `effective_time` | string | Date this concept last changed, `YYYYMMDD` |
| `attributes` | object | Named attribute groups with `{id, fsn}[]` values |
| `ctv3_codes` | string[] | CTV3 crossmap codes (UK edition only; empty array otherwise) |
| `read2_codes` | string[] | Read v2 codes (UK edition only; empty array otherwise) |
| `schema_version` | integer | Artefact schema version (currently `2`) |

### Artefact properties

- One line per active concept (inactive omitted unless `--include-inactive`)
- Stable ordering by concept ID
- Locale-aware preferred terms
- Self-contained: each line is independently interpretable
- Greppable: `grep "22298006" snomed.ndjson`

---

## Querying with standard tools

The artefact is designed to be queried with `jq` without any custom tooling.

```bash
# Look up a concept by SCTID
jq 'select(.id == "22298006")' snomed.ndjson

# Search by preferred term (case-insensitive)
jq 'select(.preferred_term | test("myocardial infarction"; "i"))' snomed.ndjson \
  | head -1 | jq '{id, preferred_term, hierarchy}'

# Count concepts by top-level hierarchy
jq -r '.hierarchy' snomed.ndjson | sort | uniq -c | sort -rn | head -10

# Find concepts with a specific attribute
jq 'select(.attributes.finding_site != null) | {id, preferred_term}' snomed.ndjson

# All concepts with CTV3 mappings
jq 'select(.ctv3_codes | length > 0) | {id, preferred_term, ctv3_codes}' snomed.ndjson

# Concepts modified in a specific release
jq 'select(.effective_time == "20260301") | .preferred_term' snomed.ndjson
```

---

## Which TRUD download to use

| TRUD item | Use it? | Notes |
|---|---|---|
| **Monolith Edition, RF2: Snapshot** | ✅ Recommended | International + UK clinical + dm+d in one directory. Single `--rf2` argument. |
| **Clinical Edition, RF2: Full, Snapshot & Delta** | ✅ Works | Snapshot files are used; Full and Delta ignored. |
| **Drug Extension, RF2: Full, Snapshot & Delta** | ⚠️ Supplement | Use as a second `--rf2` alongside Clinical Edition. |
| **Clinical Edition, RF2: Delta** | ❌ Won't work | No Snapshot files. |
| **Cross-map Historical Files** | ❌ Not needed | Ignored by `sct`. |

---

## Determinism

Given the same RF2 Snapshot directory and `--locale`, `sct ndjson` always produces byte-for-byte identical output:

```bash
sha256sum snomed-uk-20260311.ndjson
```

The file can be checksummed, committed to git-lfs, and used as a pinned dependency.

---

## RF2 file patterns recognised

`sct` scans the supplied directory recursively for:

| Pattern | Content |
|---|---|
| `sct2_Concept_Snapshot_*.txt` | Concept identifiers and status |
| `sct2_Description_Snapshot_*.txt` | Terms and synonyms |
| `sct2_Relationship_Snapshot_*.txt` | IS-A and attribute relationships (inferred) |
| `der2_cRefset_Language_*.txt` | Language reference sets (preferred term acceptability) |
| `der2_sRefset_SimpleMap_*.txt` | Simple map reference sets (CTV3/Read v2 crossmaps) |

Stated relationship files (`sct2_StatedRelationship_*`) are intentionally skipped — the inferred release is used for hierarchy and attributes. Full and Delta files are ignored.

---

*Next: load into SQLite with [`sct sqlite`](sqlite.md), export to Parquet with [`sct parquet`](parquet.md), or generate embeddings with [`sct embed`](embed.md).*
