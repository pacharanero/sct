# sct codelist

Build, validate, and manage clinical code lists as plain-text `.codelist` files.

Also accessible as `sct valueset`.

A code list is a curated collection of clinical codes used to identify a patient population or clinical event in a health dataset. `.codelist` files use YAML front-matter and a simple concept list body - they're designed to live in version control and be reviewed like source code.

---

## Quick start

```bash
# 1. Scaffold a new codelist
sct codelist new codelists/asthma-diagnosis.codelist \
  --title "Asthma diagnosis" --author "Your Name"

# 2. Add concepts (database auto-discovered - see Path resolution)
sct codelist add codelists/asthma-diagnosis.codelist \
  195967001 389145006 266361008

# 3. Validate
sct codelist validate codelists/asthma-diagnosis.codelist

# 4. Export for use
sct codelist export codelists/asthma-diagnosis.codelist --format csv
```

The `--db` flag is optional on every subcommand below - when omitted, `sct codelist` follows the [path resolution chain](../path-resolution.md) to find a SNOMED CT SQLite database (cwd → config → `$SCT_DATA_HOME/data/`). Pass `--db <path>` to override.

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
misuse: Do not use for secondary care - ICD-10 codes needed for HES.
authors:
  - name: Marcus Baw
    role: author
warnings:
  - code: not-universal-definition
    severity: info
    message: Developed for a specific study - may not suit all uses.
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
| `195967001    Asthma` | Active - included in the codelist |
| `# 41553006   Occupational asthma` | Explicitly excluded - preserved for audit |
| `# ? 57607007 Irritant-induced asthma` | Pending review - flagged by `validate` |
| `# ── heading ──` | Section comment - ignored by parsers |

---

## Composing codelists (`includes:`)

A codelist can build on others by listing them in an `includes:` front-matter key. This lets you assemble a large list from reusable building blocks - e.g. a `diabetes` list that pulls in `type-1-diabetes` and `type-2-diabetes` - transparently and in plain text, rather than opaquely via a refset or ECL query.

```yaml
includes:
  - type-1-diabetes              # bare id  -> <registry>/type-1-diabetes.codelist
  - ../shared/renal.codelist     # path     -> relative to this file
  - https://example.org/ckd.codelist   # url -> fetched and cached
```

References are resolved like Docker image names - a default registry with escape hatches for other sources:

| Reference | Resolves to |
|---|---|
| **Bare id** (`type-1-diabetes`) | `<registry>/type-1-diabetes.codelist`. The registry defaults to `./codelists`, overridable with `--codelists <dir>`, the `SCT_CODELISTS` env var, or `[codelists] dir` in the config file. |
| **Path** (contains `/`, ends in `.codelist`, or starts with `.` `~` `/`) | A file path relative to the including file (or absolute). |
| **URL** (`http(s)://`) | Fetched and cached under `$SCT_DATA_HOME/cache/codelists/`. Re-fetch with `--refresh`. |

Semantics:

- **Live, not flattened.** `includes:` stays in the file; `stats`, `validate`, `export`, `diff`, and `sct serve` compute the *effective member set* (own concepts + included concepts, recursively) on demand, so it always reflects the current source lists. Use `sct codelist resolve` to freeze a flattened snapshot.
- **Exclusions win.** An `# <id>` excluded line in the parent removes that concept even if an included list contributes it.
- **Cycles are detected** and reported as an error; `validate` fails if any include is missing or circular.

### `sct codelist include <file> <ref...>`

Add (or, with `--remove`, drop) `includes:` references. Each reference is validated to resolve before being written.

```bash
sct codelist include codelists/diabetes.codelist type-1-diabetes type-2-diabetes
sct codelist include codelists/diabetes.codelist type-2-diabetes --remove
```

### `sct codelist resolve <file>`

Flatten a composed codelist into a standalone snapshot - every effective member written inline, `includes:` dropped. Writes to stdout by default, or to `--output <file>`.

```bash
sct codelist resolve codelists/diabetes.codelist -o codelists/diabetes-flat.codelist
```

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
# Add individual concepts (DB auto-discovered)
sct codelist add codelists/asthma.codelist 195967001 389145006

# Add a concept and all its active descendants
sct codelist add codelists/asthma.codelist 195967001 \
  --include-descendants

# Add with an annotation
sct codelist add codelists/asthma.codelist 195967001 \
  --comment "confirmed by clinical lead"

# Explicitly point at a specific database
sct codelist add codelists/asthma.codelist 195967001 \
  --db /data/snomed.db

# Add every concept matched by an ECL expression
sct codelist add codelists/diabetes.codelist \
  --ecl "<<73211009"

