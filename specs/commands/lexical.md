# `sct lexical` — Full-text keyword search over the SNOMED CT SQLite database

Searches the FTS5 index built by `sct sqlite`. Supports the full FTS5 query syntax: phrase
search, prefix wildcards, column filters, and boolean operators.

---

## Synopsis

```bash
sct lexical <query> [--db <database>] [--hierarchy <name>] [--limit <n>]
```

## Arguments & flags

| Argument / Flag | Default | Description |
|---|---|---|
| `<query>` | *(required)* | Search query. FTS5 syntax: `"exact phrase"`, `prefix*`, `term AND term`, `term OR term`, `NOT term`. |
| `--db <file>` | `snomed.db` in cwd, or `$SCT_DB` | SQLite database produced by `sct sqlite`. |
| `--hierarchy <name>` | *(all)* | Restrict results to a top-level hierarchy (e.g. `"Clinical finding"`). |
| `--limit <n>` | `10` | Maximum number of results. |

---

## Examples

```bash
sct lexical "heart attack"
sct lexical "myocardial infarct*"
sct lexical "heart attack" --hierarchy "Clinical finding"
sct lexical "beta blocker" --limit 20 --db /data/snomed.db
```

---

## FTS5 query syntax

| Syntax | Example | Matches |
|---|---|---|
| Plain terms | `heart attack` | concepts containing both words |
| Exact phrase | `"heart attack"` | concepts containing the exact phrase |
| Prefix | `cardio*` | concepts with any word starting with "cardio" |
| Boolean AND | `heart AND failure` | concepts containing both terms |
| Boolean OR | `infarct OR infarction` | concepts containing either term |
| Boolean NOT | `asthma NOT occupational` | asthma, excluding occupational variants |

---

## Design notes

- Opens the SQLite database read-only.
- Plain text queries (no FTS5 operators) are automatically quoted to avoid parse errors on
  special characters.
- Results are ranked by FTS5 BM25 relevance.
