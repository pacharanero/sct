# sct refset

Inspect SNOMED CT simple reference sets loaded into a `sct sqlite` database.

Reference sets are themselves concepts in SNOMED CT — each refset ID resolves to a row in the `concepts` table with its own preferred term, FSN, and module. `sct refset` queries the `refset_members` table (populated by `sct ndjson --refsets simple` + `sct sqlite`) and joins back to `concepts` to show human-readable output.

---

## Usage

```
sct refset <SUBCOMMAND>
```

Subcommands:

| Subcommand | Description |
|---|---|
| `list` | List all refsets with at least one loaded member, with member counts. |
| `info <ID>` | Show metadata and member count for a single refset. |
| `members <ID>` | List the concepts belonging to a refset. |

All subcommands accept `--db <PATH>` (default `snomed.db`) and `--json` for machine-readable output.

---

## Examples

### List every loaded refset

```bash
sct refset list
```

```
460 refset(s):

  [999002431000000102] AIDS (acquired immune deficiency syndrome) defining illness for adults simple reference set  (26 members)
  [999002121000000109] Accessible information - communication support simple reference set  (27 members)
  ...
  [1129631000000105] Summary Care Record exclusions simple reference set  (231 members)
  ...
```

### Show metadata for one refset

```bash
sct refset info 1129631000000105
```

```
  [1129631000000105] Summary Care Record exclusions simple reference set
  Module:  999000021000000109
  Members: 231
```

### List the concepts in a refset

```bash
sct refset members 1129631000000105 --limit 5
```

```
88380005 | Acute milk alkali syndrome (Clinical finding)
397635003 | Address (Observable entity)
959831000000105 | Adult intensive care care plan (Record artifact)
713615000 | Advance care planning declined (Situation with explicit context)
1103771000000105 | Advance care planning review offered (Situation with explicit context)
```

One line per concept, so `| wc -l` gives the true count and `| cut -d' ' -f1` extracts SCTIDs.

When a member's FSN differs from its PT, the FSN is appended after ` - FSN: ` (see **Custom format** below).

### JSON output for scripting

```bash
sct refset members 1129631000000105 --json | jq '.[] | .id'
```

### Custom format

The per-concept line format is configurable. Override it per-invocation with `--format` (and optionally `--format-fsn-suffix`), or set it globally in `~/.config/sct/config.toml`:

```toml
[format]
concept = "{id} | {pt} ({hierarchy})"
concept_fsn_suffix = " - FSN: {fsn}"
```

Template variables available in both fields:

| Token | Value |
|---|---|
| `{id}` | SCTID |
| `{pt}` | Preferred term |
| `{fsn}` | FSN with the semantic tag stripped |
| `{fsn_raw}` | FSN including the semantic tag, e.g. `Fever (finding)` |
| `{tag}` | Semantic tag alone, e.g. `finding` |
| `{hierarchy}` | Top-level hierarchy name |
| `{module}` | Module SCTID (empty for list-style commands) |
| `{effective_time}` | Effective time in `YYYYMMDD` |

The `concept_fsn_suffix` template is appended only when the concept's stripped FSN differs from its PT — that's why the default output suppresses it for concepts whose PT and FSN match. Pass an empty string (`--format-fsn-suffix ''`) to suppress it unconditionally. Unknown `{tokens}` are preserved as literal text so typos are visible.

Examples:

```bash
# Tab-separated SCTID and PT, no FSN suffix, ready for cut/awk
sct refset members 1129631000000105 \
  --format '{id}	{pt}' \
  --format-fsn-suffix ''

# SNOMED compositional-style with pipes round the FSN
sct refset members 1129631000000105 \
  --format '{id} |{fsn_raw}|' \
  --format-fsn-suffix ''
```

The same `--format` and `--format-fsn-suffix` flags are accepted by `sct lexical`, and the config file applies to both commands.

---

## When members are missing

`sct refset` only sees what `sct sqlite` loaded. If a refset has no members:

- Check that `sct ndjson` was run with `--refsets simple` (the default) — if the pipeline used `--refsets none`, no memberships were written.
- Check that the RF2 release actually contains a `der2_Refset_Simple*Snapshot*.txt` file. The International release does not include UK national refsets; you need the UK Monolith or UK Clinical release for those.
- A refset whose members are all inactive concepts will have zero rows in `refset_members` — inactive concepts are filtered at RF2 load time.

## Direct SQL queries

The `refset_members` table is a standard SQLite table, so you can query it directly for
analytics that go beyond what the CLI exposes.

Which concept appears in the most refsets?

```bash
sqlite3 snomed.db "
  SELECT rm.referenced_component_id AS concept_id,
         c.preferred_term,
         COUNT(DISTINCT rm.refset_id) AS refset_count
  FROM refset_members rm
  JOIN concepts c ON c.id = rm.referenced_component_id
  GROUP BY rm.referenced_component_id
  ORDER BY refset_count DESC
  LIMIT 10"
```

In the UK Monolith release, the winner is Generic Trimbow (a triple-therapy inhaler) with
15 refset memberships — spanning COVID extraction, QOF, prescribing safety, ePrescribing
rules, and formulary classification.

Which refsets does a specific concept belong to?

```bash
sqlite3 snomed.db "
  SELECT c.preferred_term AS refset_name
  FROM refset_members rm
  JOIN concepts c ON c.id = rm.refset_id
  WHERE rm.referenced_component_id = '34683311000001106'
  ORDER BY c.preferred_term"
```

---

## Looking up which refsets a concept belongs to

Use `sct lookup` — its output now includes a **Member of refsets** section listing every refset the concept appears in. Equivalently, the `snomed_concept` MCP tool returns a `member_of` array.

## Related MCP tools

When `sct mcp` is running, two tools query refsets:

- `snomed_refsets` — equivalent to `sct refset list`
- `snomed_refset_members` — equivalent to `sct refset members <id>`
