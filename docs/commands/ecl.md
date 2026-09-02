# sct ecl

Evaluate a SNOMED CT [Expression Constraint Language](https://confluence.ihtsdotools.org/display/DOCECL) (ECL) expression against the database and emit the matching concept SCTIDs.

**When to use:** you want the *set* of concepts a query selects - to pipe into another command, build a code list, feed a script, or paste into a SQL `IN (…)`. ECL `<<73211009` means "Diabetes mellitus and all its subtypes".

`sct ecl` is the reusable engine behind [`sct codelist add --ecl`](codelist.md). Because it writes plain SCTIDs to stdout, it composes with everything else (see the [composability principle](https://github.com/pacharanero/sct/blob/main/spec/spec.md)).

---

## Usage

```
sct ecl expand <EXPRESSION> [--db <FILE>] [-f text|json|yaml]
```

## Options

| Argument / Flag | Default | Description |
|---|---|---|
| `<EXPRESSION>` | *(required)* | The ECL expression, e.g. `"<<73211009"`. Pass `-` to read the expression from stdin. |
| `--db <FILE>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |
| `-f, --format <FMT>` | `text` | Output format: `text` (newline-delimited ids), `json` (array), or `yaml`. (`--json` is a deprecated alias for `--format json`.) |

stdout is the result set (one SCTID per line, or a JSON array). The human-readable match count is written to **stderr**, so it never pollutes a pipe.

## Transitive closure fallback

When the database has a usable transitive closure table (TCT), transitive hierarchy operators such as `<`, `<<`, `>`, and `>>` use indexed lookups. Without one, or when a legacy/partial table cannot prove it completed successfully, the same expressions remain correct but use slower recursive CTEs. `sct ecl` reports the unusable TCT on stderr without changing stdout:

```text
note: this database has no usable transitive-closure table, so transitive ECL hierarchy evaluation uses slower recursive CTEs. Build or repair it for a big speed-up: `sct tct --db <db>` (or use `sct sqlite --transitive-closure` when creating the database).
```

Build it once with `sct tct --db <db>`, or include it when creating a database with `sct sqlite --transitive-closure`.

---

## Examples

```bash
# Just the ids
sct ecl expand "<<73211009"

# Pipe into a code list (see `sct codelist add … -`)
sct ecl expand "<<73211009" | sct codelist add diabetes.codelist -

# Pipe into a lookup, or jq, or a file
sct ecl expand "<<73211009 MINUS <<46635009" > type2.txt
sct ecl expand "^447562003" -f json | jq length

# Read the expression itself from stdin
echo "<<404684003 : 363698007 = <<39057004" | sct ecl expand -
```

---

## `sct ecl compress` - set → ECL

The inverse of `expand`: take an explicit set of SCTIDs (a hand-curated code list) and refactor it into a compact ECL expression that re-expands to *exactly* that set. Turns a brittle flat list into a self-documenting, release-stable intensional definition.

```
sct ecl compress [<IDS>...] [--codelist <FILE>] [--intensional-only]
                 [--max-exclusions <N>] [--pretty] [--stats] [--db <FILE>]
```

| Argument / Flag | Default | Description |
|---|---|---|
| `<IDS>...` | *(stdin)* | SCTIDs to compress. Pass `-` or no ids to read newline/whitespace-delimited ids from stdin. |
| `--codelist <FILE>` | - | Compress the effective members of a `.codelist` instead of ids. |
| `--intensional-only` | off | Emit only subsumption/exclusion clauses; do **not** add literal `OR`/`MINUS` residuals. Exits non-zero if the result is not exact. |
| `--max-exclusions <N>` | `32` | Cap the number of `MINUS <<x` clauses before the remainder falls to literal residuals. |
| `--pretty` | off | Break the expression across indented lines (text output only). |
| `--no-verify` | off | Skip the re-expansion check that verifies exactness (verification is on by default and cheap). |
| `--stats` | off | Print clause counts and intensional coverage to stderr. |
| `-f, --format <FMT>` | `text` | `text` prints the ECL expression; `json`/`yaml` emit a structured object (expression plus include/exclude/residual breakdown). |
| `--db <FILE>` | discovered | SQLite database. |
| `--codelists <DIR>` | `./codelists` | Registry directory for resolving bare `includes:` ids when compressing a `--codelist` (also `$SCT_CODELISTS` / `[codelists] dir`). |

By default the result is **exact**: if the subsumption heuristic can't express the set cleanly, literal `OR id` / `MINUS id` residuals are appended so the expression provably reproduces the input (verified by re-expansion). A set with no hierarchical structure degrades gracefully to a list of `OR`ed ids - never to a wrong answer.

```bash
# A whole subtree collapses to one clause
sct ecl expand "<<73211009" | sct ecl compress -            # => <<73211009

# Subtree-minus-subtree
sct ecl expand "<<73211009 MINUS <<46635009" | sct ecl compress - --stats  # => <<73211009 MINUS <<46635009

# Round-trip: compress then expand returns the original set
sct ecl compress --codelist diabetes.codelist | sct ecl expand -
```

This is a greedy heuristic, not a proof of minimum size; `--stats` reports how much was expressed intensionally. See [`spec/commands/ecl-compress.md`](https://github.com/pacharanero/sct/blob/main/spec/commands/ecl-compress.md).

Structured output keeps the existing `includes` array and adds `include_operator`: `<<` means the IDs are subtree roots, while `^` means the single ID is an exact refset-membership cover.

The same heuristic is available directly from a `.codelist` file via `sct codelist export --format ecl` (see [`codelist.md`](codelist.md)), with no separate `compress` step.

---

## Supported ECL

| Construct | Example | Meaning |
|---|---|---|
| Self | `73211009` | the concept itself |
| Wildcard | `*` | any concept |
| Descendants | `<73211009` / `<<73211009` | descendants / descendants-or-self |
| Ancestors | `>73211009` / `>>73211009` | ancestors / ancestors-or-self |
| Children / parents | `<!73211009` / `>!73211009` | direct children / parents |
| Refset member | `^447562003` | members of the reference set |
| Boolean | `A AND B`, `A OR B`, `A MINUS B` | intersection / union / difference |
| Refinement | `<<404684003 : 363698007 = <<39057004` | attribute constraint (comma-conjoined, `{ }` groups, `!=`) |
| History supplement | `<<195967001 {{ + HISTORY-MOD }}` | add the inactive concepts historically associated with the result |

Optional `|term|` annotations are accepted and ignored. **Attribute refinement** (the `:` operator) needs a database built with schema v4+ (which adds the `concept_relationships` table); hierarchy and refset queries work on any database. Not yet supported (clear error, never silent mis-evaluation): cardinality `[min..max]`, reverse `R` and dotted `.` attributes, and the other `{{ … }}` filters. See [`spec/ecl.md`](https://github.com/pacharanero/sct/blob/main/spec/ecl.md).

## History supplements

Inactivating a concept strips its parents and attributes, so it belongs to no `<<X` set. Query several years of coded data with a plain expression and the retired codes go **silently** unmatched - no error, just fewer rows than there should be. A history supplement follows the historical association reference sets back from your active results to the retired concepts that point at them:

```sh
sct ecl expand "<<195967001 |Asthma| {{ + HISTORY-MOD }}"
```

| Profile | Follows | When |
|---|---|---|
| `{{ + HISTORY-MIN }}` | `SAME AS` | you need precision - one-to-one equivalence only |
| `{{ + HISTORY-MOD }}` | `SAME AS`, `REPLACED BY`, `WAS A`, `PARTIALLY EQUIVALENT TO` | research and audit; the usual choice |
| `{{ + HISTORY-MAX }}` | every historical association | case-finding for manual review |

A bare `{{ + HISTORY }}` means `HISTORY-MAX`, and `{{ + HISTORY ( 900000000000527005 ) }}` names reference sets explicitly. The supplement binds to the nearest preceding focus, so parenthesise to cover a whole expression: `(A OR B) {{ + HISTORY }}`.

Supplements need the `concept_history` table, which comes from `sct ndjson --refsets all` - the default `simple` mode excludes the Association reference set files. If it is missing you get an error, not an empty result.
