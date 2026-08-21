# sct

A fast, local-first SNOMED CT toolkit written in Rust. Convert a SNOMED CT RF2
release into queryable formats in seconds. Almost ridiculously fast on modern
hardware. Free and open source. No Java. No Elasticsearch. Docker optional.

[:octicons-arrow-right-24: New to SNOMED CT? Start here](primer.md) ·
[:octicons-arrow-right-24: Full walkthrough](walkthrough/index.md) ·
[:octicons-arrow-right-24: Get your own terminology server](deploy/index.md) ·
[:octicons-arrow-right-24: Why build this?](why/why-build-this.md) ·
[:octicons-arrow-right-24: Benchmarks](benchmarks.md)

---

## Install

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

    **Arch Linux ([AUR](https://aur.archlinux.org/packages/sct-rs-bin))**

    ```bash
    yay -S sct-rs-bin
    ```

    **Shell installer** - auto-detects your architecture, verifies the checksum, installs to `~/.local/bin`:

    ```bash
    curl -fsSL https://raw.githubusercontent.com/pacharanero/sct/main/install.sh | sh
    ```

=== ":material-microsoft-windows: Windows"

    **One-line installer** (recommended - the least fuss). Open **PowerShell** (Start menu → type "PowerShell" → Enter), paste this line, and press Enter:

    ```powershell
    iwr -useb https://raw.githubusercontent.com/pacharanero/sct/main/install.ps1 | iex
    ```

    It downloads `sct`, verifies its checksum, installs it to `%LOCALAPPDATA%\sct\bin`, and **offers to add that folder to your PATH** (just press Enter to accept) - no manual PATH editing. Open a new terminal afterwards and run `sct --version`.

    **Scoop** - if you already use the [Scoop](https://scoop.sh) package manager (handy for one-command updates later):

    ```powershell
    scoop bucket add pacharanero https://github.com/pacharanero/scoop
    scoop install sct
    ```

    **Manual `.exe`** (advanced) - download [`sct-windows-x86_64.exe`](https://github.com/pacharanero/sct/releases/latest/download/sct-windows-x86_64.exe), put it in a folder that is already on your `PATH`, and run `sct` from a terminal. The one-line installer above handles this PATH step for you.

    !!! warning "Unsigned for now"
        `sct` is not yet Authenticode-signed, so Windows SmartScreen may warn on first run - choose **More info → Run anyway**. (Code-signing and a `winget` package are planned.)

=== ":material-language-rust: Cargo (any OS)"

    With a [Rust toolchain](https://rustup.rs) (stable 1.88+):

    ```bash
    cargo install sct-rs          # compile from crates.io
    ```

    Or grab a prebuilt binary without compiling, via [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall):

    ```bash
    cargo binstall sct-rs
    ```

    Build from a clone. `sct serve` and `sct tui` are included by default; add optional extras (`gui`, `dmwb`, `diagram-svg`, or `full`) as needed:

    ```bash
    git clone https://github.com/pacharanero/sct && cd sct
    cargo install --path . --features full
    ```

Verify the install:

```bash
sct --version
```

Then turn a SNOMED CT RF2 release into queryable data in three commands:

```bash
sct ndjson  --rf2 ~/path-to-your-SNOMED-RF2.zip
sct sqlite  --ndjson snomed.ndjson
sct lexical "heart attack"
```

[:octicons-arrow-right-24: Full walkthrough](walkthrough/index.md)

---

<div class="grid cards" markdown>

-   :material-pipe:{ .lg .middle } __Build the pipeline__

    ---

    Convert an RF2 snapshot into **SQLite**, **Parquet**, **Markdown**, or
    **Arrow embeddings** in a single command. 837,930 concepts in under a
    minute on a laptop.

    [:octicons-arrow-right-24: Walkthrough](walkthrough/index.md)

-   :material-database-search:{ .lg .middle } __Search__

    ---

    **Full-text search** via FTS5 for keywords and phrases. **Typo-tolerant**
    fuzzy and prefix search via a mmap'd **FST index**. **Semantic vector
    search** via local Ollama embeddings. All offline.

    [:octicons-arrow-right-24: sct lexical](commands/lexical.md)
    · [:octicons-arrow-right-24: sct fst](commands/fst.md)
    · [:octicons-arrow-right-24: sct semantic](commands/semantic.md)

-   :material-format-list-checks:{ .lg .middle } __Code lists & ECL__

    ---

    Build version-controllable clinical **code lists**, and populate them with
    **SNOMED CT Expression Constraint Language** - `sct codelist add --ecl
    "<<73211009"` expands a query into concrete concepts.

    [:octicons-arrow-right-24: sct codelist](commands/codelist.md)

-   :material-robot:{ .lg .middle } __Connect to AI__

    ---

    A local **MCP server** exposes SNOMED CT as tools for Claude, Cursor, and
    any other MCP-compatible client. Ask questions about concepts, hierarchies,
    and relationships directly in your AI assistant.

    [:octicons-arrow-right-24: sct mcp](commands/mcp.md)

-   :material-server:{ .lg .middle } __Run a terminology server__

    ---

    Start a FHIR R4 SNOMED CT terminology server on a clean VPS with Docker
    Compose. First boot downloads from TRUD, builds `snomed.db`, and serves
    `$lookup`, `$expand`, `$subsumes`, and `$translate`.

    [:octicons-arrow-right-24: Get your own server](deploy/index.md)
    · [:octicons-arrow-right-24: sct serve](commands/serve.md)

-   :material-compass:{ .lg .middle } __Explore__

    ---

    A keyboard-driven **terminal UI** and a local **web GUI** for browsing
    concepts, navigating hierarchies, and inspecting relationships - no browser
    extension or remote service needed.

    [:octicons-arrow-right-24: sct tui](commands/tui.md)
    · [:octicons-arrow-right-24: sct gui](commands/gui.md)

</div>

## Command-line contracts

Commands reserve stdout for data and stderr for diagnostics and human-facing hints. A completed command exits `0`; an unresolved single-item lookup exits `1`; and command-line usage errors exit `2`. A search with no matches is still successful: text and ID output leave stdout empty while the hint goes to stderr, and structured formats emit an empty collection. Commands with domain-specific status codes document their exceptions, such as `sct trud check` using `2` when a newer release is available.
