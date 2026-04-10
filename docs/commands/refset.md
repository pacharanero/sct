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
231 member(s):

  [88380005] Acute milk alkali syndrome
        Clinical finding
  [397635003] Address
        Observable entity
  ...
```

### JSON output for scripting

```bash
sct refset members 1129631000000105 --json | jq '.[] | .id'
```

---

## When members are missing

`sct refset` only sees what `sct sqlite` loaded. If a refset has no members:

- Check that `sct ndjson` was run with `--refsets simple` (the default) — if the pipeline used `--refsets none`, no memberships were written.
- Check that the RF2 release actually contains a `der2_Refset_Simple*Snapshot*.txt` file. The International release does not include UK national refsets; you need the UK Monolith or UK Clinical release for those.
- A refset whose members are all inactive concepts will have zero rows in `refset_members` — inactive concepts are filtered at RF2 load time.

## Looking up which refsets a concept belongs to

Use `sct lookup` — its output now includes a **Member of refsets** section listing every refset the concept appears in. Equivalently, the `snomed_concept` MCP tool returns a `member_of` array.

## Related MCP tools

When `sct mcp` is running, two tools query refsets:

- `snomed_refsets` — equivalent to `sct refset list`
- `snomed_refset_members` — equivalent to `sct refset members <id>`
