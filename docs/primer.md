# A SNOMED CT Primer

SNOMED CT is the standard clinical terminology used across the NHS and many other health
systems worldwide. This page explains the pieces that matter - concepts, descriptions,
relationships, reference sets, ECL, editions, and releases - in plain language, with runnable
`sct` examples for each one.

!!! tip "Two ways to read this"
    - **Coming from clinical practice or terminology governance?** Read top to bottom - each
      section builds on the last, and the "For clinical readers" boxes flag the parts that
      matter most for coding, mapping, and safety.
    - **Coming from software or data engineering?** The "For developers" boxes translate each
      idea into database terms, and every section links straight to the `sct` command that
      exposes it, so you can skim for the parts you don't already know.

---

## Concepts: the unit of meaning

A SNOMED CT **concept** is a single clinical idea - a diagnosis, a finding, a procedure, a body
structure, a substance. Every concept has a unique numeric identifier, the **SCTID** (SNOMED CT
identifier), such as `73211009` for "Diabetes mellitus".

The SCTID itself carries no clinical meaning. Its final digit is a checksum that helps detect
transcription errors, but it does not encode the concept. An SCTID never gets reused or renumbered
even if a concept is retired. What defines a concept's meaning
is its **descriptions** (the words used to refer to it) and its **relationships** (how it
connects to everything else). Two systems that both say `73211009` are guaranteed to mean the
same clinical idea, precisely because nothing about that meaning is inferred from the number.

```bash
sct lookup 73211009
```

!!! note "For clinical readers"
    This is why SNOMED CT coding is safer than free text or legacy code lists: the identifier
    is stable for the life of the concept, so a diagnosis coded today means the same thing when
    re-read in ten years, even if the preferred wording has since changed.

!!! note "For developers"
    Think of the `concepts` table as a conventional entity table: one row per SCTID, with
    `active`, `module`, `definition_status`, and denormalised
    `preferred_term`/`fsn` columns for fast reads. `sct lookup` is a keyed read against it;
    `sct lexical`/`sct fst`/`sct semantic` are the three search paths into the same table
    when you don't already have the id.

---

## Descriptions: the words for a concept

A concept can have many **descriptions** - the human-readable terms used to refer to it - but
exactly one meaning. Every concept has:

- A **Fully Specified Name (FSN)** - unambiguous, always includes a semantic tag in parentheses
  that states its semantic category, e.g. `Diabetes mellitus (disorder)`. FSNs are
  designed to never collide with each other, which is precisely why they read awkwardly in a
  sentence.
- A **Preferred Term (PT)** - the everyday wording clinicians actually use, e.g.
  `Diabetes mellitus`, chosen per language/dialect via a **language reference set** (see
  below).
- Zero or more **synonyms** - other acceptable terms for the same concept, e.g. `DM`.

```bash
# The output distinguishes preferred term from FSN
sct lookup 73211009 -f json | jq '{preferred_term, fsn}'
```

!!! note "For clinical readers"
    The PT is what you'll usually see in a record or a picking list. The FSN is what you should
    check when a term looks ambiguous - the `(disorder)` / `(finding)` / `(procedure)` tag in
    brackets tells you the concept's semantic category, which matters when two very
    different concepts happen to share a similar everyday name.

!!! note "For developers"
    Descriptions are a separate RF2 file (and a separate concern from `concepts`) because the
    same concept can have different preferred terms in different dialects - UK English and US
    English sometimes disagree on which synonym is "preferred". `sct` resolves this at build
    time via the `der2_cRefset_Language*` file for the dialect you loaded, and stores the
    result as denormalised `preferred_term`/`fsn` columns on `concepts` so lookups don't need a
    join at query time.

---

## Relationships: how concepts connect

**Relationships** link one concept to another. The most important one is **IS A**: `73211009
(Diabetes mellitus)` IS A `362969004 (Disorder of endocrine system)`. Chaining IS A relationships
builds the hierarchy that every other SNOMED CT operation depends on - subsumption, ECL's `<`
and `<<` operators, and `sct refset profile`'s "which hierarchy is this member in" breakdown all
walk this same chain.

Beyond IS A, **attribute relationships** describe a concept's properties, e.g. `Myocardial
infarction` has a `Finding site` of `Heart structure` and an `Associated morphology` of
`Infarct`. These are what makes ECL's refinement syntax (`: 363698007 = <<80891009`) possible -
you're querying the attributes, not just the hierarchy position.

```bash
# Walk the hierarchy: Diabetes mellitus and its descendants
sct ecl expand "<<73211009" | wc -l

