# `sct codelist` — Build, validate, and publish clinical code lists

A code list (also called a _value set_, _reference set_, or _term set_ depending on context)
is a curated collection of clinical codes used to identify a patient population or clinical
event in a health dataset. `sct codelist` manages these as plain-text `.codelist` files with
YAML front-matter, designed to live in version control and be shared via git.

Aliases `sct refset` and `sct valueset` are accepted everywhere `sct codelist` is.

---

## The `.codelist` file format

A `.codelist` file is a plain UTF-8 text file in two parts:

1. A YAML front-matter block between `---` delimiters
2. A concept list body

This mirrors the front-matter convention used by Jekyll, Hugo, and Obsidian — it renders on
GitHub and can be parsed by any YAML library.

### Example file

```
---
id: asthma-diagnosis
title: Asthma diagnosis codes
description: >
  SNOMED CT codes for a recorded diagnosis of asthma in any clinical context.
terminology: SNOMED CT
snomed_release: 20260301
authors:
  - name: Marcus Baw
    orcid: 0000-0000-0000-0000
    affiliation: RCPCH
    role: author
organisation: RCPCH
created: 2026-03-28
updated: 2026-03-28
version: 1
status: draft
licence: CC-BY-4.0
copyright: >
  Copyright 2026 RCPCH. SNOMED CT content © IHTSDO, used under NHS England national licence.
appropriate_use: >
  Identifying patients with a recorded diagnosis of asthma in UK primary care EHR systems.
misuse: >
  Do not use for secondary care — ICD-10 codes are needed for hospital episode data.
warnings:
  - code: not-universal-definition
    severity: info
    message: >
      This codelist was developed for a specific study purpose and may not meet the needs
      of other studies.
tags:
  - respiratory
  - asthma
  - primary-care
---

# concepts

# ── Asthma and variants ──────────────────────────────────────────────────────
195967001    Asthma (disorder)
401000119107 Asthma with irreversible airway obstruction (disorder)
266361008    Non-allergic asthma (disorder)
389145006    Allergic asthma (disorder)

# ── Childhood asthma ─────────────────────────────────────────────────────────
195977004    Childhood asthma (disorder)

# ── Excluded - occupational (separate pathway) ───────────────────────────────
# 41553006   Occupational asthma (disorder)

# ── Pending review ───────────────────────────────────────────────────────────
# ? 57607007  Irritant-induced asthma (disorder)  - check with clinical lead
```

---

## Front-matter schema

### Required fields

| Field | Type | Description |
|---|---|---|
| `id` | string | Machine-readable slug matching filename (lowercase, hyphens only). |
| `title` | string | Human-readable name. |
| `description` | string | What this codelist is for. |
| `terminology` | enum | `SNOMED CT`, `ICD-10`, `dm+d`, `CTV3`, `BNF`. |
| `created` | ISO date | Creation date. |
| `updated` | ISO date | Last modified — updated automatically by `sct codelist` commands. |
| `version` | integer | Logical version, starts at 1. |
| `status` | enum | `draft`, `review`, or `published`. |
| `licence` | string | SPDX identifier (e.g. `CC-BY-4.0`, `OGL-UK-3.0`). |
| `copyright` | string | Copyright statement including SNOMED IP notice where applicable. |
| `appropriate_use` | string | What this codelist is valid for. |
| `misuse` | string | What this codelist must NOT be used for. |

### Recommended fields

| Field | Type | Description |
|---|---|---|
| `authors` | list | `name`, `orcid`, `affiliation`, `role` per contributor. |
| `organisation` | string | Owning organisation. |
| `methodology` | string | How the codelist was built — inclusions, exclusions, rationale. |
| `snomed_release` | YYYYMMDD | Which SNOMED release was used — critical for reproducibility. |
| `signoffs` | list | `name`, `date`, `role`, `affiliation` per reviewer. |
| `warnings` | list | Structured warnings (see below). |
| `population` | enum/string | `all-ages`, `adult`, `paediatric`, `neonatal`. |
| `care_setting` | list | e.g. `primary-care`, `community`, `secondary-care`. |
| `tags` | list | For discovery and grouping. |

### Optional fields

