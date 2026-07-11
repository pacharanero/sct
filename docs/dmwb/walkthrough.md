# DMWB Walkthrough

Use `sct` as a local, scriptable replacement for the terminology parts of the
NHS Data Migration Workbench (DMWB): browsing mapped terms, transcoding batches
of codes, forwarding inactive SNOMED CT concepts, exporting mapped code lists,
and serving FHIR `ConceptMap/$translate`.

!!! note "Scope"
    This walkthrough covers DMWB's terminology migration workflow: `BROWSE`,
    `QUERIES`, `TRANSCODE`, cross-terminology codelists, and the Excel add-in
    path via FHIR. It does **not** cover DMWB's EPR patient-data loading,
    miscode repair, casemix analytics, or Access GUI.

!!! warning "Read v2 status"
    Current UK RF2 releases do not contain the DMWB-unique Read v2 maps. `sct`
    imports them from TRUD item 9, **NHS Data Migration**, which provides the
    final April 2020 Read v2 maps as flat TSV files.

---

## Methodology

The replacement work treats DMWB as two separate products:

| DMWB area | `sct` treatment |
|---|---|
| Terminology browsing, crossmaps, clusters, and `TRANSCODE` | In scope. Recreated with RF2-native maps/history, TRUD item 9 Read v2 import, CLI commands, codelist exports, and FHIR `$translate`. |
| EPR patient-data loading, miscode repair, casemix graphs, and Access UI | Out of scope for `sct`'s terminology toolchain. |

The analysis method is:

1. Prefer current RF2 data where it already contains DMWB-equivalent content:
   CTV3 SimpleMap, SNOMED -> ICD-10 / OPCS-4 ExtendedMap, and Association
   history refsets.
2. Use TRUD item 9 only for the DMWB-unique legacy Read v2 maps. It is the final
   April 2020 static flat-file release. `sct read2 import` loads its primary
   clinically assured Read v2 -> SNOMED CT map into `crossmaps`.
3. Preserve the migration product's own safety metadata. For Read v2 this means
   retaining `DescriptionId`, `IS_ASSURED`, `MapId`, `EffectiveDate`, and source
   release provenance, rather than flattening everything into a bare code ->
   concept lookup.
4. Expose the same map engine consistently through `sct map` (and its
   `transcode` / `crosswalk` aliases), codelist export, FHIR `$translate`,
   and eventually MCP.

---

## Build a DMWB-ready database

The shortest path is:

```bash
sct trud download --multi-terminology
```

That downloads the UK Monolith, builds SQLite with inactive concepts and all RF2
map/history refsets, downloads TRUD item 9, and imports the final Read v2 maps
into the same database under `~/.local/share/sct/data/`.

The resulting database supports:

- SNOMED CT
- CTV3 -> SNOMED CT
- Read v2 -> SNOMED CT
- SNOMED CT -> ICD-10
- SNOMED CT -> OPCS-4
- inactive SNOMED CT forwarding through RF2 association history

### Manual build

Use the UK Monolith from NHS TRUD and ask `sct` to load all RF2 map and history
refsets:

```bash
sct trud download --edition uk_monolith \
                  --pipeline \
                  --include-inactive \
                  --refsets all
```

That command downloads the latest UK Monolith release, verifies it, builds the
NDJSON artefact, and builds the SQLite database under `~/.local/share/sct/data/`.
It does **not** overwrite an existing `./snomed.db` in your current project
directory.

This build adds the RF2-native DMWB-equivalent data:

- CTV3 -> SNOMED CT SimpleMap rows
- SNOMED CT -> ICD-10 / OPCS-4 ExtendedMap rows
- SNOMED CT association history for inactive-concept forwarding
- inactive concepts, when `--include-inactive` is used

It does not add Read v2 by itself. Add item 9 afterwards:

```bash
sct trud download --edition nhs_data_migration
sct read2 import \
  --archive ~/.local/share/sct/releases/nhs_datamigration_29.0.0_20200401000001.zip \
  --db "$DB"
```

The two important flags are:

| Flag | Why it matters |
|---|---|
| `--refsets all` | Loads ExtendedMap refsets for SNOMED CT -> ICD-10 / OPCS-4 and Association refsets for concept history. |
| `--include-inactive` | Keeps inactive concepts in the database so old records can still be looked up and forwarded. |

If you already have the RF2 zip locally, run the same build by hand:

```bash
sct ndjson --rf2 uk_sct2mo_42.2.0_20260603000001Z.zip \
           --refsets all \
           --include-inactive \
           --output snomed.ndjson

sct sqlite --ndjson snomed.ndjson --output snomed.db
```

### Check the right database

First confirm which database `sct` will use:

```bash
sct paths
```

The database resolution order deliberately prefers `./snomed.db` before the
newest database under `~/.local/share/sct/data/`. That is convenient for local
experiments, but it means an older active-only `./snomed.db` can mask the
DMWB-ready database that `sct trud --pipeline` just built.

If you used `sct trud --pipeline`, either omit `--db` from later commands and
make sure `sct paths` resolves to the new data-home database, or copy/pass that
exact path explicitly. For example:

```bash
DB="$HOME/.local/share/sct/data/uk_sct2mo_42.2.0_20260603000001z.db"
```

If you used the manual `sct ndjson` / `sct sqlite` build above, `DB=./snomed.db`
is fine:

```bash
DB=./snomed.db
```

Now check that the selected database has the DMWB-replacement tables:

```bash
: "${DB:?Set DB to the SQLite database path first}"
test -f "$DB"
sqlite3 "$DB" "select count(*) from concepts"
sqlite3 "$DB" "select name from sqlite_master where type='table' and name in ('crossmaps','concept_history')"
sqlite3 "$DB" "select count(*) from crossmaps"
sqlite3 "$DB" "select count(*) from concept_history"
sqlite3 "$DB" "select count(*) from concepts where active = 0"
sqlite3 "$DB" "select count(*) from crossmaps where source_system = 'read2'"
```

If even `concepts` does not exist, `DB` is unset, points at the wrong file, or
points at an empty SQLite file. If `concepts` exists but `crossmaps` or
`concept_history` does not, that database was not built from a current
`sct ndjson --refsets all` artefact. If the inactive-concept count is zero, it
was not built with `--include-inactive` or the selected release slice contains
no inactive concepts. The most common cause is simply checking an old local
`./snomed.db` rather than the pipeline output.

You do not have to delete any database. Use one of these patterns:

```bash
# Highest priority: make every sct command use the DMWB-ready database.
export SCT_DB="$DB"

# Or pass it explicitly.
sct map 22298006 --db "$DB"

# Or replace the local convenience filename after you are sure it is old.
mv ./snomed.db ./snomed.core-only.db
cp "$DB" ./snomed.db
```

---

## Browse one code across terminologies

DMWB's `BROWSE` screen shows equivalent codes around a SNOMED CT pivot. In `sct`,
use [`sct map`](../commands/map.md):

```bash
sct map 22298006 --db "$DB"
```

If you built via `sct trud --pipeline` and did not copy the database to
`./snomed.db`, either omit `--db` and let path resolution find it, or pass the
explicit `$DB` path from the previous section.

Typical output:

```text
22298006  Myocardial infarction
  read2:  (none)
  ctv3:   X200E
  icd10:  I219
  opcs4:  (none)
```

Start from another system when that is the code you have:

```bash
sct map X200E --from ctv3 --db "$DB"
sct map I219 --from icd10 --db "$DB"
```

Use JSON when another process needs the result:

```bash
sct map 22298006 -f json --db "$DB"
```

---

## Transcode a batch of codes

DMWB's `TRANSCODE` workflow becomes [`sct map`](../commands/map.md):
one input code per line, one output row per mapping.

```bash
cat ctv3-codes.txt | sct map --from ctv3 --to snomed --db "$DB"
```

Crosswalk via SNOMED CT into ICD-10:

```bash
cat ctv3-codes.txt | sct map --from ctv3 --to icd10 --db "$DB"
```

