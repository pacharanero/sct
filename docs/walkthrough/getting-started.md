# Getting Started

Install `sct`, download a SNOMED CT release, and build the core artefacts.

---

## Installation

Prebuilt binaries are published for **Linux** (x86_64, aarch64), **macOS** (Apple Silicon, Intel), and **Windows** (x86_64) on every release, each with a SHA-256 checksum you can verify against the `SHA256SUMS` file on the [Releases page](https://github.com/pacharanero/sct/releases). Pick your platform:

=== ":material-apple: macOS"

    **Homebrew** (recommended)

    ```bash
    brew tap pacharanero/tap
    brew install sct
    ```

    **Shell installer** - auto-detects your chip, verifies the checksum, installs to `~/.local/bin`:

    ```bash
    curl -fsSL https://raw.githubusercontent.com/pacharanero/sct/main/install.sh | sh
    ```

    **Disk image** - download the `.dmg` for your Mac, open it, and drag `sct` onto a folder on your `PATH`:

    [:material-download: Apple Silicon (.dmg)](https://github.com/pacharanero/sct/releases/latest/download/sct-macos-aarch64.dmg){ .md-button }
    [:material-download: Intel (.dmg)](https://github.com/pacharanero/sct/releases/latest/download/sct-macos-x86_64.dmg){ .md-button }

    !!! warning "Unsigned for now"
        The `.dmg` is not yet notarized. On first run, **right-click `sct` → Open**, or clear the quarantine flag: `xattr -d com.apple.quarantine ./sct`.

=== ":material-linux: Linux"

    **Debian / Ubuntu** (`.deb`)

    ```bash
    curl -fLO https://github.com/pacharanero/sct/releases/latest/download/sct-linux-x86_64.deb
    sudo apt install ./sct-linux-x86_64.deb        # aarch64: sct-linux-aarch64.deb
    ```

    **Fedora / RHEL / openSUSE** (`.rpm`)

    ```bash
    curl -fLO https://github.com/pacharanero/sct/releases/latest/download/sct-linux-x86_64.rpm
    sudo dnf install ./sct-linux-x86_64.rpm        # aarch64: sct-linux-aarch64.rpm
    ```

    **Homebrew**

    ```bash
    brew tap pacharanero/tap
    brew install sct
    ```

    **Shell installer** - auto-detects your architecture, verifies the checksum, installs to `~/.local/bin`:

    ```bash
    curl -fsSL https://raw.githubusercontent.com/pacharanero/sct/main/install.sh | sh
    ```

=== ":material-microsoft-windows: Windows"

    **Scoop** (recommended)

    ```powershell
    scoop bucket add pacharanero https://github.com/pacharanero/scoop
    scoop install sct
    ```

    **PowerShell installer** - downloads, verifies the checksum, installs to `%LOCALAPPDATA%\sct\bin`:

    ```powershell
    iwr -useb https://raw.githubusercontent.com/pacharanero/sct/main/install.ps1 | iex
    ```

    **Standalone executable** - download [`sct-windows-x86_64.exe`](https://github.com/pacharanero/sct/releases/latest/download/sct-windows-x86_64.exe), drop it in a folder on your `PATH`, and run `sct` from a terminal.

    !!! warning "Unsigned for now"
        The `.exe` is not yet Authenticode-signed. SmartScreen may warn on first run - choose **More info → Run anyway**.

=== ":material-language-rust: Cargo (any OS)"

    With a [Rust toolchain](https://rustup.rs) (stable 1.70+):

    ```bash
    cargo install sct-rs          # compile from crates.io
    ```

    Or grab a prebuilt binary without compiling, via [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall):

    ```bash
    cargo binstall sct-rs
    ```

    Build from a clone. `sct serve` is included by default; add optional extras (`tui`, `gui`, `dmwb`, or `full`) as needed:

    ```bash
    git clone https://github.com/pacharanero/sct && cd sct
    cargo install --path . --features full
    ```

Verify the install:

```bash
sct --version
```

> Optionally, generate [shell completions](../commands/completions.md) for your shell at this point.

---

## Get SNOMED RF2 Data

SNOMED CT is distributed as RF2 (Release Format 2) - a set of TSV files.

### Option A - Automated download with `sct trud` (recommended for UK users)

`sct trud` authenticates with [NHS TRUD](https://isd.digital.nhs.uk/trud), downloads the
correct release zip, verifies its SHA-256 checksum, and can optionally run the full build
pipeline in one command.

**Full details:** [`sct trud`](../commands/trud.md)

#### 1. Get your TRUD API key

1. Register or sign in at [isd.digital.nhs.uk/trud](https://isd.digital.nhs.uk/trud/users/guest/filters/0/account/form)
2. Subscribe to the **UK Monolith** (item 1799) - includes International + UK Clinical + UK Drug (dm+d) + UK Pathology, pre-merged
3. Your API key is shown on your [TRUD account page](https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/account/manage)

The key is a plain string. It is tied to your account credentials: if you change your TRUD
email or password, the key is regenerated and the old one stops working immediately.

#### 2. Store the key safely

The conventional location is `~/.config/sct/trud-api-key` - a plain text file with the key
on the first line and no other content:

```bash
mkdir -p ~/.config/sct
echo "your-key-here" > ~/.config/sct/trud-api-key
chmod 600 ~/.config/sct/trud-api-key
```

> **Do not commit this file to version control.** If you accidentally expose the key,
> regenerate it immediately from your TRUD account page.

Alternatively, export it as an environment variable - useful for CI/CD and automation:

```bash
export TRUD_API_KEY=your-key-here
```

#### 3. Download and build

```bash
# Download the latest UK Monolith and immediately build the SQLite database
sct trud download --api-key-file ~/.config/sct/trud-api-key \
                  --edition uk_monolith \
                  --pipeline

# Zip saved to:  ~/.local/share/sct/releases/
# SQLite at:     ~/.local/share/sct/data/uk_sct2mo_…SNAPSHOT.db
```

or:

```bash
sct trud download --api-key ********** \
                  --edition uk_monolith \
                  --pipeline
```

These are all supported:

| Priority | Method |
| -- | -- |
| 1 | `--api-key <KEY>` |
| 2 | `--api-key-file <PATH>` |
| 3 | `$TRUD_API_KEY` env var |
| 4 | `config.toml` |

See [`sct trud`](../commands/trud.md) for the full options reference, config file format,
automation examples (launchd, systemd, cron, GitHub Actions), and troubleshooting.

---

### Option B - Manual download

If you prefer to download the zip yourself:

- **UK edition:** [NHS TRUD](https://isd.digital.nhs.uk/trud) - subscribe to item **1799** (UK Monolith, recommended)
  - Includes: International + UK Clinical + UK Drug (dm+d) + UK Pathology, pre-merged
  - Note: item 101 is the Clinical Edition (no dm+d); item 105 is the Drug Extension on its own
- **International edition:** [SNOMED MLDS](https://mlds.ihtsdotools.org/)
- **IPS Free Set:** Available without affiliate membership from SNOMED MLDS

`sct` accepts the zip directly or an already-extracted directory.

> **Confused by the NHS TRUD download options?** See [UK Edition structure](../uk-edition-structure.md)
> for a plain-English guide to the different release types, what's in each zip, and how to decode the filenames.

---

## Build the NDJSON Artefact

The first step is always `sct ndjson`. This joins the RF2 tables and produces the
canonical intermediate artefact that everything else is built from.

**Docs**: [`sct ndjson`](../commands/ndjson.md)

```bash
sct ndjson --rf2 ~/Downloads/uk_sct2mo_41.6.0_20260311000001Z.zip \
           --output snomed.ndjson

# ~52 s for 838k concepts → snomed.ndjson (1.3 GB)
```

If you pass it a `.zip` it will automatically extract and parse the RF2 files within. If you pass it a directory containing extracted RF2 files, it will parse them directly.

The output is a single `.ndjson` file - one JSON object per line, each representing a SNOMED concept with all its details (ID, preferred term, synonyms, hierarchy, relationships, attributes, etc.)

Testing on my laptop, this takes about 52 seconds for the UK Monolith release with 837,930 active concepts. The resulting NDJSON file is about 1.3 GB. Incredibly, because NDJSON is easier to handle in memory than JSON, **you can load the whole 1.3 GB file into VSCode** (takes less than 5 seconds) and play around with it there, great for getting to understand what data is available and how it's structured.

You can now query the NDJSON file with `jq` or any tool that can handle line-delimited JSON. For example, to get the full details of Myocardial infarction (disorder)":

```bash
jq 'select(.id == "22298006")' snomed.ndjson
```

Which should return something similar to the below:

```json
{
  "id": "22298006",
  "fsn": "Myocardial infarction (disorder)",
  "preferred_term": "Myocardial infarction",
  "synonyms": [
    "Infarction of heart",
    "Cardiac infarction",
    "Heart attack",
    "Myocardial infarct",
    "MI - myocardial infarction"
  ],
  "hierarchy": "Clinical finding",
  "hierarchy_path": [
    "SNOMED CT Concept",
    "Clinical finding",
    "Finding of trunk structure",
    "Finding of upper trunk",
    "Finding of thoracic region",
    "Disorder of thorax",
    "Disorder of mediastinum",
    "Heart disease",
    "Structural disorder of heart",
    "Myocardial lesion",
    "Myocardial necrosis",
    "Myocardial infarction"
  ],
  "parents": [
    {
      "id": "251061000",
      "fsn": "Myocardial necrosis (disorder)"
    },
    {
      "id": "414545008",
      "fsn": "Ischemic heart disease (disorder)"
    }
  ],
  "children_count": 14,
  "active": true,
  "module": "900000000000207008",
  "effective_time": "20020131",
  "attributes": {
    "associated_morphology": [
      {
        "id": "55641003",
        "fsn": "Infarct (morphologic abnormality)"
      }
    ],
    "finding_site": [
      {
        "id": "74281007",
        "fsn": "Myocardium structure (body structure)"
      }
    ]
  },
  "ctv3_codes": [
    "X200E"
  ],
  "read2_codes": [],
  "refsets": [
    "1127601000000107",
    "1382531000000102"
  ],
  "relationships": [
    {
      "type_id": "116676008",
      "destination_id": "55641003",
      "group": 1
    },
    {
      "type_id": "363698007",
      "destination_id": "74281007",
      "group": 1
    }
  ],
  "crossmaps": [],
  "schema_version": 5
}
```

The NDJSON artefact is the stable interface. Version-controlled. Copyable. Diffable. (Remember though, it is still copyright SNOMED International and subject to the SNOMED CT licence terms, so **don't share it publicly**.)

Here are some examples of mini-queries you can run directly on the NDJSON file with `jq` or `grep`:

Get all Procedures:

```bash
grep '"hierarchy":"Procedure"' snomed.ndjson | wc -l
```

Get all concepts with "heart" in the preferred term:

```bash
jq 'select(.preferred_term | test("heart"; "i")) | {id, preferred_term}' snomed.ndjson
```

Get a concept via CTV3 code (UK edition only):

```bash
jq 'select(.ctv3_codes | index("X200E"))' snomed.ndjson
```

### Programmatic access

You can look inside the NDJSON file with any language that can read it eg. Python:

Prints all concepts that have CTV3 codes, with their preferred term and list of CTV3 codes:

```python
import json
with open('snomed.ndjson') as f:
    for line in f:
        rec = json.loads(line)
        if rec['ctv3_codes']:
            print(f"{rec['id']}\t{rec['preferred_term']}\t{rec['ctv3_codes']}")
```

NDJSON is great for quick exploration and ad-hoc queries, but for more complex querying and analytics, the next step is to load it into SQLite or export to Parquet.

---

## SQLite + Full-Text Search

Load the NDJSON artefact into a SQLite database with FTS5 full-text search.

```bash
sct sqlite --ndjson snomed.ndjson --output snomed.db
```

**Docs**: [`sct sqlite`](../commands/sqlite.md)

On my machine this takes about 32 seconds for the UK Monolith release with 837,930 active concepts. The resulting `snomed.db` file is about 1.9 GB.

**Now you can query SNOMED CT with standard `sqlite3`:** The following examples should all work out of the box on the resulting database, running in the terminal.

!!! note "Installing `sqlite3`"
    These examples use the `sqlite3` command-line tool. `sct` itself embeds SQLite and never needs it, but the `sqlite3` CLI isn't preinstalled on every platform: macOS ships with it; on Debian/Ubuntu run `sudo apt install sqlite3`, on Fedora `sudo dnf install sqlite`, and on Windows download the "command-line tools" bundle from [sqlite.org/download.html](https://www.sqlite.org/download.html).

LLMs are excellent at generating SQL queries, so you can also use any LLM to generate custom SQL queries for you on demand. `sct` includes an MCP server that exposes the database as 'tools' to LLMs in a standard way for interactive querying - see [Semantic search and LLMs](semantic-llm.md).

Free-text search (FTS5)

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts_fts WHERE concepts_fts MATCH 'heart attack' LIMIT 5"
```

Direct concept lookup

```bash
sqlite3 snomed.db \
  "SELECT preferred_term, json(attributes) FROM concepts WHERE id = '22298006'"
```

Browse by hierarchy

```bash
sqlite3 snomed.db \
  "SELECT id, preferred_term FROM concepts WHERE hierarchy = 'Procedure' LIMIT 10"
```

Recursive subsumption via IS-A table

```bash
sqlite3 snomed.db \
  "WITH RECURSIVE descendants(id) AS (
    SELECT DISTINCT child_id FROM concept_isa WHERE parent_id = '22298006'
    UNION
    SELECT ci.child_id FROM concept_isa ci JOIN descendants d ON ci.parent_id = d.id
  )
  SELECT DISTINCT c.preferred_term FROM concepts c JOIN descendants d ON c.id = d.id LIMIT 20"
```

For simple lexical searches, I added a `sct lexical` subcommand that generates SQL queries for you, so you don't have to write the raw SQL yourself. It supports free-text search, hierarchy filtering, and prefix search:

```bash
sct lexical --db snomed.db "heart attack" --limit 10
sct lexical --db snomed.db "diabetes" --hierarchy "Clinical finding"
sct lexical --db snomed.db "amox*"      # prefix search
```

> **Docs**: [`sct lexical`](../commands/lexical.md)

> For more advanced and interesting SQL queries, see the [`sct sqlite` documentation](../commands/sqlite.md)

---

## UK Crossmaps: CTV3

**UK edition only.** The CTV3 (Clinical Terms Version 3) crossmaps are available when building from a
UK NHS SNOMED CT release (UK Monolith or UK Clinical Edition from [NHS TRUD](https://isd.digital.nhs.uk/trud)).
They are parsed automatically from the `der2_sRefset_SimpleMap` reference set (refset ID `900000000000497000`).

CTV3 is the legacy NHS terminology used in GP and secondary care systems before SNOMED CT. Having
SNOMED → CTV3 mappings is useful for:

- **Migrating data** from legacy systems that recorded CTV3 codes
- **Interoperability** with older clinical records
- **Reporting** to systems that still consume CTV3
- **Learning and exploration** - see how concepts were mapped from CTV3 to SNOMED CT

Over 380,000 concepts have CTV3 mappings in the UK Monolith release. Read v2 codes are not distributed
as a separate refset in current UK releases.

**Data structure:**

The SQLite database includes:

- `concepts.ctv3_codes` - JSON array of CTV3 codes for each concept
- `crossmaps` table - general cross-terminology rows, including CTV3 → SNOMED
  SimpleMap rows
- `concept_maps` table - legacy reverse index retained for compatibility

**Example queries:**

Forward: SNOMED → CTV3 code

```bash
sqlite3 snomed.db "SELECT id, preferred_term, ctv3_codes FROM concepts WHERE id = '22298006'"

# 22298006|Myocardial infarction|["X200E"]
```

Reverse: CTV3 code → SNOMED concept

```bash
sqlite3 snomed.db "
  SELECT c.id, c.preferred_term, c.hierarchy
  FROM concepts c
  JOIN concept_maps m ON c.id = m.concept_id
  WHERE m.code = 'X200E' AND m.terminology = 'ctv3'"

# 22298006|Myocardial infarction|Clinical finding
```

---

## Next steps

With SQLite built, you can now:

- [Browse reference sets and build code lists](refsets-codelists.md) - explore what refsets are in the release and curate clinical code lists
- [Export to Parquet](parquet-duckdb.md) - analytics with DuckDB, pandas, Polars, or Spark
- [Semantic search and LLMs](semantic-llm.md) - Markdown export, vector embeddings, MCP server
- [Everything else](everything-else.md) - transitive closure table, interactive UIs, release diff, and more
