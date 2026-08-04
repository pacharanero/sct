# sct trud

Download SNOMED CT RF2 releases directly from [NHS TRUD](https://isd.digital.nhs.uk/trud)
using the TRUD REST API. Handles authentication, SHA-256 integrity verification, and optional pipeline chaining so a single command can take you from API key to a fully-built SQLite database.

!!! note "4096-character description limit (SNOMED International, July 2026)"
    SNOMED International raised the maximum FSN/Synonym length from 255 to 4096 characters, driven by long ingredient lists on multivalent vaccine FSNs. `sct` never assumed a fixed length anywhere in the pipeline - RF2 parsing, NDJSON, SQLite, Parquet, Arrow, and the FST index all use variable-length string types throughout - so no code change was needed. Confirmed against the July 2026 UK Monolith release with no issues.

---

## Getting a TRUD API key

### Create a TRUD account and subscribe

1. Register at [isd.digital.nhs.uk/trud](https://isd.digital.nhs.uk/trud/users/guest/filters/0/account/form)
2. Once logged in, subscribe to the editions you need:
   - **UK Monolith** (item 1799) - recommended for most users; includes International + UK Clinical + UK Drug (dm+d) + UK Pathology in one merged zip. Snapshot only.
   - **UK Clinical Edition** (item 101) - International + UK Clinical, without dm+d.
   - **UK Drug Extension** (item 105) - dm+d prescribing/medicines concepts only.
   - **NHS Data Migration** (item 9) - final April 2020 Read v2 / CTV3 / SNOMED CT migration maps. This is not RF2; import it with [`sct read2 import`](read2.md), or use `sct trud download --multi-terminology`.

### Get your API key

Your API key is shown on your [TRUD account page](https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/account/manage)
once you are signed in. It is unique to your account and derived from your email address and
password - if either changes, a new key is generated and the old one is disabled immediately.

**Keep your API key private.** Anyone who has it can download releases as if they were you.

---

## Supplying the API key

`sct trud` accepts the key via four methods, checked in this order (first non-empty value wins):

| Priority | Method                                   | Notes                                                                              |
| -------- | ---------------------------------------- | ---------------------------------------------------------------------------------- |
| 1        | `--api-key <KEY>`                        | Plain string on the command line. Avoid: visible in `ps` output and shell history; `sct` warns whenever this flag is used. |
| 2        | `--api-key-file <PATH>`                  | Path to a file whose **first line** is the key. Trailing whitespace is stripped.   |
| 3        | `$TRUD_API_KEY` environment variable     | **Recommended** for regular use, cron jobs, and CI/CD.                             |
| 4        | `api_key` in `~/.config/sct/config.toml` | Convenient for interactive use on a personal machine. Write it with `sct trud auth`. |

!!! warning "Avoid `--api-key`"
    A key passed directly on the command line is exposed in process listings and shell history, so `sct` prints a warning to stderr whenever `--api-key` is used. Prefer storing it once with `sct trud auth`, reading it from `--api-key-file`, or supplying it through `TRUD_API_KEY`.

### Storing the key with `sct trud auth` (easiest)

`sct trud auth` does the whole setup in one step: it creates `~/.config/sct/` if needed, writes
`config.toml` with owner-only permissions (`0600`), and sets `api_key` in the `[trud]` section.

```bash
sct trud auth < my-trud-key.txt     # from a file
pass show nhs/trud | sct trud auth  # from a password manager
sct trud auth your-key-here         # as an argument (lands in shell history)
```

The key is checked against TRUD before it is stored, so a typo is caught immediately rather
than on your first download:

```console
$ sct trud auth < my-trud-key.txt
sct trud auth: key verified against TRUD (uk_monolith subscribed)
sct trud auth: stored key ********3456
sct trud auth: wrote /home/you/.config/sct/config.toml
sct trud auth: next step - sct trud list
```

A key TRUD rejects is not written at all. If TRUD is unreachable the key is stored with a
warning so you can set up offline; `--no-verify` skips the check entirely, and `--dry-run`
prints the resulting config without writing it.

If you already have a `config.toml`, only the `api_key` line changes - your comments, other
sections, and formatting are left exactly as they were.

!!! warning "`$TRUD_API_KEY` wins"
    The environment variable has higher priority than the config file, so an exported
    `TRUD_API_KEY` shadows the key you just stored. `sct trud auth` warns you when it spots
    this; `unset TRUD_API_KEY` to use the stored key.

### Using an environment variable (recommended for CI)

```bash
export TRUD_API_KEY=your-key-here
sct trud list
```

Or for a single command without polluting the environment:

```bash
TRUD_API_KEY=your-key-here sct trud download --edition uk_monolith
```

### Using a key file

A key file keeps the key separate from the rest of your `sct` config. The trade-off is that you
have to pass `--api-key-file <path>` to every command.

The conventional location is `~/.config/sct/trud-api-key`. The file must contain only the key on
the first line (trailing whitespace is stripped). Set permissions to `600` and **never commit
this file to version control**.

```bash
mkdir -p ~/.config/sct
echo "your-key-here" > ~/.config/sct/trud-api-key
chmod 600 ~/.config/sct/trud-api-key
sct trud list --api-key-file ~/.config/sct/trud-api-key
```

Or hand the file to `sct trud auth` once, and drop the flag entirely:

```bash
sct trud auth --api-key-file ~/.config/sct/trud-api-key
```

### Editing the config file by hand

`sct trud auth` is the easy path, but the file is plain TOML and you can equally write it
yourself. Create `~/.config/sct/config.toml`:

```toml
[trud]
api_key = "your-key-here"
download_dir = "~/.local/share/sct/releases"   # optional; this is the default
```

Set permissions to `600` if you do: `chmod 600 ~/.config/sct/config.toml`. See the
[Config file](#config-file) section for the full list of options.

---

## Connectivity pre-flight check

Every `sct trud` command automatically verifies that the TRUD service is reachable before
making any authenticated request. If the check fails you will see:

```
Cannot reach NHS TRUD (https://isd.digital.nhs.uk/…).

The service may be offline or undergoing scheduled maintenance. TRUD advises
running automation on weekdays 08:00-18:00 or 00:00-06:00 UK time to avoid
planned maintenance.

Original error: …
```

This check is connection-level only - any HTTP response from TRUD (even an error page) counts
as "reachable". Only DNS failures, TCP timeouts, or TLS errors trigger this message.

> **Tip:** If `sct trud` fails during a scheduled automation run, check the time against the
> [recommended automation windows](#automating-updates) before investigating further.

---

## Subcommands

### `sct trud auth`

Stores your API key in the config file - the one-time setup step described in
[Storing the key with `sct trud auth`](#storing-the-key-with-sct-trud-auth-easiest) above.

```
sct trud auth [KEY] [--api-key-file <PATH>] [--config <PATH>] [--no-verify] [--dry-run]
```

| Flag                    | Description                                                                 |
| ----------------------- | --------------------------------------------------------------------------- |
| `KEY`                   | The key. Omit it, or pass `-`, to read from stdin instead                    |
| `--api-key-file <PATH>` | Read the key from the first line of this file                               |
| `--config <PATH>`       | Config file to write. Default: `$SCT_CONFIG`, else `$SCT_CONFIG_HOME/config.toml` |
| `--no-verify`           | Store the key without checking it against TRUD first                        |
| `--dry-run`             | Print the resulting config to stdout without writing anything               |

A project-local `./sct.toml` is deliberately *not* written unless you name it with `--config`,
since such files are usually version-controlled.

### `sct trud list`

Lists TRUD subscription status, or available releases for one subscribed item.

```
sct trud list [--edition <NAME>] [--item <N>]
```

With no `--edition` or `--item`, `sct trud list` probes the built-in editions and
shows whether the current API key is subscribed to each one. This is useful when
setting up a new TRUD account because unsubscribed items return HTTP 404 from the
TRUD API.

```bash
sct trud list
```

Example summary:

```
Edition           Item  Status          Latest release                                        Released
----------------------------------------------------------------------------------------------------
uk_monolith       1799  not subscribed  -                                                     -
uk_clinical        101  subscribed      uk_sct2cl_41.6.0_20260311000001Z.zip                  2026-03-18
uk_drug            105  subscribed      uk_sct2dr_41.6.0_20260311000001Z.zip                  2026-03-18
```

To list all available releases for one edition, pass `--edition` or `--item`.
Results are shown newest first.

```bash
sct trud list --edition uk_monolith
sct trud list --edition uk_clinical
sct trud list --edition uk_drug
sct trud list --edition nhs_data_migration
sct trud list --item 1799              # raw TRUD item number
```

Example output:

```
File                                          Released     Size      SHA-256 (first 12 chars)
uk_sct2mo_41.6.0_20260311000001Z.zip          2026-03-18   1.8 GB    285354105EA8
uk_sct2mo_41.5.0_20260211000001Z.zip          2026-02-18   1.8 GB    91c2a4830f1b
uk_sct2mo_41.4.0_20260114000001Z.zip          2026-01-14   1.8 GB    c3d7a2940e1c
```

---

### `sct trud check`

Checks if there is a newer release.

```
sct trud check [--edition <NAME>] [--item <N>]
```

Compares the latest available release against what is already in your download directory.
If the local file is present, its SHA-256 checksum is re-computed and compared against the
TRUD metadata, so a corrupt or half-downloaded local file is not mistaken for an up-to-date
copy.

```bash
sct trud check                         # UK Monolith (default)
sct trud check --edition uk_drug
```

Possible output:

```
Up to date: uk_sct2mo_41.6.0_20260311000001Z.zip (2026-03-18)
SHA-256 verified: A1B2C3D4…
```

```
New release available: uk_sct2mo_41.6.0_20260311000001Z.zip (2026-03-18)
```

```
Local file present but SHA-256 does not match TRUD metadata - re-download recommended: uk_sct2mo_41.6.0_20260311000001Z.zip
Expected: A1B2C3D4…
Got:      9F8E7D6C…
```

**Exit codes:**

| Code | Meaning                                                                              |
| ---- | ------------------------------------------------------------------------------------ |
| `0`  | Already up to date **and** SHA-256 verified                                          |
| `2`  | New release available, **or** local file fails the checksum and needs re-downloading |
| `1`  | Error (network, bad key, etc.)                                                       |

The `2` exit code (not `1`) is deliberate - it lets shell scripts distinguish "update
available" from an error without using `set -e` workarounds:

```bash
sct trud check --edition uk_monolith
if [ $? -eq 2 ]; then
    sct trud download --edition uk_monolith --pipeline
fi
```

---

### `sct trud download`

Downloads a release

```
sct trud download [--edition <NAME>] [--item <N>]
                  [--release <VERSION>]
                  [--output-dir <PATH>] [--data-dir <PATH>]
                  [--skip-if-current]
                  [--pipeline] [--pipeline-full] [--multi-terminology] [--with-read2]
                  [--include-inactive] [--refsets <MODE>] [--locale <LOCALE>]
```

Downloads the release zip to `~/.local/share/sct/releases/` (or `download_dir` from config,
or `--output-dir`). The SHA-256 checksum is verified before the file is committed - if it does
not match, the partial download is deleted and an error is returned.

Built artefacts produced by `--pipeline` (NDJSON, SQLite, Parquet, Arrow) are written to
`~/.local/share/sct/data/` (or `data_dir` from config, or `--data-dir`),
keeping source zips and built files in separate directories.

#### Download the latest UK Monolith

```bash
sct trud download
# Saves: ~/.local/share/sct/releases/uk_sct2mo_41.6.0_20260311000001Z.zip
```

#### Download and immediately build the full pipeline

```bash
sct trud download --pipeline
```

This runs `sct ndjson` then `sct sqlite` automatically after the download completes:

```
Downloading uk_sct2mo_41.6.0_20260311000001Z.zip (1.8 GB) ...
  [########################################] 1.8 GB/1.8 GB (2m 14s)
✓ Saved: ~/.local/share/sct/releases/uk_sct2mo_41.6.0_20260311000001Z.zip

→ Running: sct ndjson
→ Running: sct sqlite
✓ Pipeline complete.
  NDJSON: ~/.local/share/sct/data/uk_sct2mo_41.6.0_20260311000001Z.ndjson
  SQLite: ~/.local/share/sct/data/uk_sct2mo_41.6.0_20260311000001Z.db
```

#### Download, build, and also compute the transitive closure table + embeddings

```bash
sct trud download --pipeline-full
```

Runs `sct ndjson` → `sct sqlite` → `sct tct` → `sct embed`. The embed step requires
[Ollama](https://ollama.com) to be running locally (`ollama serve`); if it is not reachable,
that step is skipped with a warning and the rest of the pipeline still completes.

#### Download and build with inactive concepts and all crossmaps

```bash
sct trud download --pipeline --include-inactive --refsets all
```

`--include-inactive`, `--refsets` and `--locale` shape the `sct ndjson` step of the pipeline, exactly as if you had run [`sct ndjson`](ndjson.md) by hand - they have no effect without `--pipeline` / `--pipeline-full`. Here, inactive concepts are retained (each keeps its FSN, preferred term and synonyms) and the ICD-10 / OPCS-4 crossmaps plus concept-history sidecar are built. This downloads the latest release and goes straight through to a SQLite database in one command.

#### Build a complete multi-terminology workspace

```bash
sct trud download --multi-terminology
```

This is the shortest path from a fresh `cargo install sct-rs` to a local
multi-terminology workspace. It implies:

- `--pipeline`
- `--include-inactive`
- `--refsets all`
- `--with-read2`

The command downloads the UK Monolith, builds NDJSON and SQLite, downloads TRUD
item 9, and imports the final Read v2 maps into the same database. The resulting
SQLite database supports SNOMED CT, CTV3, Read v2, ICD-10, OPCS-4, and inactive
concept forwarding.

#### Skip the download if already current (safe for cron)

```bash
sct trud download --skip-if-current --pipeline
```

Checks whether the latest release zip is already present and its SHA-256 matches. If so,
does nothing (exit 0). Useful in scheduled jobs where you want the pipeline to run on the
first invocation after a release but be a no-op on all others.

#### Download a specific older release

```bash
sct trud download --release 41.5.0
```

#### Download the final NHS Data Migration maps

TRUD item 9 is the final April 2020 NHS Data Migration Pack. It contains flat
Read v2 / CTV3 / SNOMED CT mapping tables, including the Read v2 maps missing
from current UK RF2 releases.

```bash
sct trud download --edition nhs_data_migration
```

Do not pass `--pipeline`: this archive is not an RF2 release. Import it with
[`sct read2 import`](read2.md), or use `sct trud download --multi-terminology`
to download the UK Monolith and item 9 in one workflow.

#### Save to a specific directory

```bash
sct trud download --output-dir /data/snomed/
```

---

## Options reference

### Common flags (all subcommands)

| Flag                    | Description                                                      |
| ----------------------- | ---------------------------------------------------------------- |
| `--edition <NAME>`      | Named edition: `uk_monolith` (default), `uk_clinical`, `uk_drug`, `nhs_data_migration` |
| `--item <N>`            | Raw TRUD item number - overrides `--edition`                     |
| `--api-key <KEY>`       | API key as a plain string                                        |
| `--api-key-file <PATH>` | File whose first line is the API key                             |

### `sct trud download` flags

| Flag                  | Default                   | Description                                                                                 |
| --------------------- | ------------------------- | ------------------------------------------------------------------------------------------- |
| `--release <VERSION>` | -                         | Download a specific version (e.g. `41.5.0`); omit for the latest release                    |
| `--output-dir <PATH>` | `$SCT_DATA_HOME/releases` | Where to save the downloaded zip                                                            |
| `--data-dir <PATH>`   | `$SCT_DATA_HOME/data`     | Where to write built artefacts (NDJSON, SQLite, …)                                          |
| `--skip-if-current`   | off                       | Do nothing if the latest zip is already cached with a matching checksum                     |
| `--pipeline`          | off                       | Auto-run `sct ndjson` + `sct sqlite` after download                                         |
| `--pipeline-full`     | off                       | As `--pipeline`, plus `sct tct` + `sct embed`                                               |
| `--multi-terminology` | off                       | Implies pipeline + inactive concepts + all RF2 maps/history + item 9 Read v2 import         |
| `--with-read2`        | off                       | Pipeline only: download item 9 and import final Read v2 maps into the generated database    |
| `--include-inactive`  | off                       | Pipeline only: include inactive concepts in the `sct ndjson` step                           |
| `--refsets <MODE>`    | `simple`                  | Pipeline only: refsets to load - `none`, `simple`, or `all` (adds ICD-10/OPCS-4 + history)  |
| `--locale <LOCALE>`   | `en-GB`                   | Pipeline only: BCP-47 locale for preferred-term selection                                   |

---

## Directory layout

`sct trud` uses a single base directory, defaulting to `~/.local/share/sct/`, with two
subdirectories:

```
~/.local/share/sct/
├── releases/   ← downloaded RF2 zips
└── data/       ← built artefacts (.ndjson, .db, .parquet, .arrow)
```

Configuration and credentials live in `~/.config/sct/`:

```
~/.config/sct/
├── config.toml     ← optional config (editions, download_dir, data_dir, …)
└── trud-api-key    ← API key file (plain text, first line only; chmod 600)
```

> **Never commit `~/.config/sct/trud-api-key` or any file containing your TRUD API key**
> to version control. If your key is exposed, regenerate it immediately on your
> [TRUD account page](https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/account/manage).

Set `$SCT_DATA_HOME` to override the base:

```bash
export SCT_DATA_HOME=/mnt/snomed-store
# zips  → /mnt/snomed-store/releases/
# built → /mnt/snomed-store/data/
```

Individual directories can be overridden finer-grained via `--output-dir` / `--data-dir`
flags or `download_dir` / `data_dir` in `~/.config/sct/config.toml`.

---

## Config file

`~/.config/sct/config.toml` - `sct trud` reads but never writes this file.

```toml
[trud]
api_key      = "your-key-here"         # omit if using $TRUD_API_KEY
download_dir = "~/.local/share/sct/releases"   # optional; overrides $SCT_DATA_HOME/releases
data_dir     = "~/.local/share/sct/data"       # optional; overrides $SCT_DATA_HOME/data
default_edition = "uk_monolith"

# Override a built-in edition or add a custom one:
[trud.editions.uk_monolith]
trud_item = 1799

[trud.editions.my_org_special]
trud_item = 42
```

---

## Automating updates

UK SNOMED releases are published roughly monthly, typically on a Wednesday. TRUD's
[API guide](https://isd.digital.nhs.uk/trud/users/guest/filters/0/api) says to "run automation
scripts on weekdays between 8am and 6pm, or midnight and 6am (UK time) to avoid planned
maintenance". No downtime schedule is published, so treat those as the recommended windows
rather than a guarantee about the rest of the day.

### macOS - launchd (runs weekly, Wednesday 09:00)

Save as `~/Library/LaunchAgents/uk.nhs.sct.trud-sync.plist`, then load it:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>              <string>uk.nhs.sct.trud-sync</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/sct</string>
        <string>trud</string><string>download</string>
        <string>--edition</string><string>uk_monolith</string>
        <string>--skip-if-current</string>
        <string>--pipeline</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>TRUD_API_KEY</key><string>your-key-here</string>
    </dict>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Weekday</key><integer>3</integer>
        <key>Hour</key><integer>9</integer>
        <key>Minute</key><integer>0</integer>
    </dict>
    <key>StandardOutPath</key>  <string>/tmp/sct-trud-sync.log</string>
    <key>StandardErrorPath</key><string>/tmp/sct-trud-sync.log</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/uk.nhs.sct.trud-sync.plist
```

### Linux - systemd user timer

`~/.config/systemd/user/sct-trud.service`:

```ini
[Unit]
Description=sct SNOMED TRUD sync

[Service]
Type=oneshot
ExecStart=/usr/local/bin/sct trud download --edition uk_monolith --skip-if-current --pipeline
EnvironmentFile=%h/.config/sct/env
```

`~/.config/systemd/user/sct-trud.timer`:

```ini
[Unit]
Description=Weekly SNOMED TRUD sync - Wednesday 09:00

[Timer]
OnCalendar=Wed *-*-* 09:00:00
Persistent=true

[Install]
WantedBy=timers.target
```

```bash
# ~/.config/sct/env - permissions should be 600
TRUD_API_KEY=your-key-here
```

```bash
systemctl --user enable --now sct-trud.timer
```

### Crontab

```cron
# Weekly Wednesday 09:00 - check and rebuild if a new SNOMED release is available
0 9 * * 3  TRUD_API_KEY=your-key sct trud download --edition uk_monolith --skip-if-current --pipeline >> ~/.local/share/sct/trud-sync.log 2>&1
```

### GitHub Actions

```yaml
name: Update SNOMED release
on:
  schedule:
    - cron: '0 9 * * 3'   # weekly, Wednesday 09:00 UTC
  workflow_dispatch:        # allow manual trigger

jobs:
  trud-sync:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install sct
        run: cargo install sct-rs

      - name: Check for new release
        id: check
        run: |
          sct trud check --edition uk_monolith
          echo "status=$?" >> $GITHUB_OUTPUT
        env:
          TRUD_API_KEY: ${{ secrets.TRUD_API_KEY }}
        continue-on-error: true   # exit 2 must not fail the step

      - name: Download and build
        if: steps.check.outputs.status == '2'
        run: sct trud download --edition uk_monolith --pipeline
        env:
          TRUD_API_KEY: ${{ secrets.TRUD_API_KEY }}
```

---

## Troubleshooting

### "Cannot reach NHS TRUD"

The TRUD service is not reachable. Check:

1. **Planned maintenance** - TRUD does not publish a downtime schedule, but
   [advises](https://isd.digital.nhs.uk/trud/users/guest/filters/0/api) running automation on
   weekdays between 08:00–18:00 or midnight–06:00 UK time to avoid it. Outside those hours, try
   again before investigating.
2. **Network connectivity** - can you load [isd.digital.nhs.uk](https://isd.digital.nhs.uk) in a browser?
3. **Firewall / proxy** - some corporate networks block direct HTTPS to NHS services.
4. **Android / Termux** - if the error ends `failed to lookup address information: Try again`
   while `ping` and `curl` work, this is the known DNS limitation of the static `linux-aarch64`
   build on Android. See [Android (Termux)](../android-termux.md).

### "Invalid API key (HTTP 400)"

Your API key is wrong or has been regenerated. Get the current key from your [TRUD account page](https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/account/manage).
Regeneration happens automatically when you change your TRUD email address or password.

### "No releases found … not subscribed (HTTP 404)"

Your account exists but is not subscribed to this item. Log in to TRUD and subscribe to the
item (1799 for Monolith, 101 for Clinical Edition, 105 for Drug Extension).

### "SHA-256 checksum mismatch"

The downloaded file is corrupt. The partial file has been deleted automatically. Re-run the
command - this is almost always a transient network issue.

### `--pipeline` fails at the `sct ndjson` step

Check that the downloaded zip is a valid SNOMED RF2 Snapshot archive. The UK Monolith
(item 1799) ships Snapshot only - if you downloaded a different item that ships Full or Delta
files, `sct ndjson` will need the Snapshot sub-directory pointed to explicitly.
