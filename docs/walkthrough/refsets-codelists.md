# Refsets and Code Lists

Browse the reference sets in your SNOMED release, and build curated clinical code lists
as version-controlled plain-text files.

---

## Reference Sets

SNOMED CT uses **reference sets** (refsets) to group concepts for specific purposes — drug
safety alerts, summary care record exclusions, diagnostic imaging procedures, and hundreds
more. Each refset is itself a SNOMED CT concept, so it has its own ID, preferred term, and
module.

Once you have a SQLite database (see [Getting started](getting-started.md)), `sct refset`
lets you explore what refsets are available and list their members.

> **Docs**: [`sct refset`](../commands/refset.md)

### List all refsets in your release

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

The UK Monolith release contains around 460 simple refsets. The International release has
fewer, but the same commands work regardless of edition.

### Show metadata for a single refset

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

### JSON output for scripting

All subcommands accept `--json` for machine-readable output:

```bash
sct refset members 1129631000000105 --json | jq '.[] | .id'
```

### Custom format

The per-concept line format is configurable with `--format` (and optionally `--format-fsn-suffix`),
or globally in `~/.config/sct/config.toml`:

```bash
# Tab-separated SCTID and PT, no FSN suffix, ready for cut/awk
sct refset members 1129631000000105 \
  --format '{id}	{pt}' \
  --format-fsn-suffix ''
```

See the [`sct refset` docs](../commands/refset.md) for the full list of template variables.

### Which refsets does a concept belong to?

Use `sct lookup` — its output includes a **Member of refsets** section listing every refset
the concept appears in. The `snomed_concept` MCP tool returns the same data as a `member_of` array.

### When members are missing

`sct refset` only sees what `sct sqlite` loaded. If a refset has no members:

- Check that `sct ndjson` was run with `--refsets simple` (the default) — if the pipeline used `--refsets none`, no memberships were written.
- Check that the RF2 release actually contains a `der2_Refset_Simple*Snapshot*.txt` file. The International release does not include UK national refsets; you need the UK Monolith or UK Clinical release for those.
- A refset whose members are all inactive concepts will have zero rows in `refset_members` — inactive concepts are filtered at RF2 load time.

---

## Code Lists

Manage curated collections of clinical codes as plain-text `.codelist` files with YAML
front-matter — designed to live in version control and be reviewed like source code.

Also accessible as `sct refset` and `sct valueset`.

### Scaffold a new codelist

```bash
sct codelist new codelists/asthma-diagnosis.codelist \
  --title "Asthma diagnosis" \
  --author "Marcus Baw" \
  --terminology "SNOMED CT"
```

Creates the file with full YAML front-matter (id, title, description, licence, warnings, etc.)
and opens it in `$EDITOR`. Pass `--no-edit` to skip the editor.

### Add concepts

```bash
# Add single concepts by SCTID (resolved against the database)
sct codelist add codelists/asthma-diagnosis.codelist 195967001 389145006 --db snomed.db

# Add a concept plus all its active descendants
sct codelist add codelists/asthma-diagnosis.codelist 195967001 \
  --db snomed.db \
  --include-descendants
```

### Remove (exclude) a concept

```bash
sct codelist remove codelists/asthma-diagnosis.codelist 41553006 \
  --comment "occupational asthma — separate pathway"
```

Moves the line to a commented exclusion record, preserving the audit trail:

```
# 41553006      Occupational asthma  # occupational asthma — separate pathway
```

### Validate (CI-ready)

```bash
sct codelist validate codelists/asthma-diagnosis.codelist --db snomed.db
```

Checks: all SCTIDs exist and are active, preferred terms match the database (warns on
drift), pending review items, required fields, duplicate SCTIDs.

Exit code 0 = warnings only. Exit code 1 = errors. Suitable for CI.

### Stats

```bash
sct codelist stats codelists/asthma-diagnosis.codelist --db snomed.db
```

Prints concept count, hierarchy breakdown, leaf vs. intermediate ratio, excluded count,
and SNOMED release age.

### Diff two codelists

```bash
sct codelist diff codelists/asthma-v1.codelist codelists/asthma-v2.codelist
```

Reports added, removed, moved-to-excluded, and preferred-term-changed concepts.

### Export

```bash
sct codelist export codelists/asthma-diagnosis.codelist --format csv
sct codelist export codelists/asthma-diagnosis.codelist --format opencodelists-csv
sct codelist export codelists/asthma-diagnosis.codelist --format markdown --output asthma.md
```

### Typical git workflow

```bash
sct codelist new codelists/asthma-diagnosis.codelist
git add codelists/asthma-diagnosis.codelist
git commit -m "codelist: scaffold asthma-diagnosis"

sct codelist add codelists/asthma-diagnosis.codelist 195967001 266361008 389145006 --db snomed.db
git commit -m "codelist: add core asthma concepts"

sct codelist validate codelists/asthma-diagnosis.codelist --db snomed.db
git tag codelist/asthma-diagnosis/v1
```