# Read SCTIDs from stdin (the `-` form) - composes with any source
sct ecl expand "<<73211009" | sct codelist add codelists/diabetes.codelist -
```

Deduplicates silently. Bumps `version` and updates `updated` date.

#### Reading SCTIDs from stdin (`-`)

Pass `-` in place of SCTIDs to read newline-delimited SCTIDs from stdin (the leading token of each non-comment line). This makes `add` compose with any concept source - most naturally [`sct ecl expand`](ecl.md):

```bash
sct ecl expand "<<73211009 MINUS <<46635009" | sct codelist add t2dm.codelist -
```

`--ecl` and the `-` stdin form reach the same place; `--ecl` is the one-step convenience (and the only one that knows the *expression*, so it can record intent), while the pipe is the composable building block.

#### `--ecl <expression>`

Add every concept matched by a SNOMED CT [Expression Constraint Language](https://confluence.ihtsdotools.org/display/DOCECL) expression, evaluated against the database. Mutually exclusive with positional SCTIDs. This is the most powerful way to populate a codelist - `<<73211009` is "Diabetes mellitus and all its subtypes".

```bash
sct codelist add dm.codelist --ecl "<<73211009"                       # descendants-or-self
sct codelist add dm.codelist --ecl "<<73211009 MINUS <<46635009"      # exclude type 1
sct codelist add cv.codelist --ecl "<<404684003 : 363698007 = <<39057004"  # attribute refinement
sct codelist add x.codelist  --ecl "^447562003"                       # members of a refset
```

Supported operators: `<` `<<` `>` `>>` (descendants/ancestors, with/without self), `<!` `>!` (children/parents), `^` (refset member), `AND` `OR` `MINUS`, parentheses, `*` (wildcard), and attribute refinement (`focus : type = value`, comma-conjoined, with `{ }` groups and `!=`). Optional `|term|` annotations are accepted and ignored.

Hierarchy and refset expressions work on any database built by `sct sqlite`. **Attribute refinement** (the `:` operator) requires a database built with a current `sct` (schema v4+), which adds the `concept_relationships` table - rebuild with `sct ndjson` then `sct sqlite` if you see a message to that effect.

Not yet supported (clear error, never silent mis-evaluation): cardinality `[min..max]`, reverse `R` and dotted `.` attributes, and group-cardinality semantics. See [`spec/ecl.md`](https://github.com/pacharanero/sct/blob/main/spec/ecl.md).

### `sct codelist remove <file> <sctid>`

Move a concept from active to explicitly excluded, preserving the audit trail.

```bash
sct codelist remove codelists/asthma.codelist 41553006 \
  --comment "occupational asthma - separate pathway"
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
sct codelist validate codelists/asthma.codelist
```

Exit code 0 = warnings only. Exit code 1 = errors. Suitable for CI.

### `sct codelist stats <file>`

```bash
sct codelist stats codelists/asthma.codelist
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

# Append cross-terminology columns (build all maps with sct trud download --multi-terminology)
sct codelist export codelists/asthma.codelist --format csv --include-maps read2,ctv3,icd10 --db snomed.db

# FHIR R4 ValueSet resource
sct codelist export codelists/asthma.codelist --format fhir-json --output asthma.valueset.json
```

| Format | Description |
|---|---|
| `csv` | `sctid,preferred_term` - plain CSV |
| `opencodelists-csv` | `code,term` - OpenCodelists-compatible upload format |
| `markdown` | Markdown table with front-matter metadata header |
| `fhir-json` | A FHIR R4 `ValueSet` resource (extensional `compose.include.concept`) |

`--include-maps <terminologies>` (csv/markdown only) appends a column per terminology - `ctv3`, `read2`, `icd10`, `opcs4` - so a SNOMED codelist can be cross-walked to legacy and classification codes in one export. Maps are read from the general `crossmaps` table, with a fallback to the legacy `concept_maps` table for older CTV3/Read v2 databases. `sct trud download --multi-terminology` builds the full map set; manually, ICD-10 / OPCS-4 require [`sct ndjson --refsets all`](ndjson.md), and Read v2 requires [`sct read2 import`](read2.md) over TRUD item 9.

#### FHIR ValueSet export (`--format fhir-json`)

Emits the codelist as a FHIR R4 `ValueSet` resource whose `compose.include[0]` lists every effective member (composition flattened) over the SNOMED CT code system. The resource metadata is taken from the front-matter: `id`, `title`, `version`, `description`, `copyright`, and `status` (mapped onto the FHIR `draft` / `active` / `retired` / `unknown` value set). This is the **same ValueSet that [`sct serve`](serve.md) publishes** for a stored `.codelist` - the export and the served form go through one shared builder, so they never diverge.

`--url <base>` sets the canonical URL: `ValueSet.url` becomes `<base>/ValueSet/<id>`, matching how `sct serve` addresses it. When `--url` is omitted, the front-matter's `opencodelists_url` is used if present, otherwise `url` is left off (it is optional in FHIR).

```bash
# Canonical URL -> https://tx.example.nhs.uk/fhir/ValueSet/asthma
sct codelist export codelists/asthma.codelist --format fhir-json \
  --url https://tx.example.nhs.uk/fhir --output asthma.valueset.json
```

An `rf2` format (SNOMED CT Simple Reference Set) is planned but not yet implemented: producing a valid RF2 refset needs a real SNOMED CT namespace (a `refsetId`, `moduleId`, and member-row UUIDs) that a codelist does not carry, so it cannot be generated correctly without that input. Use `fhir-json` for a portable, standards-based export today.

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
| `snomed_release` | Which SNOMED release was used (`YYYYMMDD`) - critical for reproducibility |
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
  195967001 266361008 389145006
git commit -m "codelist: add core asthma concepts"

sct codelist validate codelists/asthma-diagnosis.codelist
git tag codelist/asthma-diagnosis/v1
```

Git commits are the authoritative history. The `version` integer is a human label.

---

## Federation and sharing

`.codelist` files are plain text - they distribute trivially:

- **Git repo** - clone and `sct codelist validate` locally
- **GitHub search** - `filename:*.codelist terminology:"SNOMED CT" asthma` finds public codelists via GitHub's index (no central registry required)
