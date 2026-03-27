# sct

Modern, fast tooling for SNOMED-CT. Bring Your Own SNOMED

---

## Why

SNOMED CT is distributed as a set of tab-separated RF2 files that require joining across multiple tables to get anything useful. This tool performs that join once, deterministically, and writes the result to a flat file you can grep, commit to git-lfs, and pass to any downstream tool without running a server. The entire healthcare industry seems to rely on remote terminology servers to do this work - with the added overhead of a TLS handshake and a REST API - this is an alternative for people who just want to query the data directly.

It's also a stepping stone towards a future where SNOMED CT can be distributed in a more usable format, without the need for custom tooling to join concepts, descriptions, relationships, and reference sets together. Democratising access to terminologies is likely to result in better adoption of standards and more innovation in tooling.

---

## Quick start

This takes you from a fresh SNOMED download to querying real terminology data in a few minutes.

### 1. Build the tool

```bash
git clone https://github.com/pacharanero/sct
cd sct
cargo install --path sct
```

### 2. Download SNOMED CT

**UK users:** log in to [NHS Digital TRUD](https://isd.digital.nhs.uk/), find **SNOMED CT UK Edition**, and download **SNOMED CT UK Monolith Edition, RF2: Snapshot**. Unzip it somewhere — we'll use `~/snomed/` here.

**Other users:** download your national edition from [MLDS](https://mlds.ihtsdotools.org/) or the [NLM](https://www.nlm.nih.gov/healthit/snomedct/us_edition.html) and unzip it.

### 3. Convert

Point `sct` at the unzipped directory. The output filename is derived automatically from the release name:

```bash
sct --rf2 .downloads/SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/
# Writing 831132 records...
# Output: snomedct-monolithrf2-production-20260311t120000z.ndjson
# Done.
```

Takes around 10 seconds. You now have a single self-contained NDJSON file.

### 4. Query it

No custom tools needed — just `jq`.

**Look up a concept by name:**
```bash
jq -c 'select(.preferred_term | test("myocardial infarction"; "i")) | {id, preferred_term, hierarchy, synonyms}' \
  snomedct-monolithrf2-production-20260311t120000z.ndjson | head -1 | jq .
```
```json
{
  "id": "22298006",
  "preferred_term": "Myocardial infarction",
  "hierarchy": "Clinical finding",
  "synonyms": ["Heart attack", "Cardiac infarction", "MI - Myocardial infarction"]
}
```

**Find all subtypes of a concept (its children):**
```bash
jq 'select(.parents[].id == "22298006") | .preferred_term' \
  snomedct-monolithrf2-production-20260311t120000z.ndjson
```

**Get the full hierarchy path for a concept:**
```bash
jq 'select(.id == "22298006") | .hierarchy_path' \
  snomedct-monolithrf2-production-20260311t120000z.ndjson
```
```json
["SNOMED CT Concept", "Clinical finding", "Disorder of cardiovascular system", "Ischemic heart disease", "Myocardial infarction"]
```

**Count concepts by top-level hierarchy:**
```bash
jq -r '.hierarchy' snomedct-monolithrf2-production-20260311t120000z.ndjson \
  | sort | uniq -c | sort -rn | head -10
```
```
 210341 Clinical finding
 160000 Procedure
  85000 Organism
  70000 Substance
  ...
```

**Find where a drug is in the hierarchy:**
```bash
jq -c 'select(.preferred_term | test("amlodipine"; "i")) | {id, preferred_term, hierarchy_path}' \
  snomedct-monolithrf2-production-20260311t120000z.ndjson | head -1 | jq .
```

---

## Prerequisites

- Rust toolchain (stable, 1.70+): [rustup.rs](https://rustup.rs)
- A licensed copy of a SNOMED CT RF2 Snapshot release

SNOMED CT is licensed. UK users are covered by the NHS England national licence via [NHS Digital TRUD](https://isd.digital.nhs.uk/). International users need an affiliate licence from [SNOMED International](https://www.snomed.org/snomed-ct/get-snomed).

---

## Build

```bash
git clone https://github.com/pacharanero/sct
cd sct
cargo build --release --manifest-path sct/Cargo.toml
# Binary at: sct/target/release/sct
```

Or install directly (from project root):

```bash
cargo install --path sct
```

---

## Usage

```
sct --rf2 <RF2_DIR> [--rf2 <RF2_DIR>...] [OPTIONS]
```

### Options

| Flag | Default | Description |
|---|---|---|
| `--rf2 <DIR>` | *(required)* | Path to an RF2 Snapshot directory. Repeat to layer extensions. |
| `--locale <LOCALE>` | `en-GB` | BCP-47 locale for preferred term selection. |
| `--output <FILE>` | *(derived from RF2 dir name)* | Output NDJSON file path. Use `-o -` for stdout. |
| `--include-inactive` | off | Include inactive concepts (omitted by default). |

---

## Which TRUD download to use

NHS Digital TRUD offers several SNOMED CT release types. Only **Snapshot** files are used by `sct` — Full and Delta files are automatically ignored if present.

| TRUD item | Use it? | Notes |
|---|---|---|
| **Monolith Edition, RF2: Snapshot** | ✅ Recommended | Contains international + UK clinical + drug extension in one download. Single `--rf2` argument. |
| **Clinical Edition, RF2: Full, Snapshot & Delta** | ✅ Works | Snapshot files are used; Full and Delta files ignored. |
| **Drug Extension, RF2: Full, Snapshot & Delta** | ⚠️ Supplement | Use as a second `--rf2` alongside the Clinical Edition if not using Monolith. |
| **Clinical Edition, RF2: Delta** | ❌ Won't work | Delta files contain only changes since the last release — no Snapshot files. |
| **Cross-map Historical Files** | ❌ Not needed | ICD-10/OPCS mapping reference sets. Ignored by `sct`. |

For most purposes, **download the Monolith Snapshot** — it's one file, one `--rf2` argument, and contains everything.

Note: `sct` also handles the `MONOSnapshot` filename variant used in the UK Monolith edition.

---

## Examples

### International release only

```bash
sct \
  --rf2 ./SnomedCT_InternationalRF2_PRODUCTION_20250101T120000Z/ \
  --locale en-US \
  --output snomed-international-20250101.ndjson
```

### UK edition (international base + UK clinical extension)

The UK release packages both in a single directory — just point `--rf2` at it:

```bash
sct \
  --rf2 ./SnomedCT_UKClinicalRF2_PRODUCTION_20250401T000001Z/ \
  --locale en-GB \
  --output snomed-uk-20250401.ndjson
```

### UK edition with drug extension layered on top

```bash
sct \
  --rf2 ./SnomedCT_UKClinicalRF2_PRODUCTION_20250401T000001Z/ \
  --rf2 ./SnomedCT_UKDrugRF2_PRODUCTION_20250401T000001Z/ \
  --locale en-GB \
  --output snomed-uk-full-20250401.ndjson
```

### Write to stdout (pipe into another tool)

```bash
sct --rf2 ./SnomedCT_Release/ | grep '"22298006"'
```

---

## Output format

Each line is a valid JSON object. Lines are ordered by concept SCTID (ascending numeric).

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
  }
}
```

### Fields

| Field | Description |
|---|---|
| `id` | SNOMED CT concept identifier (SCTID) |
| `fsn` | Fully Specified Name — unique, includes semantic tag in parentheses |
| `preferred_term` | Preferred synonym for the requested locale |
| `synonyms` | All other active synonyms (preferred term excluded) |
| `hierarchy` | Top-level hierarchy label (e.g. `Clinical finding`, `Procedure`, `Substance`) |
| `hierarchy_path` | Ancestor chain from root to this concept, using display labels (semantic tags stripped) |
| `parents` | Direct IS-A parents `[{id, fsn}]`, sorted by SCTID |
| `children_count` | Number of direct IS-A children in this release |
| `active` | Always `true` unless `--include-inactive` is used |
| `module` | SNOMED module identifier |
| `effective_time` | Date this concept last changed, `YYYYMMDD` |
| `attributes` | Named attribute groups (finding site, morphology, etc.) with `[{id, fsn}]` values |

---

## Working with the output

The file is designed to be queried with standard Unix tools without any custom binary.

```bash
# Look up a concept by SCTID (jq is more reliable than grep for top-level id)
jq 'select(.id == "22298006")' snomed.ndjson

# All procedures
grep '"hierarchy":"Procedure"' snomed.ndjson | wc -l

# Concepts with a finding_site attribute
jq 'select(.attributes.finding_site != null) | {id, preferred_term}' snomed.ndjson

# Concepts modified in the July 2024 release
jq 'select(.effective_time == "20240731") | .preferred_term' snomed.ndjson

# Check file integrity — line count should match total active concepts reported
wc -l snomed.ndjson
```

---

## Determinism

Given the same RF2 snapshot directory and the same `--locale` flag, `sct` always produces byte-for-byte identical output. This means the NDJSON file can be:

- Checksummed: `sha256sum snomed-uk-20250401.ndjson`
- Committed to git-lfs alongside application code
- Used as a pinned dependency: checking out a commit gives you the exact terminology version used

---

## Locale and preferred terms

SNOMED CT has language reference sets that define which synonym is "preferred" for a given locale. `sct` loads all language refset files present in the supplied RF2 directories, then selects preferred terms as follows:

1. An active synonym whose description ID is marked **Preferred** in the lang refset *and* whose `languageCode` matches the requested locale language tag — used as `preferred_term`
2. If no locale-matched preferred term exists, any description marked Preferred in the loaded refsets
3. If no Preferred acceptability entry exists, the FSN label (semantic tag stripped) is used

For the UK edition with `--locale en-GB`, this selects British English spellings (e.g. "Appendicectomy" rather than "Appendectomy").

---

## RF2 directory structure

`sct` scans the supplied directory recursively for files matching these patterns (Snapshot only — Full and Delta files are ignored):

| Pattern | Content |
|---|---|
| `sct2_Concept_Snapshot_*.txt` | Concept identifiers and status |
| `sct2_Description_Snapshot_*.txt` | Terms and synonyms |
| `sct2_Relationship_Snapshot_*.txt` | IS-A and attribute relationships (inferred) |
| `der2_cRefset_Language_*.txt` | Language reference sets (preferred term acceptability) |

Stated relationship files (`sct2_StatedRelationship_*`) are intentionally skipped — the inferred release is used for hierarchy and attributes.
