Build, validate, and publish clinical code lists as plain-text `.codelist` files.

Also accessible as `sct refset` and `sct valueset`.

A code list is a curated collection of clinical codes used to identify a patient population or clinical event in a health dataset. `.codelist` files use YAML front-matter and a simple concept list body — they're designed to live in version control and be reviewed like source code.

---

## Quick start

```bash
# 1. Scaffold a new codelist
sct codelist new codelists/asthma-diagnosis.codelist \
  --title "Asthma diagnosis" --author "Your Name"

# 2. Add concepts (resolved from the database)
sct codelist add codelists/asthma-diagnosis.codelist \
  195967001 389145006 266361008 --db snomed.db

# 3. Validate
sct codelist validate codelists/asthma-diagnosis.codelist --db snomed.db

# 4. Export for use
sct codelist export codelists/asthma-diagnosis.codelist --format csv
```

---

## File format

A `.codelist` file is a UTF-8 text file in two parts: a YAML front-matter block between `---` delimiters, followed by the concept list.

```
---
id: asthma-diagnosis
title: Asthma diagnosis codes
description: SNOMED CT codes for a recorded diagnosis of asthma.
terminology: SNOMED CT
snomed_release: 20260301
created: 2026-03-28
updated: 2026-03-28
version: 1
status: draft
licence: CC-BY-4.0
copyright: Copyright 2026 RCPCH. SNOMED CT content © IHTSDO.
appropriate_use: UK primary care EHR diagnosis identification.
misuse: Do not use for secondary care — ICD-10 codes needed for HES.
authors:
  - name: Marcus Baw
    role: author
warnings:
  - code: not-universal-definition
    severity: info
    message: Developed for a specific study — may not suit all uses.
---

# concepts

# ── Asthma and variants ────────────────────────────────────────────────
195967001      Asthma
389145006      Allergic asthma
266361008      Non-allergic asthma

# ── Excluded ───────────────────────────────────────────────────────────
# 41553006      Occupational asthma  # separate pathway

# ── Pending review ─────────────────────────────────────────────────────
# ? 57607007    Irritant-induced asthma  - check with clinical lead
```

### Concept line types

| Line | Meaning |
|---|---|
| `195967001    Asthma` | Active — included in the codelist |
| `# 41553006   Occupational asthma` | Explicitly excluded — preserved for audit |
| `# ? 57607007 Irritant-induced asthma` | Pending review — flagged by `validate` |
| `# ── heading ──` | Section comment — ignored by parsers |

---

## Subcommands

### `sct codelist new <file>`

Scaffold a new `.codelist` file with all required fields and standard warnings.

```bash
sct codelist new codelists/asthma-diagnosis.codelist \
  --title "Asthma diagnosis" \
  --description "Codes for recorded asthma diagnosis" \
  --terminology "SNOMED CT" \
  --author "Marcus Baw" \
  --no-edit          # skip opening $EDITOR
```

### `sct codelist add <file> <sctid...>`

Add one or more concepts, resolved against the SNOMED CT database.

```bash
# Add individual concepts
sct codelist add codelists/asthma.codelist 195967001 389145006 --db snomed.db

# Add a concept and all its active descendants
sct codelist add codelists/asthma.codelist 195967001 \
  --db snomed.db \
  --include-descendants

# Add with an annotation
sct codelist add codelists/asthma.codelist 195967001 \
  --db snomed.db \
  --comment "confirmed by clinical lead"
```

Deduplicates silently. Bumps `version` and updates `updated` date.

### `sct codelist remove <file> <sctid>`

Move a concept from active to explicitly excluded, preserving the audit trail.

```bash
sct codelist remove codelists/asthma.codelist 41553006 \
  --comment "occupational asthma — separate pathway"
```

### `sct codelist validate <file>`

CI-ready validation. Checks:

