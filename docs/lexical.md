Keyword (FTS5) search over a SNOMED CT SQLite database.

**When to use:** you know the word you're looking for — a drug name, disorder term, body structure, etc. If you're searching by *meaning* rather than exact words (e.g. "sticky blood" → hypercoagulable state), use [`sct semantic`](semantic.md) instead.

Uses the full-text search index built by `sct sqlite`. Supports all FTS5 query syntax: phrase search, prefix wildcards, and boolean operators.

---

## Usage

```
sct lexical <QUERY> [--db <FILE>] [--hierarchy <NAME>] [--limit <N>]
```

## Options

| Flag | Default | Description |
|---|---|---|
| `<QUERY>` | *(required)* | Search query. Bare text is treated as a phrase; FTS5 operators are also accepted. |
| `--db <FILE>` | `snomed.db` | SQLite database produced by `sct sqlite`. |
| `--hierarchy <NAME>` | *(none)* | Restrict results to a specific top-level hierarchy. |
| `--limit <N>` | `10` | Maximum number of results. |

---

## Examples

```bash
# Simple phrase search
sct lexical "heart attack"

# Prefix wildcard (matches "myocardial", "myocardium", …)
sct lexical "myocardial*"

# Boolean operators
sct lexical "heart AND failure"
sct lexical "fracture NOT pathological"

# Restrict to a hierarchy
sct lexical "beta blocker" --hierarchy "Substance"

# Return more results
sct lexical "diabetes" --limit 25

# Use a non-default database
sct lexical "appendicitis" --db /data/snomed.db
```

---

## FTS5 query syntax

| Syntax | Meaning |
|---|---|
| `word` | Matches any concept containing that word |
| `"two words"` | Phrase match — both words adjacent in that order |
| `word*` | Prefix match — matches any word starting with `word` |
| `a AND b` | Both terms must be present |
| `a OR b` | Either term |
| `a NOT b` | `a` present, `b` absent |
| `preferred_term:word` | Search only in the preferred term column |

If you type plain text with no special characters, `sct lexical` automatically wraps it in quotes for phrase matching.

---

## Output

```
3 results for "heart attack":

  [22298006] Heart attack
        FSN: Myocardial infarction
        Clinical finding
  [57054005] Acute myocardial infarction
        Clinical finding
  [233843008] Silent myocardial infarction
        Clinical finding
```

---

*If keyword search doesn't find what you need, try [`sct semantic`](semantic.md) for meaning-based search. Next: connect Claude with [`sct mcp`](mcp.md).*

---

## See also

- [`sct semantic`](semantic.md) — semantic similarity search using vector embeddings
- [`sct sqlite`](sqlite.md) — build the SQLite database
