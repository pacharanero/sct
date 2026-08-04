# sct lookup

Look up a SNOMED CT concept by SCTID, reverse-resolve a CTV3 (Read v3) code, or resolve an ordered batch of codes from stdin.

**When to use:** you have an identifier and want the full concept record (preferred term, FSN, hierarchy, parents, attributes, maps). For text search, use [`sct lexical`](lexical.md) or [`sct fst`](fst.md); for set queries, use [`sct codelist add --ecl`](codelist.md).

---

## Usage

```
sct lookup <CODE|-> [--db <FILE>] [-f text|json|yaml]
```

## Options

| Argument / Flag | Default | Description |
|---|---|---|
| `<CODE>` | *(required)* | A numeric SCTID (e.g. `22298006`), a CTV3 code (e.g. `XE0Uh`) for reverse lookup via the `concept_maps` table, or `-` to read one code per line from stdin. |
| `--db <FILE>` | discovered (see [Path resolution](../path-resolution.md)) | SQLite database produced by `sct sqlite`. |
| `-f, --format <FMT>` | `text` | Output format: `text` (human), `json`, or `yaml`. (`--json` is a deprecated alias for `--format json`.) |
| `--ids` | off | Emit only the resolved SCTID(s), newline-delimited, for piping. With a CTV3 code, prints the mapped SNOMED concept id(s). Mutually exclusive with an explicit `--format`. |
| `--provenance` / `--no-provenance` | auto | Show or hide the release provenance footer (default: on for an interactive terminal). |

**Exit codes:** an unresolved `<CODE>` (unknown SCTID, or a CTV3 code with no mapping) writes a hint to stderr and exits `1`, so `sct lookup` fails loudly in scripts instead of succeeding with no output. Stdin batches are fail-closed: every input is resolved before stdout is written, so one invalid code exits `1` with empty stdout rather than leaving a partial result.

---

## Examples

```bash
# By SCTID
sct lookup 22298006

# Raw JSON (for scripting / piping to jq)
sct lookup 22298006 -f json | jq '.preferred_term'

# Reverse lookup from a CTV3 code (requires a UK Monolith-derived database)
sct lookup XE0Uh

# Explicit database
sct lookup 73211009 --db /data/snomed.db
```

```bash
# Resolve a CTV3 code to its SNOMED SCTID for piping
sct lookup XE0Uh --ids

# Resolve an ordered batch while retaining each input/result association
printf '%s\n' 22298006 XE0Uh | sct lookup - --format json
```

CTV3 reverse lookup requires a database built from a UK edition that includes the CTV3 simple map refset; on an International-only database those codes won't resolve.

## Batch input

Passing `-` reads the first whitespace-delimited token from each nonblank line, with a 64 KiB line limit, up to 10,000 entries and 100,000 retained results. Lines beginning with `#` are ignored, so both bare IDs and lines such as `22298006 |Myocardial infarction|` can be piped in. Input order and duplicates are preserved; multiple CTV3 mappings are ordered by SCTID.

Text output writes `input | resolved_sctid | preferred_term`. `--ids` flattens the resolved SCTIDs in input order and cannot be combined with an explicit `--format`. JSON and YAML emit one document shaped as `{ "items": [{ "input": "...", "result": [...] }] }`, which retains the boundary between inputs. Supplying one code directly keeps the existing single-concept output shape.