| Field | Type | Description |
|---|---|---|
| `derived_from` | list | Upstream codelists used as source — `url`, `description`, `licence`. |
| `supersedes` | string | `id` of the codelist this replaces. |
| `related_codelists` | list | Companion codelists (e.g. diagnosis + medication pair). |
| `search_terms_used` | list | Search strings used during construction. |
| `opencodelists_id` | string | `owner/slug` if published to OpenCodelists. |
| `opencodelists_url` | URL | Direct link to OpenCodelists page. |
| `doi` | string | DOI if formally published (e.g. via Zenodo). |
| `references` | list | URLs to papers, guidelines, upstream sources. |
| `acknowledgements` | string | Credits for upstream work. |
| `dmd_hierarchy_levels` | list | dm+d only: which levels are included (`VTM`, `VMP`, `AMP`). |

---

## The `warnings` field

Warnings are structured so tools can surface them consistently. Each warning has `code`
(machine-readable), `severity` (`info`, `caution`, `warning`), and `message` (human-readable).

### Standard warning codes

`sct codelist new` pre-populates warnings based on terminology:

| Code | Auto-added for | Severity |
|---|---|---|
| `dmd-currency` | dm+d codelists | `warning` |
| `dmd-vmp-code-change` | dm+d codelists | `caution` |
| `not-universal-definition` | all codelists | `info` |
| `snomed-release-age` | SNOMED CT codelists older than 12 months | `caution` |
| `draft-not-reviewed` | codelists with `status: draft` | `info` |
| `paediatric-not-validated` | codelists without `population: paediatric` in paediatric use | `caution` |

Custom warnings can use any `code` string not in the standard set.

---

## Concept list body format

After the closing `---` of the front-matter:

- Active concept line: `<SCTID><whitespace><preferred term>`
- Preferred term stored for human readability; `sct codelist validate` warns if it diverges
  from the database
- `#` begins a comment — anything after `#` on a concept line is annotation
- `# <digits> term` — **explicitly excluded** concept, preserved for audit trail
- `# ? <sctid> term` — **pending review**, surfaced by `sct codelist validate`
- Blank lines and section headers (`# ── heading ──`) are ignored by parsers

---

## CLI subcommand reference

### Aliases

```bash
sct codelist <verb>    # canonical
sct refset <verb>      # alias
sct valueset <verb>    # alias
```

### `sct codelist new <filename>`

Scaffolds a new `.codelist` file with all required and recommended fields from template.
Prompts for `title`, `description`, `terminology`, `authors`. Opens `$EDITOR` on completion.
Pre-populates standard warnings based on terminology.

Flags: `--title`, `--description`, `--terminology`, `--author`, `--from-opencodelists <url>`,
`--no-edit`.

### `sct codelist add <file> <sctid> [--include-descendants] [--comment "note"]`

Resolves SCTID against `snomed.db`, appends concept + preferred term. Updates `updated` date
and bumps `version`. With `--include-descendants`, appends all active descendants.
Deduplicates silently.

### `sct codelist remove <file> <sctid> [--comment "reason"]`

Moves concept line to a commented exclusion record. Appends inline comment if `--comment` is
provided. Updates `updated` and `version`.

### `sct codelist search <file> <query>`

Searches `snomed.db` using FTS5, presents results interactively:

```
195967001  Asthma (disorder)
[i]nclude / [e]xclude / [s]kip / [q]uit / [?]more info >
```

Each decision is written to the file immediately. `?` shows hierarchy path and synonyms before
deciding.

### `sct codelist validate <file> [--db snomed.db]`

Checks:
- All SCTIDs exist and are active in the current database
- Preferred terms match current database (warns on divergence, does not error)
- Pending review items (`# ?` lines) reported as unresolved
- `snomed_release` vs current database release date
- Required fields present and non-empty
- `appropriate_use` and `misuse` not empty if `status: published`
- `signoffs` not empty if `status: published`
- Duplicate SCTIDs
- dm+d: VMP code changes since `snomed_release`

Returns exit code 0 for warnings, 1 for errors. Suitable for CI.

### `sct codelist stats <file>`

Prints: total concept count, breakdown by hierarchy, leaf vs. intermediate node ratio,
excluded concept count, pending review count, days since `snomed_release`.

