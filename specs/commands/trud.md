# `sct trud` — Automated SNOMED CT Release Downloads via NHS TRUD API

Authenticate with the [NHS TRUD](https://isd.digital.nhs.uk/trud) REST API to list, check,
and download SNOMED CT RF2 release files, with optional pipeline chaining to build the full
`sct` artefact stack immediately after download.

---

## Background

NHS TRUD (Technology Reference Update Distribution) is the distribution platform for UK
SNOMED CT releases. It provides a REST API that allows authenticated account holders to list
and download releases for items they are subscribed to.

### TRUD API

The API has two endpoints:

| Purpose | URL |
|---|---|
| List releases | `GET https://isd.digital.nhs.uk/trud/api/v1/keys/{api_key}/items/{item_id}/releases` |
| Latest release only | `GET …/releases?latest` |
| Download | `GET {archiveFileUrl}` (URL from the list response) |

The list response includes `archiveFileSha256` for integrity verification; `sct trud download`
always verifies this after download and deletes the file if the checksum does not match.

### Relevant TRUD item numbers

| Item | Edition | Release types | Contents |
|---|---|---|---|
| **1799** | UK Monolith | Snapshot only | International + UK Clinical + UK Drug (dm+d) + UK Pathology, fully merged and de-duplicated |
| **101** | UK Clinical Edition | Full, Snapshot & Delta | International + UK Clinical extension |
| **105** | UK Drug Extension (dm+d) | Full, Snapshot & Delta | Prescribing/medicines concepts only |

For most users the **Monolith (item 1799)** is the right default — it is a single zip containing
everything, with conflicts and duplicates already resolved by NHS England.

---

## API key

A TRUD API key is required. Each account has a unique key derived from the account's email
address and password; it is shown on the [Your account](https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/account/manage)
page. The key is invalidated if the email or password changes.

**Keep the key private.** Anyone who has it can download releases as if they were you.

### API key lookup order

`sct trud` resolves the API key using the following precedence (highest to lowest). The first
source that provides a non-empty value is used; the remaining sources are not consulted.

1. `--api-key <KEY>` CLI flag (plain string — avoid where possible; the key is visible in
   process listings and shell history)
2. `--api-key-file <PATH>` CLI flag — path to a file whose first line is the API key; the file
   may contain only the key and optional trailing whitespace
3. `$TRUD_API_KEY` environment variable — **preferred for CI/CD and cron jobs**
4. `api_key` field in the config file (`~/.config/sct/config.toml`)

If no key is found from any source, `sct trud` exits with a clear error message directing the
user to the TRUD account page.

---

## Configuration file

`~/.config/sct/config.toml` — created by the user; `sct trud` reads but never writes it.

```toml
[trud]
api_key = "your-trud-api-key-here"   # optional; prefer $TRUD_API_KEY in CI

# Directory where downloaded zip files are stored.
# Defaults to ~/.local/share/sct/releases if not set.
download_dir = "~/.local/share/sct/releases"

# Default edition to use when --edition is not supplied.
# Must match a key under [trud.editions].
default_edition = "uk_monolith"
```

### Edition profiles

Built-in editions are defined internally. Users may override them or add custom profiles in
the config file:

```toml
[trud.editions.uk_monolith]
trud_item = 1799
description = "UK Monolith (International + UK Clinical + UK Drug/dm+d + UK Pathology)"

[trud.editions.uk_clinical]
trud_item = 101
description = "UK Clinical Edition (International + UK Clinical, no dm+d)"

[trud.editions.uk_drug]
trud_item = 105
description = "UK Drug Extension (dm+d only)"
```

---

## Subcommands

### `sct trud list`

List available releases for an edition, from newest to oldest.

```
sct trud list [--edition <NAME>] [--item <N>]
              [--api-key <KEY>] [--api-key-file <PATH>]
```

| Flag | Default | Description |
|---|---|---|
| `--edition <NAME>` | `uk_monolith` | Named edition profile (see config). |
| `--item <N>` | — | Raw TRUD item number; overrides `--edition`. |
| `--api-key <KEY>` | — | API key as a plain string. |
| `--api-key-file <PATH>` | — | Path to a file containing the API key. |

**Output** (table to stdout):

```
Version                                    Released     Size      SHA-256 (first 12)
uk_sct2mo_41.6.0_20260311000001Z.zip       2026-03-18   1.8 GB    285354105EA8…
uk_sct2mo_41.5.0_20260211000001Z.zip       2026-02-18   1.8 GB    91c2a4830f1b…
```

---

### `sct trud check`

Check whether a newer release is available compared to what is already in `download_dir`.

```
sct trud check [--edition <NAME>] [--item <N>]
               [--api-key <KEY>] [--api-key-file <PATH>]
```

Hits `…/releases?latest` and compares the `releaseDate` in the response against the newest
matching zip already present in `download_dir`.

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | Already up to date |
| `2` | A newer release is available |
| `1` | Error (network failure, bad API key, etc.) |

Exit code `2` (not `1`) for "update available" means `sct trud check` can be used safely in
shell scripts without being confused with error conditions:

```bash
sct trud check --edition uk_monolith
if [ $? -eq 2 ]; then
    sct trud download --edition uk_monolith --pipeline
fi
```

**Output examples:**

```
Up to date: uk_sct2mo_41.6.0_20260311000001Z.zip (2026-03-18)
```

```
New release available: uk_sct2mo_41.6.0_20260311000001Z.zip (2026-03-18)
Currently have:        uk_sct2mo_41.5.0_20260211000001Z.zip (2026-02-18)
```

---

### `sct trud download`

Download a release zip to `download_dir`.

```
sct trud download [--edition <NAME>] [--item <N>]
                  [--latest | --release <VERSION>]
                  [--output-dir <PATH>]
                  [--api-key <KEY>] [--api-key-file <PATH>]
                  [--skip-if-current]
                  [--pipeline] [--pipeline-full]
```

| Flag | Default | Description |
|---|---|---|
| `--edition <NAME>` | `uk_monolith` | Named edition profile. |
| `--item <N>` | — | Raw TRUD item number; overrides `--edition`. |
| `--latest` | on | Download the most recent release. |
| `--release <VERSION>` | — | Download a specific named version (e.g. `41.5.0`). Mutually exclusive with `--latest`. |
| `--output-dir <PATH>` | `download_dir` from config | Directory to save the zip. |
| `--api-key <KEY>` | — | API key as a plain string. |
| `--api-key-file <PATH>` | — | Path to a file containing the API key. |
| `--skip-if-current` | off | Do nothing (exit 0) if the latest release is already present and its SHA-256 matches. |
| `--pipeline` | off | After a successful download, run `sct ndjson` then `sct sqlite` automatically. |
| `--pipeline-full` | off | As `--pipeline`, plus `sct tct` and `sct embed` (Ollama must be running for embed). |

#### Download behaviour

1. Call the TRUD list endpoint (`?latest` or unfiltered for `--release`).
2. If the target zip already exists in `output-dir` and its SHA-256 matches the TRUD response:
   - With `--skip-if-current`: exit 0 silently.
   - Without: re-use the existing file (no re-download) and continue to pipeline if requested.
3. Stream the zip to `output-dir` using a temporary filename; show an `indicatif` progress bar
   using the `Content-Length` response header.
4. On completion, verify SHA-256. If the checksum does not match: delete the partial file and
   exit with an error.
5. On success: rename from the temporary filename to the final filename.

#### `--pipeline` behaviour

After a successful download, `--pipeline` automatically invokes:

```
sct ndjson  --rf2 <downloaded.zip>  --output <output-dir>/<name>.ndjson
sct sqlite  --input <name>.ndjson   --output <output-dir>/<name>.db
```

`--pipeline-full` additionally runs:

```
sct tct    --db <name>.db
sct embed  --input <name>.ndjson  --output <output-dir>/<name>.arrow
```

The embed step is skipped with a warning if Ollama is not reachable, rather than failing the
whole pipeline.

**Example — full automated update:**

```bash
sct trud download --edition uk_monolith --skip-if-current --pipeline-full
```

---

## Automation

TRUD recommends running automation between **08:00–18:00** or **midnight–06:00 UK time** to
avoid planned maintenance windows. UK SNOMED releases are published roughly monthly on a
Wednesday.

### macOS — launchd

Create `~/Library/LaunchAgents/uk.nhs.sct.trud-sync.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>uk.nhs.sct.trud-sync</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/sct</string>
        <string>trud</string>
        <string>download</string>
        <string>--edition</string>
        <string>uk_monolith</string>
        <string>--skip-if-current</string>
        <string>--pipeline</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>TRUD_API_KEY</key>
        <string>your-key-here</string>
    </dict>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Weekday</key><integer>3</integer>
        <key>Hour</key><integer>9</integer>
        <key>Minute</key><integer>0</integer>
    </dict>
    <key>StandardOutPath</key>
    <string>/tmp/sct-trud-sync.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/sct-trud-sync.log</string>
</dict>
</plist>
```

Load with: `launchctl load ~/Library/LaunchAgents/uk.nhs.sct.trud-sync.plist`

### Linux — systemd user timer

`~/.config/systemd/user/sct-trud.service`:

```ini
[Unit]
Description=sct SNOMED TRUD sync

[Service]
Type=oneshot
ExecStart=/usr/local/bin/sct trud download --edition uk_monolith --skip-if-current --pipeline
EnvironmentFile=%h/.config/sct/env   # file containing TRUD_API_KEY=...
```

`~/.config/systemd/user/sct-trud.timer`:

```ini
[Unit]
Description=Weekly SNOMED TRUD sync (Wednesday 09:00)

[Timer]
OnCalendar=Wed *-*-* 09:00:00
Persistent=true

[Install]
WantedBy=timers.target
```

Enable with: `systemctl --user enable --now sct-trud.timer`

### Crontab

```cron
# Weekly Wednesday 09:00 — check and rebuild if a new SNOMED release is available
0 9 * * 3  TRUD_API_KEY=<key> sct trud download --edition uk_monolith --skip-if-current --pipeline >> ~/.local/share/sct/trud-sync.log 2>&1
```

### GitHub Actions

```yaml
name: Update SNOMED release
on:
  schedule:
    - cron: '0 9 * * 3'   # weekly, Wednesday 09:00 UTC
  workflow_dispatch:

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
          echo "exit=$?" >> $GITHUB_OUTPUT
        env:
          TRUD_API_KEY: ${{ secrets.TRUD_API_KEY }}
        continue-on-error: true

      - name: Download and build pipeline
        if: steps.check.outputs.exit == '2'
        run: sct trud download --edition uk_monolith --pipeline
        env:
          TRUD_API_KEY: ${{ secrets.TRUD_API_KEY }}
```

---

## Implementation notes

### Crate dependencies

| Crate | Already in `Cargo.toml`? | Purpose |
|---|---|---|
| `ureq` | Yes (`v3`, features = `["json"]`) | All HTTP calls |
| `indicatif` | Yes | Download progress bar |
| `sha2` | **No — add to `Cargo.toml`** | SHA-256 checksum verification |
| `toml` | **No — add to `Cargo.toml`** | Config file parsing |

For the download stream, use `response.into_reader()` from `ureq 3.x` to pipe the response
body to disk in chunks rather than loading the entire multi-GB zip into memory.

### API key security

- Never include the raw API key in log output or error messages. Truncate to the first 6
  characters (e.g. `deadc0…`) if it must appear in a diagnostic message.
- When reading from `--api-key-file`, trim all leading/trailing whitespace from the first line;
  do not read beyond the first line.
- Validate that the key is non-empty before making any network request.

### Error handling

| Condition | Behaviour |
|---|---|
| No API key found | Exit 1 with message directing user to TRUD account page and the four supply methods |
| HTTP 400 from TRUD | Exit 1: "Invalid API key. Check your TRUD account page." |
| HTTP 404 from TRUD | Exit 1: "No releases found — check your item number and TRUD subscription." |
| SHA-256 mismatch | Delete partial file, exit 1: "Checksum mismatch — download may be corrupt." |
| Disk full during download | Delete partial file, exit 1 with I/O error details |
| Ollama unreachable during `--pipeline-full` | Skip embed step with warning; do not fail overall |

### Relation to the library refactor

`--pipeline` invokes `ndjson::run()` and `sqlite::run()` directly as Rust function calls, not
as subprocess forks. This requires those functions to be callable as library functions —
consistent with the refactor described in [`specs/library-rs.md`](../library-rs.md). If the
library refactor has not yet landed, `--pipeline` can spawn `sct ndjson` / `sct sqlite` as
child processes as a temporary measure, with a `// TODO: replace with direct call after
library-rs refactor` comment.