# Attribute refinement: disorders of the heart structure
sct ecl expand "<<64572001 : 363698007 = <<80891009"
```

!!! note "For clinical readers"
    This is the machinery behind "code for the specific diagnosis, and the system already knows
    it's a kind of endocrine disorder, a kind of disease, a kind of clinical finding" - you
    don't need a separate lookup table to know that Type 2 diabetes is a form of diabetes
    mellitus; it's provable from the relationships themselves.

!!! note "For developers"
    Repeatedly walking IS A chains at query time is slow at 800k+ concepts, so `sct sqlite
    --transitive-closure` (or `sct tct` afterwards) precomputes every ancestor/descendant pair
    into an indexed table. `sct ecl` uses it automatically when present and falls back to a
    recursive CTE otherwise - see [`sct ecl`](commands/ecl.md#transitive-closure-fallback).

---

## Reference sets (refsets): named groupings

A **reference set** ("refset") is a named list of members - almost always other SNOMED CT
concepts, though the referenced component doesn't have to be a concept. Refsets do two
different jobs:

- **Grouping**, e.g. a UK "Summary Care Record exclusions" refset, or a clinical audit set for a
  specific condition.
- **Mapping**, e.g. the CTV3 simple map refset that links each SNOMED concept to its legacy Read
  v3 code, or the ICD-10 extended map refset.

Refsets are themselves concepts - a refset's own SCTID resolves to a row in `concepts` with its
own preferred term, which is why `sct refset list` can show human-readable names instead of bare
ids.

```bash
sct refset list
sct refset members 1129631000000105 --limit 5
```

!!! note "For clinical readers"
    If you've ever worked with a QOF business rule, a national extraction specification, or a
    "these codes count as diabetes for this audit" list, you've used a refset. `sct refset
    profile <id>` is a fast sanity check on any refset you're handed: it shows the hierarchy
    breakdown, so a stray cardiology code in an otherwise-respiratory set stands out
    immediately.

!!! note "For developers"
    `refset_members` is a single table keyed by `(refset_id, referenced_component_id)`; see
    [`sct refset`](commands/refset.md) for the schema and direct-SQL examples. Refsets that
    carry a payload beyond simple membership (maps, language, attribute-value) are loaded with
    `sct ndjson --refsets all` into their own dedicated tables and streams - see
    [`spec/cross-terminology-mapping.md`](https://github.com/pacharanero/sct/blob/main/spec/cross-terminology-mapping.md).

---

## ECL: querying by meaning, not by list

**Expression Constraint Language (ECL)** is SNOMED CT's query language for describing a *set* of
concepts by their position in the hierarchy or their attributes, rather than as a hand-typed
list of ids. `<<73211009` means "diabetes mellitus and every kind of it" - as new subtypes are
added in future releases, the same expression keeps matching them, with no list to maintain.

```bash
# Every kind of diabetes mellitus
sct ecl expand "<<73211009"

# Every kind of diabetes mellitus except Type 1
sct ecl expand "<<73211009 MINUS <<46635009"
```

The reverse operation, `sct ecl compress`, takes a flat list of ids (perhaps a legacy code list
inherited from a spreadsheet) and turns it back into a compact ECL expression - useful for
turning a brittle, undocumented list into a self-explaining one.

```bash
sct ecl compress --codelist legacy-diabetes.codelist --stats
```

!!! note "For clinical readers"
    A hand-maintained flat code list silently goes stale every time SNOMED CT adds a new
    subtype in a release - nobody remembers to add the new code. An ECL-defined list doesn't:
    `<<73211009` matches every future descendant automatically. `sct codelist add <file> --ecl
    "<expr>"` is the supported way to build a list this way. See
    [Why code lists?](why/why-codelists.md) for the fuller argument.

!!! note "For developers"
    Full grammar and supported/unsupported constructs (cardinality and reverse attributes are
    explicitly not yet supported - `sct` fails loudly rather than silently mis-evaluating them):
    [`sct ecl`](commands/ecl.md#supported-ecl) and [`spec/ecl.md`](https://github.com/pacharanero/sct/blob/main/spec/ecl.md).

---

## Editions: whose SNOMED CT is this?

SNOMED CT is published as an **International Edition** plus national **extensions** that add
locally relevant content. In the UK, NHS England distributes these pre-merged as the **UK
Monolith** - International content, the UK Clinical Extension, and the dm+d drug extension all
in one release, which is the simplest starting point for most `sct` users.

```bash
sct trud download --edition uk_monolith
```

!!! note "For clinical readers"
    A code that exists only in a national extension (a UK-specific refset member, for example)
    won't resolve against an International-only database from another country. If a lookup
    fails unexpectedly, check which edition built the database you're querying.

!!! note "For developers"
    Full breakdown of what's inside each UK download, directory layout, and cryptic RF2
    filenames decoded: [UK Edition Structure](uk-edition-structure.md).

---

## Releases: Snapshot, Full, and Delta

Every SNOMED CT release ships the same content three ways:

| Type | Contents | When to use it |
|---|---|---|
| **Snapshot** | Current state only - one row for the latest version of each component, whether active or inactive | Building a database with `sct sqlite` (the default and normal choice) |
| **Full** | Every version of every row ever published | Audit/history, or reconstructing a past point in time |
| **Delta** | Only the records changed since the last release | Auditing changes, or updating a system that supports Delta processing |

`sct` builds its canonical NDJSON from Snapshot releases; use `sct diff` to compare two snapshots.
SNOMED International publishes twice yearly, while UK releases are roughly monthly.

```bash
sct ndjson --rf2 ~/Downloads/uk_sct2mo_*.zip
sct diff --old old-snapshot.ndjson --new new-snapshot.ndjson
```

!!! note "For clinical readers"
    A concept's SCTID is stable across releases, but its preferred term, hierarchy position, or
    active status can change - and a concept can be **inactivated** in favour of a replacement.
    `sct diff` between two releases' NDJSON is how you'd audit exactly what changed and why a
    previously-working code list might need attention.

!!! note "For developers"
    Full breakdown of the Snapshot/Full/Delta directory layout and filename conventions:
    [UK Edition Structure](uk-edition-structure.md).

---

## Where to go next

- New to `sct` itself? Start with the [Getting Started walkthrough](walkthrough/getting-started.md).
- Building a maintainable code list? [Why code lists?](why/why-codelists.md)
- Need the UK-specific file layout in detail? [UK Edition Structure](uk-edition-structure.md)
- Full command reference: the **Commands** section in the navigation.
