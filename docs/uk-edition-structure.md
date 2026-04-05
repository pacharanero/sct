# UK SNOMED CT Releases: A Plain-English Guide

The official NHS TRUD documentation for SNOMED CT releases is written for a specific audience
that already knows what everything means. This page is for everyone else.

---

## What you're downloading and why it's this complicated

NHS England distributes SNOMED CT as **RF2** (Release Format 2) — a set of tab-separated text files.
The UK release is split into editions because different organisations need different subsets, and SNOMED
International requires that the International Edition be shipped separately from UK extensions.

There are effectively three things you might download from [NHS TRUD](https://isd.digital.nhs.uk/trud):

| Download | Contents | Use when |
|---|---|---|
| **UK Monolith** (`uk_sct2mo_*`) | Everything pre-merged into one flat release | You want the simplest possible starting point |
| **UK Clinical Edition** (`uk_sct2cl_*`) | International + UK Clinical + UK Refsets bundled together as separate sub-packages | You want more control, or you need the Delta files |
| **UK Drug Extension** (`uk_sct2dr_*`) | dm+d medicines extension on its own | You only need drug/prescribing concepts |

For most purposes, **the Monolith is the right choice**. It contains the same clinical content as
the Clinical Edition and is simpler to work with: one zip, one directory, done.

---

## Inside the zip: Full, Snapshot, and Delta

Every UK release ships three copies of its data. They're not three versions — they're three ways of
slicing the same version:

| Type | What it contains | Size |
|---|---|---|
| **Snapshot** | The current state — one row per active record | Moderate |
| **Full** | Every version of every row ever published, including retired ones | Very large |
| **Delta** | Only rows that changed since the last release | Small |

**For building a terminology database, use Snapshot.** Full is for audit/history purposes; Delta is
for incremental updates to an existing database.

---

## The Monolith directory tree

After extracting (`uk_sct2mo_*.zip`):

```
SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z/
└── Snapshot/
    ├── Terminology/           ← core clinical content
    │   ├── sct2_Concept_MONOSnapshot_GB_*.txt
    │   ├── sct2_Description_MONOSnapshot-en_GB_*.txt
    │   ├── sct2_Relationship_MONOSnapshot_GB_*.txt
    │   └── ...
    └── Refset/                ← extensions and cross-maps
        ├── Language/          ← preferred/acceptable terms per dialect
        │   └── der2_cRefset_LanguageMONOSnapshot-en_GB_*.txt
        ├── Map/               ← cross-maps to other terminologies (CTV3, ICD-10, etc.)
        │   ├── der2_sRefset_SimpleMapMONOSnapshot_GB_*.txt     ← CTV3 and others
        │   ├── der2_iisssciRefset_ExtendedMapMONOSnapshot_GB_*.txt  ← ICD-10 (NHS)
        │   └── ...
        ├── Content/           ← clinical groupings (e.g. care record elements)
        └── Metadata/          ← module dependency declarations etc.
```

The **Clinical Edition** zip is structurally identical, but instead of one directory it contains
four: the International Edition, UK Clinical Extension, UK Clinical Refsets, and UK Edition
(the combined view). Each has the same `Snapshot/Terminology/` + `Snapshot/Refset/` layout.

---

## Decoding the cryptic filenames

Filenames follow a rigid naming scheme. Breaking down `der2_sRefset_SimpleMapMONOSnapshot_GB_20260311.txt`:

| Part | Meaning |
|---|---|
| `der2` | Derived file (a reference set — as opposed to `sct2` which means core content) |
| `s` | Column type code (see below) |
| `Refset` | It's a reference set |
| `SimpleMap` | The type of reference set |
| `MONO` | Release tag: this came from the Monolith. UK Clinical uses `UKCL`, UK Refsets use `UKCR`, International uses `INT` |
| `Snapshot` | Release type (Snapshot/Full/Delta) |
| `GB` | Locale |
| `20260311` | Release date |

The column type codes (`s`, `iissscc`, `iisssci`, etc.) describe the column structure of the file —
`i` means integer, `s` means string, `c` means component reference. They exist for tooling validation
purposes and you can otherwise ignore them.

A simpler mapping:

| Filename fragment | What it is |
|---|---|
| `sct2_Concept_*` | All active SNOMED CT concepts |
| `sct2_Description_*` | All terms (FSN, preferred, synonyms) |
| `sct2_Relationship_*` | IS-A and other relationships |
| `sct2_StatedRelationship_*` | Authoring-time relationships (use `sct2_Relationship_*` for queries) |
| `der2_cRefset_Language*` | Which terms are preferred/acceptable in which dialect |
| `der2_sRefset_SimpleMap*` | Simple cross-maps: one SNOMED concept → one legacy code |
| `der2_iisssciRefset_ExtendedMap*` | ICD-10 mappings (one-to-many with rules) |
| `der2_iissscRefset_ComplexMap*` | Legacy ICD-9/OPCS complex maps |

---

## The SimpleMap file and CTV3

The file `der2_sRefset_SimpleMapMONOSnapshot_GB_*.txt` is a single TSV that contains several
different simple maps, identified by the `refsetId` column:

| refsetId | What it maps to |
|---|---|
| `900000000000497000` | **CTV3 (Clinical Terms Version 3)** — NHS legacy GP/secondary care codes |
| `446608001` | ICD-O-3 (oncology morphology codes, e.g. `C76.5`) |
| `82551000000107` | ICD-10 chapter M (musculoskeletal, e.g. `M20.3`) |
| `1323081000000108` | NHS COVID-19 test catalogue codes |
| `1323091000000105` | NHS COVID-19 test catalogue (second set) |

CTV3 is what most people are after for legacy migration. The `900000000000497000` refset contains
over 524,000 mappings in the current Monolith release.

Structure of each row:

```
id  effectiveTime  active  moduleId  refsetId  referencedComponentId  mapTarget
```

- `referencedComponentId` — the SNOMED CT SCTID
- `mapTarget` — the legacy code (e.g. `X200E` for CTV3)
- `active` — filter to `1` only

---

## What `sct ndjson` picks up automatically

`sct ndjson` scans the extracted directory and loads:

- All `sct2_Concept_*Snapshot*.txt` files
- All `sct2_Description_*Snapshot*.txt` files
- All `sct2_Relationship_*Snapshot*.txt` files
- All `der2_cRefset_Language*Snapshot*.txt` files (for preferred term selection)
- `der2_sRefset_SimpleMap*Snapshot*.txt` — CTV3 codes (refset `900000000000497000`)

It does **not** currently parse ICD-10, OPCS, or other maps — those are
different reference set types (ExtendedMap/ComplexMap) with more complex rule columns.

---

## Read v2 (Read Codes)

Read v2 codes are **not in current UK SNOMED CT releases**. They were phased out of NHS
distributions. If you need Read v2 mappings, historical crosswalk files exist but are not
included in TRUD downloads.

---

## Which TRUD item should I download?

- [Item 1799](https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/categories/26/items/1799/releases) — **UK Monolith** — International + UK Clinical + UK Drug (dm+d) + UK Pathology, fully merged and de-duplicated. Snapshot only.
- [Item 101](https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/categories/26/items/101/releases) — **UK Clinical Edition** — International + UK Clinical extension (no dm+d drugs). Full, Snapshot & Delta.
- [Item 105](https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/categories/26/items/105/releases) — **UK Drug Extension (dm+d)** — prescribing/medicines concepts only. Full, Snapshot & Delta.

For most clinical/GP use cases, the **Monolith (item 1799)** is recommended — it includes everything
in a single zip with no layering to deal with. Note that the Monolith ships Snapshot only; use items
101 + 105 if you need Full or Delta files.

Releases are roughly **every 6 months** (major) with minor releases approximately every 6 weeks
following the SNOMED International release cycle.