- All active SCTIDs exist and are active in the database
- Preferred terms match the database (warns on drift)
- Pending review items (`# ?` lines) reported
- Required fields present and non-empty
- Duplicate SCTIDs
- Signoffs present if `status: published`

```bash
sct codelist validate codelists/asthma.codelist --db snomed.db
```

Exit code 0 = warnings only. Exit code 1 = errors. Suitable for CI.

### `sct codelist stats <file>`

```bash
sct codelist stats codelists/asthma.codelist --db snomed.db
```

Prints: concept count, hierarchy breakdown, leaf/intermediate ratio, excluded count, pending review count, and SNOMED release age.

### `sct codelist diff <file-a> <file-b>`

Compare two versions of a codelist:

```bash
sct codelist diff codelists/asthma-v1.codelist codelists/asthma-v2.codelist
```

Reports added, removed, moved-to-excluded, and preferred-term-changed concepts.

> Note: this compares two `.codelist` files. [`sct diff`](diff.md) compares two SNOMED releases.

### `sct codelist export <file> --format <fmt>`

```bash
sct codelist export codelists/asthma.codelist --format csv
sct codelist export codelists/asthma.codelist --format opencodelists-csv
sct codelist export codelists/asthma.codelist --format markdown --output asthma.md
```

| Format | Description |
|---|---|
| `csv` | `sctid,preferred_term` — plain CSV |
| `opencodelists-csv` | `code,term` — OpenCodelists-compatible upload format |
| `markdown` | Markdown table with front-matter metadata header |

---

## Front-matter fields

### Required

| Field | Description |
|---|---|
| `id` | Machine-readable slug matching the filename |
| `title` | Human-readable name |
| `description` | What this codelist is for |
| `terminology` | `SNOMED CT`, `ICD-10`, `dm+d`, `CTV3`, or `BNF` |
| `created` | ISO date |
| `updated` | ISO date (updated automatically by `sct codelist` commands) |
| `version` | Integer, starts at 1 |
| `status` | `draft`, `review`, or `published` |
| `licence` | SPDX identifier (e.g. `CC-BY-4.0`) |
| `copyright` | Copyright statement including SNOMED IP notice |
| `appropriate_use` | What this codelist is valid for |
| `misuse` | What this codelist must NOT be used for |

### Recommended

| Field | Description |
|---|---|
| `authors` | `name`, `orcid`, `affiliation`, `role` per contributor |
| `snomed_release` | Which SNOMED release was used (`YYYYMMDD`) — critical for reproducibility |
| `organisation` | Owning organisation |
| `warnings` | Structured warnings (see below) |
| `tags` | For discovery and grouping |

---

## Warnings

Structured warnings are surfaced consistently by tools. Each has `code`, `severity` (`info`, `caution`, `warning`), and `message`.

Standard codes auto-added by `sct codelist new`:

| Code | Added for |
|---|---|
| `not-universal-definition` | All codelists |
| `draft-not-reviewed` | `status: draft` |
| `snomed-release-age` | SNOMED CT codelists |
| `dmd-currency` | dm+d codelists |

---

## Version control workflow

```bash
sct codelist new codelists/asthma-diagnosis.codelist
git add codelists/asthma-diagnosis.codelist
git commit -m "codelist: scaffold asthma-diagnosis"

sct codelist add codelists/asthma-diagnosis.codelist \
  195967001 266361008 389145006 --db snomed.db
git commit -m "codelist: add core asthma concepts"

sct codelist validate codelists/asthma-diagnosis.codelist --db snomed.db
git tag codelist/asthma-diagnosis/v1
```

Git commits are the authoritative history. The `version` integer is a human label.

---

## Federation and sharing

`.codelist` files are plain text — they distribute trivially:

- **Git repo** — clone and `sct codelist validate` locally
- **GitHub search** — `filename:*.codelist terminology:"SNOMED CT" asthma` finds public codelists via GitHub's index (no central registry required)
- **OpenCodelists** — `sct codelist publish --to opencodelists` (coming)
