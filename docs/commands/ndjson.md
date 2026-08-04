# sct ndjson

Convert an RF2 Snapshot directory into the canonical SNOMED CT NDJSON artefact.

**This is the required first step - all other `sct` subcommands consume this output.** It joins the RF2 files once, deterministically, and writes each active concept as a single line of JSON.

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
| `--refsets <MODE>` | `simple` | Which reference sets to load. `simple` loads concept-level Simple refsets (SCR exclusion, care connect, etc.); `none` skips them; `all` additionally loads **ComplexMap**, **ExtendedMap**, **AttributeValue**, and **Association** refsets. Payload-bearing rows are preserved in `<stem>.refsets.ndjson`; history is written to `<stem>.history.ndjson`. `all` is larger and slower and therefore requires file output rather than `-o -`. See [cross-terminology mapping](https://github.com/pacharanero/sct/blob/main/spec/cross-terminology-mapping.md). |

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

> **Tip for UK users:** prefer the single, pre-merged **[UK Monolith Edition](https://isd.digital.nhs.uk/trud/users/guest/filters/0/categories/26/items/1799/releases)** from NHS TRUD (one `--rf2`) over hand-layering International + extensions. NHS merges International + UK Clinical + UK Drug + UK Pathology and *resolves the conflicts for you*; `sct` recognises it as the "UK Monolith" edition.

---

## Locale and preferred-term selection

`--locale` chooses the **dialect** of the preferred term, by selecting the SNOMED **language reference set** to honour - not just filtering on language code (GB and US English descriptions both have `languageCode` "en"; only the refset id distinguishes them):

| `--locale` | Language reference sets consulted, in priority order |
|---|---|
| `en-GB` (default) | UK National/Clinical (`999001261000000100`) → UK dm+d (`999000691000001104`) → International GB English (`900000000000508004`) |
| `en-US` | International US English (`900000000000509007`) |
| other `en-*` | GB English → US English |

A concept's preferred term is the synonym marked **Preferred** in the highest-priority refset that has an entry for it, falling back to any preferred synonym, then the FSN. Refsets absent from your input are simply skipped - so `en-GB` works correctly on an International-only release (it falls through to GB English). Concretely, `80146002` resolves to *Appendicectomy* under `en-GB` and *Appendectomy* under `en-US`.

### Layering multiple `--rf2` sources

When you repeat `--rf2`, sources are layered in argument order. Concept rows and the loaded SimpleMap, Simple, Association, Complex Map, Extended Map, and Attribute Value member families resolve repeated component/member UUIDs with **the last source winning**; for projected map, membership, and history data, a later inactive member retracts the earlier active projection. There is no Module Dependency Reference Set resolution - for robust extension-on-base layering, use a publisher-merged Edition (e.g. the UK Monolith) instead.

---

### Write to stdout (pipe into another tool)

```bash
sct ndjson --rf2 ./SnomedCT_Release/ -o - | jq 'select(.id == "22298006")'
```

`--refsets all` requires a named output file because payload and history records use companion NDJSON streams; `sct` fails rather than silently omitting them from stdout. The payload-refset stream is provenance-bound as described below.

---

## Output format

One JSON object per line, sorted by concept SCTID. Every line is a standalone JSON object - the file is valid NDJSON. The first line is a provenance record (`"_type": "sct_provenance"`) carrying the source edition, release date, the `sct` version that built the file, and a manifest of any required companion streams - every line after that is a concept record. Older (pre-provenance) NDJSON files without this header line still work; downstream `sct` commands detect the header by its `_type` tag and fall through to the concept-record path otherwise.

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
  "definition_status": "900000000000073002",
  "module": "900000000000207008",
  "effective_time": "20020131",
  "attributes": {
    "finding_site": [{"id": "302509004", "fsn": "Entire heart (body structure)"}],
    "associated_morphology": [{"id": "55641003", "fsn": "Infarct (morphologic abnormality)"}]
  },
  "ctv3_codes": ["X200E"],
  "read2_codes": [],
  "refsets": ["991381000000107"],
  "relationships": [
    {"type_id": "363698007", "destination_id": "302509004", "group": 0},
    {"type_id": "116676008", "destination_id": "55641003", "group": 0}
  ],
  "crossmaps": [
    {
      "system": "icd10",
      "code": "I219",
      "refset": "999002271000000101",
      "group": 1,
      "priority": 1,
      "advice": "ALWAYS I21.9"
    }
  ],
  "schema_version": 6
}
```

### Fields

| Field | Type | Description |
|---|---|---|
| `id` | string | SNOMED CT concept identifier (SCTID) |
| `fsn` | string | Fully Specified Name - unique, includes semantic tag in parentheses |
| `preferred_term` | string | Preferred synonym for the requested locale |
| `synonyms` | string[] | All other active synonyms (preferred term excluded) |
| `hierarchy` | string | Top-level hierarchy label (e.g. `Clinical finding`, `Procedure`) |
| `hierarchy_path` | string[] | Ancestor chain from root to this concept (semantic tags stripped) |
| `parents` | `{id, fsn}`[] | Direct IS-A parents, sorted by SCTID |
| `children_count` | integer | Number of direct IS-A children in this release |
| `active` | boolean | Always `true` unless `--include-inactive` is used |
| `definition_status` | string | RF2 definition-status SCTID: primitive or fully defined (schema v6) |
| `module` | string | SNOMED module identifier |
| `effective_time` | string | Date this concept last changed, `YYYYMMDD` |
| `attributes` | object | Named attribute groups with `{id, fsn}[]` values |
| `ctv3_codes` | string[] | CTV3 crossmap codes (UK edition only; empty array otherwise) |
| `read2_codes` | string[] | Read v2 codes (UK edition only; empty array otherwise) |
| `refsets` | string[] | SCTIDs of reference sets this concept belongs to (populated with `--refsets simple`) |
| `relationships` | `{type_id, destination_id, group}`[] | Typed attribute relationships - SCTID-keyed, with group number (schema v4). The SCTID-preserving counterpart of `attributes`; consumed by ECL attribute refinement |
| `crossmaps` | object[] | SNOMED CT → external map targets from RF2 ExtendedMap refsets (schema v5; populated with `--refsets all`) |
| `schema_version` | integer | Artefact schema version (currently `6`) |

### Payload-refset companion stream

With `--refsets all`, `sct` writes `<stem>.refsets.ndjson` and declares it in the main provenance header with its schema version, record count, and content fingerprint. The companion's first line is an `sct_refset_provenance` header containing both a fingerprint of its records and the provenance/fingerprint of the concept NDJSON it belongs to. Remaining lines are typed `sct_complex_map_refset_member`, `sct_extended_map_refset_member`, or `sct_attribute_value_refset_member` records. Each record retains the full RF2 member envelope as canonical snake-case fields (`id`, `effective_time`, `active`, `module_id`, `refset_id`, and `referenced_component_id`) plus every family payload field. This keeps inactive members, null maps, unknown map systems, and rows that reference descriptions or relationships out of the concept-only `refsets` array without losing them.

`sct sqlite` verifies both fingerprints and the release identity before loading the companion stream. A stale, mismatched, or modified sidecar fails the rebuild transaction rather than producing a mixed-release database.

The main and companion streams are fully written and synced to same-directory temporary files before publication. Companions switch first and the manifest-bearing main stream switches last; if an ordinary filesystem replacement fails, already-switched files are rolled back to the previous bundle.

### Artefact properties

- One line per active concept (inactive omitted unless `--include-inactive`)
- Stable ordering by concept ID
- `fsn`, `preferred_term`, and `synonyms` are unbounded-length strings - no truncation at 255 or any other length, so SNOMED International's July 2026 increase of the maximum description length to 4096 characters (for long multivalent-vaccine ingredient lists) needs no schema change here
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

Given the same RF2 Snapshot directory and `--locale`, the concept records in `sct ndjson`'s output - every line after the first - are always byte-for-byte identical. The first line is the provenance header (see above), which embeds a `created_at` build timestamp, so it differs between runs even against identical input. Exclude it when checksumming for reproducibility:

```bash
tail -n +2 snomed-uk-20260311.ndjson | sha256sum
```

The concept lines can be checksummed this way, committed to git-lfs, and used as a pinned dependency.

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
| `der2_Refset_Simple_*.txt` | Generic concept-level Simple reference sets (membership only, e.g. SCR exclusion); loaded with `--refsets simple` (default) or `all` |
| `der2_*Refset_ExtendedMap_*.txt` | ExtendedMap reference sets (SNOMED CT → ICD-10 / OPCS-4); loaded with `--refsets all` only |
| `der2_*Refset_ComplexMap*Snapshot*.txt` | ComplexMap reference sets; payload preserved verbatim with `--refsets all` without guessing a target system |
| `der2_cRefset_AttributeValue*Snapshot*.txt` | AttributeValue reference sets, including concept inactivation indicators; loaded with `--refsets all` only |
| `der2_cRefset_Association_*.txt` | Historical Association reference sets (inactive-concept forwarding); loaded with `--refsets all` only |

Stated relationship files (`sct2_StatedRelationship_*`) are intentionally skipped - the inferred release is used for hierarchy and attributes. Full and Delta files are ignored.

---

*Next: load into SQLite with [`sct sqlite`](sqlite.md), export to Parquet with [`sct parquet`](parquet.md), or generate embeddings with [`sct embed`](embed.md).*
