# sct map

Map clinical codes between SNOMED CT, Read v2, CTV3, ICD-10, and OPCS-4, pivoting through SNOMED CT. `sct map` unifies two jobs:

- **Show every equivalent of one code** (the old `sct crosswalk`) - one code in, its equivalents in every other terminology out.
- **Convert a stream of codes to one target terminology** (the old `sct transcode`) - the workhorse of clinical data migration (forwarding legacy Read v2 / CTV3 GP records to SNOMED, or mapping SNOMED to ICD-10 / OPCS-4 for secondary-care reporting).

`transcode` and `crosswalk` remain as **aliases**, so existing scripts keep working.

Built on the maps described in [cross-terminology mapping](https://github.com/pacharanero/sct/blob/main/spec/cross-terminology-mapping.md). `sct trud download --multi-terminology` builds the full workspace. ICD-10 / OPCS-4 need a database built with [`sct ndjson --refsets all`](ndjson.md); CTV3 works from UK RF2 SimpleMap rows; Read v2 comes from [`sct read2 import`](read2.md) over TRUD item 9.

---

## Usage

```
sct map [CODE] [--from SYS] [--to SYS] [--input FILE]
        [--forward-history] [-f FORMAT] [--db FILE]
```

**Input source and direction are independent** - that is the whole idea:

| You run | What you get |
|---|---|
| `sct map 22298006` | one code → **all** equivalents |
| `sct map 22298006 --to icd10` | one code → **just** the ICD-10 map |
| `sct map --from read2 --to snomed < codes.txt` | a **stream** → one conversion |
| `cat codes.txt \| sct map` | a stream → all equivalents per code |
| `sct map --input codes.txt --to ctv3` | a file → one conversion |

## Options

| Argument / Flag | Default | Description |
|---|---|---|
| `[CODE]` | *(stdin)* | A single code to map. Omit (or pass `-`) to read codes from stdin; one code (leading token) per line. |
| `--from <SYS>` | `snomed` | Source terminology: `snomed` \| `read2` \| `ctv3` \| `icd10` \| `opcs4`. |
| `--to <SYS>` | *(all)* | Target terminology. **Omit to show equivalents in every other terminology.** |
| `--input <FILE>` | - | Read codes from a file instead of stdin (leading token per line; `#` comments ignored). |
| `--forward-history` | off | Forward inactive SNOMED pivots to their replacement(s) (needs a database built with `--refsets all`). |
| `-f, --format <FMT>` | `text` | `text` \| `tsv` \| `csv` \| `json`. |
| `--db <FILE>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database from `sct sqlite`. |

Data goes to **stdout**; the mapped/unmapped summary goes to **stderr**, so it never pollutes a pipe.

## Formats

- **`text`** (default, human) - a readable block of equivalents for a single code, or a `code → target` line per input in conversion mode.
- **`tsv`** / **`csv`** - a header row plus one row per result. Conversion mode columns are `input, target, snomed, display`; equivalents mode has one column per terminology (source excluded). CSV quotes fields containing commas.
- **`json`** - one JSON object per input line (NDJSON), so it streams and pipes cleanly into `jq` for any number of codes.

---

## Examples

```bash
# All equivalents of a SNOMED concept (human-readable)
sct map 22298006
# 22298006  Myocardial infarction
#   read2:  G30..
#   ctv3:   X200E
#   icd10:  I21.9
#   opcs4:  (none)

# Just the ICD-10 map for one code
sct map 22298006 --to icd10          # 22298006  →  I21.9

# ICD-10 input is accepted dotted or undotted (I21.9 or I219 both resolve)
sct map I219 --from icd10 --to snomed  # I219  →  22298006

# Migrate a column of Read v2 codes to SNOMED, as TSV for a spreadsheet
cut -f1 gp_extract.tsv | sct map --from read2 --to snomed -f tsv > snomed.tsv

# Forward inactive concepts while mapping, as NDJSON for jq
sct map --from snomed --to icd10 --forward-history -f json < ids.txt | jq -r .target

# Compose with ECL: expand a value set, then map every member to ICD-10
sct ecl expand "<<73211009" | sct map --to icd10 -f tsv
```

Legacy `sct transcode …` and `sct crosswalk …` invocations continue to work unchanged (they are aliases of `sct map`). The old `--json` flag is accepted as a deprecated alias for `--format json`.

See [`spec/cross-terminology-mapping.md`](https://github.com/pacharanero/sct/blob/main/spec/cross-terminology-mapping.md) for how the maps are built and stored.