Map SNOMED CT concepts to ICD-10:

```bash
printf '22298006\n73211009\n' \
  | sct map --from snomed --to icd10 --db "$DB"
```

Emit JSON lines for a downstream script:

```bash
cat ctv3-codes.txt \
  | sct map --from ctv3 --to icd10 -f json --db "$DB"
```

```bash
cat read2-code-term-keys.txt \
  | sct map --from read2 --to snomed --forward-history --db "$DB"
```

The recommended Read v2 input key is the seven-character ReadCode+TermCode
form, e.g. `0111.00`. See [Read v2 via TRUD item 9](read-v2-item9.md).

---

## Forward inactive SNOMED CT concepts

Old clinical records often contain concepts that are inactive in the current
release. Build with `--refsets all --include-inactive`, then add
`--forward-history`:

```bash
cat old-sctids.txt \
  | sct map --from snomed --to snomed --forward-history --db "$DB"
```

The forwarding data comes from the RF2 Association refsets, so it is sourced from
the current SNOMED CT release rather than from DMWB's Access tables.

---

## Export a mapped code list

For a DMWB-style cluster translation, keep the canonical list in SNOMED CT and
append target terminology columns at export time:

```bash
sct codelist export codelists/mi.codelist \
  --format csv \
  --include-maps ctv3,icd10,opcs4 \
  --db "$DB" \
  --output mi-crosswalk.csv
```

This leaves the reviewed codelist stable and repeatable, while the exported CSV
can carry the downstream migration or reporting codes needed for a particular
job.

---

## Serve the Excel add-in path

DMWB's Excel add-in can target a FHIR terminology server. `sct serve` provides
FHIR R4 `ConceptMap/$translate` over the same local SQLite database:

```bash
sct serve --db "$DB" --host 127.0.0.1 --port 8080
```

Then call `$translate` directly:

```bash
curl 'http://localhost:8080/ConceptMap/$translate?system=http://snomed.info/sct&code=22298006&targetsystem=http://hl7.org/fhir/sid/icd-10'
```

Bare system names are accepted too:

```bash
curl 'http://localhost:8080/ConceptMap/$translate?system=ctv3&code=X200E&targetsystem=icd10'
```

See [`sct serve`](../commands/serve.md) for the full FHIR surface.

---

## Inspect a DMWB Access file

The optional [`sct dmwb`](../commands/dmwb.md) command can inspect DMWB `.mdb`
files without Microsoft Access or `mdbtools`:

```bash
sct dmwb tables "DMWB NHS Data Migration Maps.mdb"
sct dmwb dump "DMWB NHS Data Migration Maps.mdb" SCTICDMAP --limit 5
```

This is useful for validation and reverse engineering, but it is not the
recommended production ingestion path for Read v2 today. Prefer `sct read2
import` using TRUD item 9's flat files, and the RF2-native maps loaded by
`--refsets all` for ICD-10, OPCS-4, CTV3, and history.

---

## Status matrix

| DMWB need | `sct` route | Status |
|---|---|---|
| Browse equivalent codes around a concept | `sct map` | Shipped |
| Batch terminology migration | `sct map --to` | Shipped |
| SNOMED CT -> ICD-10 / OPCS-4 maps | `sct ndjson --refsets all` + `sct sqlite` | Shipped |
| CTV3 <-> SNOMED CT maps | UK RF2 SimpleMap -> `crossmaps` | Shipped |
| Inactive concept forwarding | RF2 Association refsets + `--forward-history` | Shipped |
| Cross-terminology codelist exports | `sct codelist export --include-maps` | Shipped |
| FHIR translation for Excel / integration | `sct serve` `ConceptMap/$translate` | Shipped |
| DMWB Read v2 map import | `sct read2 import` over TRUD item 9 flat files | Shipped |
| DMWB EPR DATA / casemix analytics | Out of scope | Not planned |

For the design history and remaining Read v2 options, see the
[cross-terminology mapping spec](https://github.com/pacharanero/sct/blob/main/spec/cross-terminology-mapping.md).