### `sct codelist diff <file-a> <file-b>`

Human-readable diff: added concepts, removed concepts, concepts moved from active to excluded,
preferred term changes. Useful for comparing versions across SNOMED releases.

Note: this is distinct from `sct diff`, which compares two NDJSON artefacts at the
release level.

### `sct codelist export <file> --format <fmt>`

| Format | Description |
|---|---|
| `csv` | SCTID + term, no metadata |
| `opencodelists-csv` | OCL-compatible CSV for upload |
| `rf2` | RF2 Simple Reference Set snapshot file |
| `fhir-json` | FHIR R4 ValueSet JSON resource |
| `fhir-xml` | FHIR R4 ValueSet XML resource |
| `markdown` | Human-readable markdown table |

### `sct codelist import --from <source> <url-or-file>`

| Source | Input |
|---|---|
| `opencodelists` | OCL codelist URL — fetches CSV + metadata |
| `csv` | Local CSV with SCTID + term columns |
| `rf2` | RF2 Simple Reference Set file |
| `fhir-json` | FHIR ValueSet JSON |

### `sct codelist publish --to <destination>`

| Destination | Notes |
|---|---|
| `opencodelists` | Requires credentials in `~/.config/sct/credentials.toml` |
| `<url>` | Any `sct serve` endpoint (future) |

Sets `status: published`, records published URL in front-matter.

---

## Versioning and git

`.codelist` files are designed to live in git. The `version` integer is a logical label for
humans; git commits are the authoritative history.

```bash
# Scaffold
sct codelist new codelists/asthma-diagnosis.codelist
git add codelists/asthma-diagnosis.codelist
git commit -m "codelist: scaffold asthma-diagnosis"

# Build out
sct codelist search codelists/asthma-diagnosis.codelist "asthma"
git commit -m "codelist: add core asthma concepts"

# Validate
sct codelist validate codelists/asthma-diagnosis.codelist
git tag codelist/asthma-diagnosis/v1

# Publish
sct codelist publish codelists/asthma-diagnosis.codelist --to opencodelists
```

A Zenodo deposit of the git tag gives a citable DOI for the codelist as a research output.

---

## Federation and sharing

### Tier 1 — git repo (zero infrastructure)

Any public git repo of `.codelist` files is a valid distribution mechanism:

```bash
git clone https://github.com/rcpch/codelists
sct codelist validate rcpch-codelists/asthma-diagnosis.codelist
```

### Tier 2 — GitHub search (emergent registry, zero infrastructure)

Because `.codelist` files have consistent YAML front-matter, GitHub code search indexes them
immediately. Search across all public repos with:

```
filename:*.codelist terminology:"SNOMED CT" asthma
```

No central registry required — the registry is GitHub's index.

### Tier 3 — OpenCodelists

`sct codelist publish --to opencodelists` pushes to the existing OpenCodelists platform.
Build and iterate locally, publish when ready — same workflow as local git → GitHub.

A conversation with the Bennett Institute team about API access is needed before implementing
the publisher; they are collaborative and will likely welcome a CLI companion.

### Tier 4 — `sct serve` (future)

A future `sct serve` subcommand exposes a directory of `.codelist` files over HTTP:

```
GET /codelists          → JSON index
GET /codelists/{id}     → JSON detail + metadata
GET /codelists/{id}/csv → CSV download
GET /codelists/{id}/fhir → FHIR ValueSet JSON
GET /                   → HTML browse interface
```

A GitHub Pages site of static JSON files (generated by a GitHub Action) is a valid
`sct serve`-compatible endpoint — no server required.

---

## OpenCodelists licensing notes

**`sct` does not bundle OpenCodelists content.** The `import` command fetches at runtime from
the user's own network connection. The user's own SNOMED licence (for UK users, covered by the
NHS England national licence) covers the content they pull.

A curated index of known OpenCodelists URLs and metadata (IDs, titles, terminology type — no
SNOMED codes) may be bundled with `sct` to enable search and discovery without a network
round-trip. This index contains no clinical codes and is not subject to the SNOMED licence.
Formal arrangements with the Bennett Institute should be explored before any bulk mirroring.
