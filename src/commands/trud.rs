// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct trud` - Download SNOMED CT RF2 releases via the NHS TRUD API.
//!
//! Subcommands:
//!   sct trud auth     - store your API key in the config file (one-time setup)
//!   sct trud list     - list available releases for an edition/item
//!   sct trud check    - check whether a newer release is available (exit 0/2)
//!   sct trud download - download a release, verifying SHA-256, with optional pipeline
//!
//! API key resolution order (first non-empty value wins):
//!   1. --api-key <KEY>           plain string flag
//!   2. --api-key-file <PATH>     first line of the named file
//!   3. $TRUD_API_KEY             environment variable (recommended for CI/cron)
//!   4. api_key in ~/.config/sct/config.toml (write it with `sct trud auth`)

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufWriter, IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tempfile::NamedTempFile;

use crate::humanize::human_bytes as human_size;
use crate::paths::{self, Config};

// ---------------------------------------------------------------------------
// TRUD endpoint constants - change here if NHS TRUD ever moves their API.
// ---------------------------------------------------------------------------
/// Base URL for the TRUD REST API (v1).
const TRUD_API_BASE: &str = "https://isd.digital.nhs.uk/trud/api/v1";
/// TRUD account page where users can find or regenerate their API key.
const TRUD_ACCOUNT_URL: &str =
    "https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/account/manage";
/// Stable public TRUD page used as a connectivity pre-flight check.
/// No authentication required. Any HTTP response (even 4xx/5xx) proves the
/// host is reachable; only connection-level errors indicate the service is down.
const TRUD_HEALTH_URL: &str = "https://isd.digital.nhs.uk/trud/users/guest/filters/0/home";

/// TRUD API base URL. Overridable via `SCT_TRUD_API_BASE` so the network-layer
/// tests can point the client at a local mock server; defaults to production.
fn trud_api_base() -> String {
    std::env::var("SCT_TRUD_API_BASE").unwrap_or_else(|_| TRUD_API_BASE.to_string())
}

/// TRUD health-check URL. Overridable via `SCT_TRUD_HEALTH_URL` (test seam).
fn trud_health_url() -> String {
    std::env::var("SCT_TRUD_HEALTH_URL").unwrap_or_else(|_| TRUD_HEALTH_URL.to_string())
}

// ---------------------------------------------------------------------------
// sct directory layout
// ---------------------------------------------------------------------------
//
// Directory layout, env vars, and config schema are defined in
// `crate::paths` and `spec/path-resolution.md`. This module only re-exports
// the data-dir subdirectory constants for write-side use.

use crate::paths::{DATA_SUBDIR, RELEASES_SUBDIR};

// ---------------------------------------------------------------------------
// CLI types
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: TrudCommand,
}

#[derive(Subcommand, Debug)]
pub enum TrudCommand {
    /// Store your TRUD API key in the config file, creating it if needed.
    ///
    /// This is the one-time setup step: it creates $SCT_CONFIG_HOME (normally
    /// ~/.config/sct), writes config.toml with mode 0600, and sets api_key in
    /// the [trud] section, leaving every other section and comment untouched.
    ///
    /// The key is checked against TRUD before being written, so a typo is
    /// reported now rather than on your first download. Pass --no-verify to
    /// skip that round-trip when offline.
    ///
    /// Supply the key as an argument, from a file with --api-key-file, or on
    /// stdin (recommended - it keeps the key out of your shell history):
    ///
    ///   sct trud auth < my-key.txt
    ///
    ///   pass show nhs/trud | sct trud auth
    Auth(AuthArgs),

    /// List available releases for a TRUD edition/item, newest first.
    List(ListArgs),

    /// Check whether a newer release is available.
    ///
    /// Compares the latest TRUD release against what is on disk, and - if the
    /// local file is present - verifies its SHA-256 against the TRUD metadata
    /// so a corrupt or half-downloaded local file is not reported as current.
    ///
    /// Exit codes: 0 = already up to date and SHA-256 verified, 2 = new release
    /// available OR local file fails checksum, 1 = error. Use exit code 2 (not
    /// 1) in shell scripts to distinguish "action required" from an error.
    Check(CheckArgs),

    /// Download a SNOMED CT RF2 release from TRUD, with SHA-256 verification.
    Download(DownloadArgs),
}

/// Flags for supplying the TRUD API key - shared across all subcommands.
#[derive(Parser, Debug)]
struct KeyArgs {
    /// TRUD API key as a plain string.
    ///
    /// Avoid where possible: the key is visible in process listings and shell
    /// history. Prefer --api-key-file or the TRUD_API_KEY environment variable.
    #[arg(long)]
    api_key: Option<String>,

    /// Path to a file whose first line is the TRUD API key.
    ///
    /// The file may contain only the key and optional trailing whitespace.
    /// Only the first line is read.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    api_key_file: Option<PathBuf>,
}

#[derive(Parser, Debug)]
pub struct AuthArgs {
    /// TRUD API key.
    ///
    /// Omit it (or pass `-`) to read the key from stdin instead, which keeps it
    /// out of your shell history and out of process listings.
    api_key: Option<String>,

    /// Read the key from the first line of this file.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    api_key_file: Option<PathBuf>,

    /// Config file to write.
    ///
    /// Defaults to $SCT_CONFIG when set, otherwise $SCT_CONFIG_HOME/config.toml
    /// (normally ~/.config/sct/config.toml). Note this ignores a ./sct.toml in
    /// the current directory - project-local files are often version-controlled,
    /// so we never write a secret there unless you name it explicitly.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    config: Option<PathBuf>,

    /// Write the key without checking it against the TRUD API first.
    #[arg(long)]
    no_verify: bool,

    /// Report what would change, without writing anything.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Named edition profile: uk_monolith, uk_clinical, uk_drug, nhs_data_migration.
    ///
    /// If omitted (and --item is not given), shows subscription status for all
    /// built-in editions. If supplied, lists all releases for that edition.
    #[arg(long)]
    edition: Option<String>,

    /// Raw TRUD item number - overrides --edition.
    #[arg(long)]
    item: Option<u32>,

    #[command(flatten)]
    key: KeyArgs,
}

#[derive(Parser, Debug)]
pub struct CheckArgs {
    /// Named edition profile: uk_monolith (default), uk_clinical, uk_drug, nhs_data_migration.
    #[arg(long, default_value = "uk_monolith")]
    edition: String,

    /// Raw TRUD item number - overrides --edition.
    #[arg(long)]
    item: Option<u32>,

    #[command(flatten)]
    key: KeyArgs,
}

#[derive(Parser, Debug)]
pub struct DownloadArgs {
    /// Named edition profile: uk_monolith (default), uk_clinical, uk_drug, nhs_data_migration.
    #[arg(long, default_value = "uk_monolith")]
    edition: String,

    /// Raw TRUD item number - overrides --edition.
    #[arg(long)]
    item: Option<u32>,

    /// Download a specific named version (e.g. 41.5.0). Defaults to latest.
    #[arg(long)]
    release: Option<String>,

    /// Directory for the downloaded RF2 zip.
    /// Defaults to download_dir in config, then $SCT_DATA_HOME/releases.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    output_dir: Option<PathBuf>,

    /// Directory for built artefacts produced by --pipeline / --pipeline-full
    /// (.ndjson, .db, .arrow). Defaults to data_dir in config,
    /// then $SCT_DATA_HOME/data.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    data_dir: Option<PathBuf>,

    /// Do nothing (exit 0) if the latest release zip is already present and
    /// its SHA-256 matches. Safe to use in cron jobs.
    #[arg(long)]
    skip_if_current: bool,

    /// After a successful download, run `sct ndjson` then `sct sqlite` automatically.
    #[arg(long)]
    pipeline: bool,

    /// As --pipeline, plus `sct tct` and `sct embed`.
    /// The embed step is skipped with a warning if Ollama is not reachable.
    #[arg(long)]
    pipeline_full: bool,

    /// Build a ready-to-use multi-terminology workspace.
    ///
    /// Implies --pipeline, --include-inactive, --refsets all, and --with-read2.
    /// Downloads the UK Monolith plus TRUD item 9, then imports CTV3, Read v2,
    /// ICD-10, OPCS-4, and concept history into one SQLite database.
    #[arg(long)]
    multi_terminology: bool,

    /// After the SNOMED pipeline, download TRUD item 9 and import final Read v2
    /// maps into the generated SQLite database. Requires --pipeline or
    /// --pipeline-full unless --multi-terminology is used.
    #[arg(long)]
    with_read2: bool,

    /// BCP-47 locale for preferred-term selection in the pipelined `sct ndjson`
    /// step (e.g. en-GB, en-US). Only used with --pipeline / --pipeline-full.
    #[arg(long, default_value = "en-GB")]
    locale: String,

    /// Include inactive concepts in the pipelined `sct ndjson` step. Only takes
    /// effect with --pipeline / --pipeline-full.
    #[arg(long, default_value_t = false)]
    include_inactive: bool,

    /// Which reference sets the pipelined `sct ndjson` step loads: `simple`
    /// (default), `none`, or `all` (adds ICD-10/OPCS-4 crossmaps and concept
    /// history). Only takes effect with --pipeline / --pipeline-full.
    #[arg(long, value_enum, default_value_t = super::ndjson::RefsetMode::default())]
    refsets: super::ndjson::RefsetMode,

    #[command(flatten)]
    key: KeyArgs,
}

// ---------------------------------------------------------------------------
// TRUD API response types
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
struct TrudListResponse {
    releases: Vec<TrudRelease>,
}

#[derive(Deserialize, Debug, Clone)]
struct TrudRelease {
    #[serde(rename = "archiveFileUrl")]
    archive_file_url: String,
    #[serde(
        rename = "archiveFileName",
        deserialize_with = "deserialize_archive_file_name"
    )]
    archive_file_name: String,
    #[serde(rename = "archiveFileSizeBytes")]
    archive_file_size_bytes: u64,
    #[serde(rename = "archiveFileSha256")]
    archive_file_sha256: String,
    #[serde(rename = "releaseDate")]
    release_date: String,
}

/// TRUD metadata is remote input: require a plain filename before any caller
/// can join it to a local directory. Explicitly reject both separator styles so
/// the behaviour is identical on Unix and Windows.
fn deserialize_archive_file_name<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let name = String::deserialize(deserializer)?;
    let mut components = Path::new(&name).components();
    let is_single_normal_component =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if !is_single_normal_component
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(serde::de::Error::custom(
            "unsafe TRUD archiveFileName: expected a plain filename",
        ));
    }
    Ok(name)
}

// ---------------------------------------------------------------------------
// Config schema lives in crate::paths.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Built-in edition definitions
// ---------------------------------------------------------------------------

struct BuiltinEdition {
    trud_item: u32,
    #[allow(dead_code)] // reserved for `sct trud list --editions` display
    description: &'static str,
}

fn builtin_editions() -> HashMap<&'static str, BuiltinEdition> {
    let mut m = HashMap::new();
    m.insert(
        "uk_monolith",
        BuiltinEdition {
            trud_item: 1799,
            description: "UK Monolith (International + UK Clinical + UK Drug/dm+d + UK Pathology)",
        },
    );
    m.insert(
        "uk_clinical",
        BuiltinEdition {
            trud_item: 101,
            description: "UK Clinical Edition (International + UK Clinical, no dm+d)",
        },
    );
    m.insert(
        "uk_drug",
        BuiltinEdition {
            trud_item: 105,
            description: "UK Drug Extension (dm+d only)",
        },
    );
    m.insert(
        "nhs_data_migration",
        BuiltinEdition {
            trud_item: 9,
            description: "NHS Data Migration Pack (final Read v2 / CTV3 / SNOMED CT maps)",
        },
    );
    m
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(args: Args) -> Result<()> {
    match args.subcommand {
        TrudCommand::Auth(a) => run_auth(a),
        TrudCommand::List(a) => run_list(a),
        TrudCommand::Check(a) => run_check(a),
        TrudCommand::Download(a) => run_download(a),
    }
}

// ---------------------------------------------------------------------------
// sct trud auth
// ---------------------------------------------------------------------------

/// Item probed to prove a key works. uk_monolith is the default edition
/// everywhere else in this module, so a key that can see it is a key that can
/// run `sct trud download` with no further flags.
const VERIFY_ITEM_ID: u32 = 1799;

fn run_auth(args: AuthArgs) -> Result<()> {
    let key = read_auth_key(args.api_key.as_deref(), args.api_key_file.as_deref())?;

    if !args.no_verify {
        verify_api_key(&key)?;
    }

    let path = auth_config_path(args.config.as_deref());
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading config file {}", path.display())),
    };

    let edit = set_config_api_key(&existing, &key)
        .with_context(|| format!("updating config file {}", path.display()))?;

    if args.dry_run {
        eprintln!(
            "sct trud auth: would {} api_key in {}",
            match &edit.previous {
                Some(_) => "replace",
                None => "set",
            },
            path.display()
        );
        print!("{}", edit.text);
        return Ok(());
    }

    write_config_file(&path, &edit.text)?;

    match &edit.previous {
        Some(previous) if previous == &key => {
            eprintln!("sct trud auth: key unchanged ({})", mask_key(&key));
        }
        Some(previous) => {
            eprintln!(
                "sct trud auth: replaced existing key {} with {}",
                mask_key(previous),
                mask_key(&key)
            );
        }
        None => eprintln!("sct trud auth: stored key {}", mask_key(&key)),
    }
    eprintln!("sct trud auth: wrote {}", path.display());

    // The env var outranks the config file in resolve_api_key, so a stale
    // TRUD_API_KEY would silently win over what we just wrote.
    if let Ok(env_key) = std::env::var("TRUD_API_KEY") {
        if !env_key.trim().is_empty() && env_key.trim() != key {
            eprintln!(
                "sct trud auth: WARNING - $TRUD_API_KEY is set to a different key ({}) and takes \
                 precedence over the config file. Unset it to use the key just stored.",
                mask_key(env_key.trim())
            );
        }
    }

    eprintln!("sct trud auth: next step - sct trud list");
    Ok(())
}

/// Resolve the key from the argument, a file, or stdin.
fn read_auth_key(arg: Option<&str>, file: Option<&Path>) -> Result<String> {
    if let Some(path) = file {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading API key file {}", path.display()))?;
        let key = contents.lines().next().unwrap_or("").trim().to_string();
        return validate_key_shape(key)
            .with_context(|| format!("in API key file {}", path.display()));
    }

    // `-` is the repo-wide convention for "read from stdin".
    if let Some(arg) = arg.filter(|a| *a != "-") {
        eprintln!(
            "sct trud auth: note - a key passed as an argument is recorded in your shell \
             history and visible in process listings. Prefer `sct trud auth < keyfile`."
        );
        return validate_key_shape(arg.trim().to_string());
    }

    if std::io::stdin().is_terminal() {
        eprint!("TRUD API key (visible as you type): ");
        std::io::stderr().flush().ok();
    }
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading API key from stdin")?;
    validate_key_shape(line.trim().to_string())
        .context("no API key on stdin. Pass it as an argument, use --api-key-file, or pipe it in")
}

/// Reject input that cannot be a TRUD key, so we fail here rather than writing
/// a broken config and failing on the next command.
fn validate_key_shape(key: String) -> Result<String> {
    if key.is_empty() {
        anyhow::bail!("API key is empty");
    }
    if key.chars().any(|c| c.is_whitespace()) {
        anyhow::bail!(
            "API key contains whitespace - did a label or a second field get pasted with it?"
        );
    }
    if key.chars().any(|c| c.is_control()) {
        anyhow::bail!("API key contains control characters");
    }
    Ok(key)
}

/// Check the key against TRUD. A key that is merely unsubscribed to the probed
/// item still proves the key itself is good, so only an outright rejection is
/// fatal; anything else (TRUD down, no network) is reported and allowed through
/// so that setup works offline.
fn verify_api_key(key: &str) -> Result<()> {
    match probe_edition(key, VERIFY_ITEM_ID) {
        Ok(Some(_)) => {
            eprintln!("sct trud auth: key verified against TRUD (uk_monolith subscribed)");
            Ok(())
        }
        Ok(None) => {
            eprintln!(
                "sct trud auth: key accepted by TRUD, but this account is not subscribed to \
                 uk_monolith (item {VERIFY_ITEM_ID}). Run `sct trud list` to see what it can \
                 reach."
            );
            Ok(())
        }
        // probe_edition maps TRUD's HTTP 400 to exactly this: a bad key.
        Err(e) if e.to_string().contains("TRUD API key invalid") => Err(e),
        Err(e) => {
            eprintln!("sct trud auth: WARNING - could not verify the key: {e}");
            eprintln!("sct trud auth: storing it anyway; re-run `sct trud list` when back online.");
            Ok(())
        }
    }
}

/// Config file `sct trud auth` writes to.
///
/// Unlike [`paths::config_path`], this deliberately ignores `./sct.toml`: a
/// project-local config is usually version-controlled, and a secret should not
/// land there by accident. `--config` overrides this.
fn auth_config_path(flag: Option<&Path>) -> PathBuf {
    if let Some(path) = flag {
        return path.to_path_buf();
    }
    if let Ok(value) = std::env::var("SCT_CONFIG") {
        if !value.trim().is_empty() {
            return paths::expand_tilde(value.trim());
        }
    }
    paths::config_home().join("config.toml")
}

/// Result of editing a config file's `api_key`.
///
/// Deliberately not `Debug`: it holds the key in the clear, and the whole point
/// of `mask_key` is that the key never reaches a log or an error message.
struct ConfigEdit {
    text: String,
    /// The key that was there before, if any.
    previous: Option<String>,
}

/// Set `api_key` in the `[trud]` section of `existing`, returning the new file.
///
/// This is a targeted line edit rather than a parse-and-reserialise, because a
/// round trip through a TOML value tree would discard the user's comments,
/// ordering, and formatting. The input is parsed first (so we never write to a
/// file we do not understand) and the output is parsed again (so we never write
/// an edit that did not land where we intended).
fn set_config_api_key(existing: &str, key: &str) -> Result<ConfigEdit> {
    let parsed: toml::Table = toml::from_str(existing)
        .context("config file is not valid TOML - fix or move it, then re-run")?;
    let previous = parsed
        .get("trud")
        .and_then(|t| t.get("api_key"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // toml::Value's Display is the TOML representation, so this quotes and
    // escapes the key correctly whatever it contains.
    let assignment = format!("api_key = {}", toml::Value::from(key));

    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();

    if let Some(header) = lines.iter().position(|l| l.trim() == "[trud]") {
        // End of the [trud] section: the next table header, or end of file.
        let end = lines
            .iter()
            .skip(header + 1)
            .position(|l| l.trim_start().starts_with('['))
            .map(|offset| header + 1 + offset)
            .unwrap_or(lines.len());

        match (header + 1..end).find(|&i| is_api_key_assignment(&lines[i])) {
            Some(i) => {
                let indent: String = lines[i]
                    .chars()
                    .take_while(|c| c.is_whitespace() && *c != '\n')
                    .collect();
                lines[i] = format!("{indent}{assignment}");
            }
            None => lines.insert(header + 1, assignment),
        }
    } else if let Some(subtable) = lines
        .iter()
        .position(|l| l.trim_start().starts_with("[trud."))
    {
        // Only subtables like [trud.editions.foo] exist. Declaring [trud] after
        // them is legal TOML but confusing to read, so go in above the first.
        lines.insert(subtable, "[trud]".to_string());
        lines.insert(subtable + 1, assignment);
        lines.insert(subtable + 2, String::new());
    } else {
        if lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[trud]".to_string());
        lines.push(assignment);
    }

    let mut text = lines.join("\n");
    text.push('\n');

    // Confirm the edit produced the config we meant to write.
    let check: Config = toml::from_str(&text)
        .context("internal error: edited config is not valid TOML (config left unchanged)")?;
    let landed = check.trud.as_ref().and_then(|t| t.api_key.as_deref());
    if landed != Some(key) {
        anyhow::bail!(
            "internal error: api_key did not take effect after editing (config left unchanged)"
        );
    }

    Ok(ConfigEdit { text, previous })
}

/// Is this line an `api_key = ...` assignment (bare or quoted key)?
fn is_api_key_assignment(line: &str) -> bool {
    let trimmed = line.trim_start();
    for prefix in ["api_key", "\"api_key\"", "'api_key'"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            if rest.trim_start().starts_with('=') {
                return true;
            }
        }
    }
    false
}

/// Write the config file with owner-only permissions, atomically.
///
/// The file holds a credential, so it is created 0600 (and a directory we create
/// ourselves 0700) and written via a temporary file in the same directory, so a
/// crash mid-write cannot truncate an existing config.
fn write_config_file(path: &Path, contents: &str) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating config directory {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).with_context(
                || format!("setting permissions on config directory {}", dir.display()),
            )?;
        }
    }

    let mut tmp = NamedTempFile::new_in(dir)
        .with_context(|| format!("creating temporary file in {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("setting permissions on the new config file")?;
    }
    tmp.write_all(contents.as_bytes())
        .context("writing config file")?;
    tmp.as_file().sync_all().context("flushing config file")?;
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("replacing {}: {}", path.display(), e))?;
    Ok(())
}

/// Render a key for display, showing only the last four characters.
fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 4 {
        return "*".repeat(chars.len());
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{}{tail}", "*".repeat(chars.len() - 4))
}

// ---------------------------------------------------------------------------
// sct trud list
// ---------------------------------------------------------------------------

fn run_list(args: ListArgs) -> Result<()> {
    let config = load_config();
    let api_key = resolve_api_key(
        args.key.api_key.as_deref(),
        args.key.api_key_file.as_deref(),
        &config,
    )?;

    // No edition or item specified → show subscription status for all built-ins.
    if args.item.is_none() && args.edition.is_none() {
        ping_trud()?;
        return run_list_all(&api_key);
    }

    let edition = args.edition.as_deref().unwrap_or("uk_monolith");
    let item_id = resolve_item_id(args.item, edition, &config)?;

    let releases = fetch_releases(&api_key, item_id, false)?;

    if releases.is_empty() {
        println!("No releases found for TRUD item {item_id}.");
        return Ok(());
    }

    println!(
        "{:<52}  {:<12}  {:>8}  SHA-256 (first 12 chars)",
        "File", "Released", "Size"
    );
    println!("{}", "-".repeat(92));
    for r in &releases {
        let sha_prefix = &r.archive_file_sha256[..r.archive_file_sha256.len().min(12)];
        println!(
            "{:<52}  {:<12}  {:>8}  {}",
            r.archive_file_name,
            r.release_date,
            human_size(r.archive_file_size_bytes),
            sha_prefix,
        );
    }
    Ok(())
}

/// Show subscription status for all built-in editions in a summary table.
///
/// Called when `sct trud list` is run without --edition or --item.
/// Probes the TRUD API for each built-in edition and reports whether the
/// account is subscribed, along with the latest available release if so.
fn run_list_all(api_key: &str) -> Result<()> {
    // Fixed display order for the three built-in editions.
    let editions: &[(&str, u32, &str)] = &[
        (
            "uk_monolith",
            1799,
            "International + UK Clinical + UK Drug/dm+d + UK Pathology",
        ),
        ("uk_clinical", 101, "International + UK Clinical (no dm+d)"),
        ("uk_drug", 105, "UK Drug Extension / dm+d only"),
    ];

    println!(
        "{:<16}  {:>4}  {:<14}  {:<52}  Released",
        "Edition", "Item", "Status", "Latest release"
    );
    println!("{}", "-".repeat(100));

    for (name, item_id, _desc) in editions {
        match probe_edition(api_key, *item_id)? {
            Some(release) => {
                println!(
                    "{:<16}  {:>4}  {:<14}  {:<52}  {}",
                    name, item_id, "subscribed", release.archive_file_name, release.release_date
                );
            }
            None => {
                println!(
                    "{:<16}  {:>4}  {:<14}  {:<52}  -",
                    name, item_id, "not subscribed", "-"
                );
            }
        }
    }

    println!();
    println!("To subscribe: https://isd.digital.nhs.uk/trud/users/authenticated/filters/0/home");
    println!("To list all releases for a subscribed edition:");
    println!("  sct trud list --edition <NAME>");
    Ok(())
}

// ---------------------------------------------------------------------------
// sct trud check
// ---------------------------------------------------------------------------

fn run_check(args: CheckArgs) -> Result<()> {
    let config = load_config();
    let api_key = resolve_api_key(
        args.key.api_key.as_deref(),
        args.key.api_key_file.as_deref(),
        &config,
    )?;
    let item_id = resolve_item_id(args.item, &args.edition, &config)?;
    let releases_dir = resolve_releases_dir(None, &config);

    let releases = fetch_releases(&api_key, item_id, true)?;
    let latest = releases
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No releases found for TRUD item {item_id}"))?;

    let local_path = releases_dir.join(&latest.archive_file_name);

    if !local_path.exists() {
        println!(
            "New release available: {} ({})",
            latest.archive_file_name, latest.release_date
        );
        // exit 2 - not an error, but signals "please update"
        std::process::exit(2);
    }

    // File exists - verify its SHA-256 against the TRUD metadata so we don't
    // report a corrupt or half-downloaded local file as "up to date".
    let local_hash = sha256_of_file(&local_path)?;
    if local_hash.eq_ignore_ascii_case(&latest.archive_file_sha256) {
        println!(
            "Up to date: {} ({})\nSHA-256 verified: {}",
            latest.archive_file_name, latest.release_date, latest.archive_file_sha256
        );
        // exit 0 - already current and intact
        return Ok(());
    }

    // File is present but does not match the expected checksum. Treat this as
    // "action required" - exit 2, same as "new release available" - so shell
    // scripts that re-download on exit 2 will heal a corrupt local file.
    println!(
        "Local file present but SHA-256 does not match TRUD metadata - re-download recommended: {}\n\
         Expected: {}\n\
         Got:      {}",
        latest.archive_file_name, latest.archive_file_sha256, local_hash
    );
    std::process::exit(2);
}

// ---------------------------------------------------------------------------
// sct trud download
// ---------------------------------------------------------------------------

fn run_download(mut args: DownloadArgs) -> Result<()> {
    let config = load_config();
    if args.multi_terminology {
        args.pipeline = true;
        args.include_inactive = true;
        args.refsets = super::ndjson::RefsetMode::All;
        args.with_read2 = true;
    }

    let api_key = resolve_api_key(
        args.key.api_key.as_deref(),
        args.key.api_key_file.as_deref(),
        &config,
    )?;
    let item_id = resolve_item_id(args.item, &args.edition, &config)?;
    let releases_dir = resolve_releases_dir(args.output_dir.as_deref(), &config);
    let data_dir = resolve_data_dir(args.data_dir.as_deref(), &config);

    std::fs::create_dir_all(&releases_dir)
        .with_context(|| format!("creating releases directory {}", releases_dir.display()))?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating data directory {}", data_dir.display()))?;

    if item_id == 9 && (args.pipeline || args.pipeline_full) {
        anyhow::bail!(
            "TRUD item 9 is not an RF2 release and cannot use --pipeline. \
             To import Read v2 into an existing database, run: \
             sct read2 import --archive <item9.zip> --db <snomed.db>"
        );
    }
    if args.with_read2 && !args.pipeline && !args.pipeline_full {
        anyhow::bail!("--with-read2 requires --pipeline, --pipeline-full, or --multi-terminology");
    }

    // Fetch release metadata
    let latest_only = args.release.is_none();
    let releases = fetch_releases(&api_key, item_id, latest_only)?;

    let release = if let Some(ref version) = args.release {
        releases
            .into_iter()
            .find(|r| r.archive_file_name.contains(version))
            .ok_or_else(|| {
                anyhow::anyhow!("No release found matching version '{version}' for item {item_id}")
            })?
    } else {
        releases
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No releases found for TRUD item {item_id}"))?
    };

    let dest = releases_dir.join(&release.archive_file_name);

    // Check if already present with a matching SHA-256
    if dest.exists() {
        let existing_hash = sha256_of_file(&dest)?;
        if existing_hash.eq_ignore_ascii_case(&release.archive_file_sha256) {
            if args.skip_if_current {
                println!(
                    "Already up to date: {} - skipping download.",
                    release.archive_file_name
                );
                return finish_download(&args, &dest, &data_dir, &api_key, &releases_dir);
            }
            println!(
                "File already present with matching SHA-256: {}",
                release.archive_file_name
            );
            return finish_download(&args, &dest, &data_dir, &api_key, &releases_dir);
        }
        // Checksum mismatch - re-download
        eprintln!(
            "Warning: existing file has unexpected SHA-256 - re-downloading {}",
            release.archive_file_name
        );
    }

    println!(
        "Downloading {} ({}) ...",
        release.archive_file_name,
        human_size(release.archive_file_size_bytes)
    );

    // Keep the temp file in the destination directory so persistence is atomic.
    let mut tmp = NamedTempFile::new_in(&releases_dir)
        .with_context(|| format!("creating temporary file in {}", releases_dir.display()))?;

    let resp = ureq::get(&release.archive_file_url)
        .call()
        .map_err(|e| anyhow::anyhow!("TRUD download request failed: {e}"))?;

    let content_length: Option<u64> = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    let pb = match content_length {
        Some(total) => {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(
                        "{spinner:.green} [{elapsed_precise}] \
                         [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
                    )
                    .unwrap()
                    .progress_chars("#>-"),
            );
            pb
        }
        None => {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} [{elapsed_precise}] {bytes} downloaded")
                    .unwrap(),
            );
            pb
        }
    };
    pb.enable_steady_tick(Duration::from_millis(120));

    // Write to temp file while computing the SHA-256 in one pass
    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        let mut hasher = Sha256::new();
        let mut body_reader = resp.into_body().into_reader();
        let mut buf = [0u8; 65536]; // 64 KiB chunks
        let mut downloaded: u64 = 0;

        loop {
            let n = body_reader
                .read(&mut buf)
                .context("reading download response body")?;
            if n == 0 {
                break;
            }
            writer
                .write_all(&buf[..n])
                .context("writing to temp file")?;
            hasher.update(&buf[..n]);
            downloaded += n as u64;
            pb.set_position(downloaded);
        }
        writer.flush().context("flushing temp file")?;

        pb.finish_with_message(format!(
            "Downloaded {} ({})",
            release.archive_file_name,
            human_size(downloaded)
        ));

        // Verify SHA-256 before committing the file
        let computed = hex_upper(&hasher.finalize());
        if !computed.eq_ignore_ascii_case(&release.archive_file_sha256) {
            anyhow::bail!(
                "SHA-256 checksum mismatch - download may be corrupt. Temporary file deleted.\n\
                 Expected: {}\n\
                 Got:      {}",
                release.archive_file_sha256,
                computed
            );
        }
    }

    tmp.persist(&dest)
        .map_err(|e| e.error)
        .with_context(|| format!("persisting verified download to {}", dest.display()))?;
    println!("✓ Saved: {}", dest.display());
    if args.pipeline || args.pipeline_full {
        println!("  Built artefacts will go to: {}", data_dir.display());
    }

    finish_download(&args, &dest, &data_dir, &api_key, &releases_dir)
}

// ---------------------------------------------------------------------------
// Pipeline chaining
// ---------------------------------------------------------------------------

fn finish_download(
    args: &DownloadArgs,
    zip_path: &Path,
    data_dir: &Path,
    api_key: &str,
    releases_dir: &Path,
) -> Result<()> {
    let db_path = run_pipeline_if_requested(args, zip_path, data_dir)?;
    if args.with_read2 {
        let Some(db_path) = db_path else {
            anyhow::bail!("--with-read2 requires a generated SQLite database");
        };
        println!("\n→ Running: sct read2 import");
        let item9 = ensure_latest_release(api_key, 9, releases_dir)?;
        let summary = super::read2::import_archive(&db_path, &item9)
            .context("sct read2 import step failed")?;
        println!(
            "✓ Read v2 imported: {} active source key(s), {} target concept(s)",
            summary.distinct_source_keys, summary.distinct_target_concepts
        );
    }
    Ok(())
}

fn run_pipeline_if_requested(
    args: &DownloadArgs,
    zip_path: &Path,
    data_dir: &Path,
) -> Result<Option<PathBuf>> {
    if !args.pipeline && !args.pipeline_full {
        return Ok(None);
    }

    // Name the artefacts after the release, using the same slug `sct ndjson`
    // would pick for this zip, so a TRUD-built workspace and a hand-built one
    // are named identically.
    let stem = super::ndjson::slugify_path(zip_path);
    let ndjson_path = data_dir.join(format!("{stem}.ndjson"));
    let db_path = data_dir.join(format!("{stem}{}", paths::suffix::DB));

    // --- sct ndjson ---
    println!("\n→ Running: sct ndjson");
    super::ndjson::run(super::ndjson::Args {
        rf2_dirs: vec![zip_path.to_path_buf()],
        locale: args.locale.clone(),
        output: Some(ndjson_path.clone()),
        include_inactive: args.include_inactive,
        refsets: args.refsets,
    })
    .context("sct ndjson step failed")?;

    // --- sct sqlite ---
    println!("\n→ Running: sct sqlite");
    super::sqlite::run(super::sqlite::Args {
        input: ndjson_path.clone(),
        output: Some(db_path.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .context("sct sqlite step failed")?;

    if args.pipeline_full {
        // --- sct tct ---
        println!("\n→ Running: sct tct");
        super::tct::run(super::tct::Args {
            db: db_path.clone(),
            include_self: false,
        })
        .context("sct tct step failed")?;

        // --- sct embed (best-effort - skip if Ollama unavailable) ---
        println!("\n→ Running: sct embed");
        let arrow_path = data_dir.join(format!("{stem}{}", paths::suffix::EMBEDDINGS));
        if let Err(e) = super::embed::run(super::embed::Args {
            input: ndjson_path.clone(),
            model: "nomic-embed-text".into(),
            ollama_url: "http://localhost:11434".into(),
            output: Some(arrow_path),
            batch_size: 64,
        }) {
            eprintln!("Warning: sct embed skipped - {e}");
            eprintln!("  (Is Ollama running? Start with: ollama serve)");
        }
    }

    println!("\n✓ Pipeline complete.");
    println!("  NDJSON: {}", ndjson_path.display());
    println!("  SQLite: {}", db_path.display());
    Ok(Some(db_path))
}

fn ensure_latest_release(api_key: &str, item_id: u32, releases_dir: &Path) -> Result<PathBuf> {
    let release = fetch_releases(api_key, item_id, true)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No releases found for TRUD item {item_id}"))?;
    let dest = releases_dir.join(&release.archive_file_name);
    if dest.exists() {
        let existing_hash = sha256_of_file(&dest)?;
        if existing_hash.eq_ignore_ascii_case(&release.archive_file_sha256) {
            println!(
                "File already present with matching SHA-256: {}",
                release.archive_file_name
            );
            return Ok(dest);
        }
        eprintln!(
            "Warning: existing file has unexpected SHA-256 - re-downloading {}",
            release.archive_file_name
        );
    }
    download_release(&release, &dest)?;
    Ok(dest)
}

fn download_release(release: &TrudRelease, dest: &Path) -> Result<()> {
    println!(
        "Downloading {} ({}) ...",
        release.archive_file_name,
        human_size(release.archive_file_size_bytes)
    );

    let dir = dest.parent().unwrap_or(Path::new("."));
    let mut tmp = NamedTempFile::new_in(dir)
        .with_context(|| format!("creating temporary file in {}", dir.display()))?;
    let resp = ureq::get(&release.archive_file_url)
        .call()
        .map_err(|e| anyhow::anyhow!("TRUD download request failed: {e}"))?;

    let content_length: Option<u64> = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    let pb = match content_length {
        Some(total) => ProgressBar::new(total),
        None => ProgressBar::new_spinner(),
    };
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] \
                 [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
            )
            .unwrap()
            .progress_chars("#>-"),
    );
    pb.enable_steady_tick(Duration::from_millis(120));

    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        let mut hasher = Sha256::new();
        let mut body_reader = resp.into_body().into_reader();
        let mut buf = [0u8; 65536];
        let mut downloaded: u64 = 0;

        loop {
            let n = body_reader
                .read(&mut buf)
                .context("reading download response body")?;
            if n == 0 {
                break;
            }
            writer
                .write_all(&buf[..n])
                .context("writing to temp file")?;
            hasher.update(&buf[..n]);
            downloaded += n as u64;
            pb.set_position(downloaded);
        }
        writer.flush().context("flushing temp file")?;
        pb.finish_with_message(format!(
            "Downloaded {} ({})",
            release.archive_file_name,
            human_size(downloaded)
        ));

        let computed = hex_upper(&hasher.finalize());
        if !computed.eq_ignore_ascii_case(&release.archive_file_sha256) {
            anyhow::bail!(
                "SHA-256 checksum mismatch - download may be corrupt. Temporary file deleted.\n\
                 Expected: {}\n\
                 Got:      {}",
                release.archive_file_sha256,
                computed
            );
        }
    }

    tmp.as_file()
        .sync_all()
        .context("flushing downloaded file to disk")?;

    tmp.persist(dest)
        .map_err(|e| e.error)
        .with_context(|| format!("persisting verified download to {}", dest.display()))?;
    println!("✓ Saved: {}", dest.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// API key resolution
// ---------------------------------------------------------------------------

fn resolve_api_key(
    flag_key: Option<&str>,
    flag_key_file: Option<&Path>,
    config: &Config,
) -> Result<String> {
    // 1. --api-key flag
    if let Some(key) = flag_key {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
    }

    // 2. --api-key-file flag
    if let Some(path) = flag_key_file {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading API key file {}", path.display()))?;
        let key = contents.lines().next().unwrap_or("").trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
        anyhow::bail!(
            "API key file {} is empty or contains only whitespace.",
            path.display()
        );
    }

    // 3. TRUD_API_KEY environment variable
    if let Ok(key) = std::env::var("TRUD_API_KEY") {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
    }

    // 4. Config file
    if let Some(trud) = &config.trud {
        if let Some(key) = &trud.api_key {
            let key = key.trim().to_string();
            if !key.is_empty() {
                return Ok(key);
            }
        }
    }

    anyhow::bail!(
        "No TRUD API key found. Provide one via:\n\
         \n\
         \x20 --api-key <KEY>                   plain string (visible in process list)\n\
         \x20 --api-key-file <PATH>              file whose first line is the key\n\
         \x20 TRUD_API_KEY=<key> sct trud ...   environment variable (recommended)\n\
         \x20 api_key in ~/.config/sct/config.toml\n\
         \n\
         Your API key is shown at:\n\
         \x20 {TRUD_ACCOUNT_URL}"
    )
}

// ---------------------------------------------------------------------------
// Edition / item resolution
// ---------------------------------------------------------------------------

fn resolve_item_id(flag_item: Option<u32>, edition: &str, config: &Config) -> Result<u32> {
    // --item overrides everything
    if let Some(n) = flag_item {
        return Ok(n);
    }

    // User-defined config editions take precedence over built-ins
    if let Some(trud) = &config.trud {
        if let Some(editions) = &trud.editions {
            if let Some(profile) = editions.get(edition) {
                return Ok(profile.trud_item);
            }
        }
    }

    // Built-in editions
    let builtins = builtin_editions();
    if let Some(b) = builtins.get(edition) {
        return Ok(b.trud_item);
    }

    let names: Vec<_> = {
        let mut v: Vec<_> = builtin_editions()
            .into_iter()
            .map(|(k, v)| format!("{k} (item {})", v.trud_item))
            .collect();
        v.sort();
        v
    };
    anyhow::bail!(
        "Unknown edition '{edition}'. Built-in editions: {}\n\
         Use --item <N> to specify a TRUD item number directly, or define\n\
         [trud.editions.{edition}] in ~/.config/sct/config.toml.",
        names.join(", ")
    )
}

// ---------------------------------------------------------------------------
// Directory resolution
// ---------------------------------------------------------------------------

/// Resolve the directory for downloaded RF2 zip files.
///
/// Resolution order: --output-dir flag → config download_dir → $SCT_DATA_HOME/releases
fn resolve_releases_dir(flag_dir: Option<&Path>, config: &Config) -> PathBuf {
    if let Some(dir) = flag_dir {
        return dir.to_path_buf();
    }
    if let Some(trud) = &config.trud {
        if let Some(dir) = &trud.download_dir {
            return paths::expand_tilde(dir);
        }
    }
    paths::data_home().join(RELEASES_SUBDIR)
}

/// Resolve the directory for built artefacts (.ndjson, .db, .parquet, .arrow).
///
/// Resolution order: --data-dir flag → config data_dir → $SCT_DATA_HOME/data
fn resolve_data_dir(flag_dir: Option<&Path>, config: &Config) -> PathBuf {
    if let Some(dir) = flag_dir {
        return dir.to_path_buf();
    }
    if let Some(trud) = &config.trud {
        if let Some(dir) = &trud.data_dir {
            return paths::expand_tilde(dir);
        }
    }
    paths::data_home().join(DATA_SUBDIR)
}

// ---------------------------------------------------------------------------
// Config file - schema in crate::paths, this thin wrapper preserves the
// historic helper name so tests don't need to be rewritten.
// ---------------------------------------------------------------------------

fn load_config() -> Config {
    paths::load_config()
}

// ---------------------------------------------------------------------------
// TRUD API
// ---------------------------------------------------------------------------

/// Connectivity pre-flight: verify the TRUD host is reachable before making
/// authenticated requests. Any HTTP response proves the service is up; only
/// connection-level errors (DNS failure, TCP timeout, TLS error) mean it is
/// truly unreachable.
///
/// Called automatically at the start of every `fetch_releases` invocation so
/// users get a clear, actionable message rather than a cryptic network error.
fn ping_trud() -> Result<()> {
    let health = trud_health_url();
    match ureq::get(&health).call() {
        // Any HTTP response - including 4xx/5xx - means we reached the server.
        Ok(_) | Err(ureq::Error::StatusCode(_)) => Ok(()),
        Err(e) => Err(anyhow::anyhow!(unreachable_message(
            &health,
            &e.to_string(),
            resolv_conf_present()
        ))),
    }
}

/// Is there resolver configuration a statically linked build could read?
///
/// Non-unix targets never look for this file, so they always report `true` and
/// never see the hint.
fn resolv_conf_present() -> bool {
    if cfg!(unix) {
        Path::new("/etc/resolv.conf").exists()
    } else {
        true
    }
}

/// Extra guidance for the one environment where a name lookup fails no matter
/// how healthy the network is.
///
/// The `linux-aarch64` release is a static musl binary, so it carries musl's own
/// resolver, which reads `/etc/resolv.conf`. Android has no such file: `/etc` is
/// a symlink to the read-only `/system/etc`, and DNS configuration is reached
/// through the `netd` daemon, which only Bionic's `getaddrinfo` knows how to
/// talk to. The result is `EAI_AGAIN` ("Try again") on a device where `ping` and
/// `curl` both work - baffling without this pointer.
fn dns_hint(error: &str, resolv_conf_present: bool) -> Option<&'static str> {
    let looks_like_dns_failure = error.contains("failed to lookup address information")
        || error.contains("Temporary failure in name resolution")
        || error.contains("Name or service not known");

    if looks_like_dns_failure && !resolv_conf_present {
        Some(
            "\nThis system has no /etc/resolv.conf, so a statically linked build of sct has no\n\
             resolver configuration to read - which is why the lookup failed even though the\n\
             network itself is fine. On Android/Termux this is expected; see\n\
             https://pacharanero.github.io/sct/android-termux/ for the ways round it.\n",
        )
    } else {
        None
    }
}

/// The "cannot reach TRUD" diagnostic.
///
/// Split out so the test asserts the message users actually see, rather than a
/// copy of it that can drift.
///
/// TRUD publishes no maintenance schedule. The only statement it makes is the
/// automation guidance on <https://isd.digital.nhs.uk/trud/users/guest/filters/0/api>:
/// "Run automation scripts on weekdays between 8am and 6pm, or midnight and 6am
/// (UK time) to avoid planned maintenance." Quote that, and do not extrapolate a
/// downtime window from it.
fn unreachable_message(health_url: &str, error: &str, resolv_conf_present: bool) -> String {
    let hint = dns_hint(error, resolv_conf_present).unwrap_or("");
    format!(
        "Cannot reach NHS TRUD ({health_url}).

The service may be offline or undergoing scheduled maintenance. TRUD advises
running automation on weekdays 08:00-18:00 or 00:00-06:00 UK time to avoid
planned maintenance.
{hint}
Original error: {error}"
    )
}

/// Probe a single TRUD item to determine subscription status.
///
/// Returns:
///   Ok(Some(release)) - subscribed; `release` is the latest available release
///   Ok(None)          - not subscribed to this item (HTTP 404)
///   Err(...)          - unexpected error (bad key, network failure, etc.)
///
/// The caller is responsible for calling `ping_trud()` first if needed.
fn probe_edition(api_key: &str, item_id: u32) -> Result<Option<TrudRelease>> {
    let base = trud_api_base();
    let url = format!("{base}/keys/{api_key}/items/{item_id}/releases?latest");
    match ureq::get(&url).call() {
        Ok(resp) => {
            let body: TrudListResponse = resp
                .into_body()
                .read_json()
                .context("parsing TRUD API response")?;
            Ok(body.releases.into_iter().next())
        }
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(ureq::Error::StatusCode(400)) => Err(anyhow::anyhow!(
            "TRUD API key invalid (HTTP 400). Check your key at:\n  {TRUD_ACCOUNT_URL}"
        )),
        Err(ureq::Error::StatusCode(code)) => Err(anyhow::anyhow!("TRUD API returned HTTP {code}")),
        Err(e) => Err(anyhow::anyhow!(
            "TRUD API request failed: {}",
            redact_key(&e.to_string(), api_key)
        )),
    }
}

fn fetch_releases(api_key: &str, item_id: u32, latest_only: bool) -> Result<Vec<TrudRelease>> {
    ping_trud()?;
    let suffix = if latest_only { "?latest" } else { "" };
    let base = trud_api_base();
    let url = format!("{base}/keys/{api_key}/items/{item_id}/releases{suffix}");

    let resp = ureq::get(&url).call().map_err(|e| {
        if let ureq::Error::StatusCode(code) = e {
            match code {
                400 => anyhow::anyhow!(
                    "TRUD API key invalid (HTTP 400). Check your key at:\n  {TRUD_ACCOUNT_URL}"
                ),
                404 => anyhow::anyhow!(
                    "TRUD item {item_id} not found or your account is not subscribed to it \
                     (HTTP 404)."
                ),
                _ => anyhow::anyhow!("TRUD API returned HTTP {code}"),
            }
        } else {
            anyhow::anyhow!(
                "TRUD API request failed: {}",
                redact_key(&e.to_string(), api_key)
            )
        }
    })?;

    let body: TrudListResponse = resp
        .into_body()
        .read_json()
        .context("parsing TRUD API response")?;

    Ok(body.releases)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip the API key out of error text before it reaches stderr/CI logs.
///
/// `probe_edition` and `fetch_releases` build the request URL as
/// `{base}/keys/{api_key}/items/...`, so `ureq`'s transport-error `Display`
/// (DNS failure, TLS error, redirect loop, etc.) can embed the full URI,
/// key included. HTTP status errors are formatted separately and never carry
/// the URL, so this only needs to cover the generic fallthrough arms.
fn redact_key(text: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        return text.to_string();
    }
    text.replace(api_key, "<REDACTED>")
}

/// Uppercase hex, matching TRUD's `archiveFileSha256` casing. sha2 0.11's
/// `finalize()` output type dropped its `UpperHex`/`LowerHex` impls, so format
/// byte-by-byte rather than relying on the hasher's return type.
fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn sha256_of_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("opening {} for checksum verification", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).context("reading file for checksum")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_upper(&hasher.finalize()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::{EditionProfile, TrudConfig};
    use std::io::Write;
    use tempfile::NamedTempFile;

    // --- sct trud auth: config editing -------------------------------------

    /// The `[trud] api_key` value a config file parses to.
    fn parsed_key(text: &str) -> Option<String> {
        toml::from_str::<Config>(text)
            .expect("edited config must be valid TOML")
            .trud
            .and_then(|t| t.api_key)
    }

    #[test]
    fn auth_creates_trud_section_in_an_empty_config() {
        let edit = set_config_api_key("", "KEY123").unwrap();
        assert_eq!(parsed_key(&edit.text).as_deref(), Some("KEY123"));
        assert_eq!(edit.previous, None);
        assert!(edit.text.ends_with('\n'), "config must end with a newline");
    }

    #[test]
    fn auth_appends_trud_section_after_unrelated_sections() {
        let existing = "[format]\ndefault = \"json\"\n";
        let edit = set_config_api_key(existing, "KEY123").unwrap();
        assert_eq!(parsed_key(&edit.text).as_deref(), Some("KEY123"));
        assert!(
            edit.text.starts_with("[format]\ndefault = \"json\"\n"),
            "existing sections must be preserved verbatim, got:\n{}",
            edit.text
        );
    }

    #[test]
    fn auth_replaces_an_existing_key_and_reports_the_old_one() {
        let existing = "[trud]\napi_key = \"OLD\"\ndownload_dir = \"~/rel\"\n";
        let edit = set_config_api_key(existing, "NEW").unwrap();
        assert_eq!(edit.previous.as_deref(), Some("OLD"));
        assert_eq!(parsed_key(&edit.text).as_deref(), Some("NEW"));
        assert!(
            edit.text.contains("download_dir = \"~/rel\""),
            "sibling keys must survive, got:\n{}",
            edit.text
        );
        assert!(
            !edit.text.contains("OLD"),
            "the old key must not be left behind, got:\n{}",
            edit.text
        );
    }

    #[test]
    fn auth_preserves_comments_and_other_sections() {
        let existing = "\
# top comment
[paths]
db = \"~/snomed.db\"  # inline comment

[trud]
# where the key came from
api_key = \"OLD\"

[format]
default = \"json\"
";
        let edit = set_config_api_key(existing, "NEW").unwrap();
        for expected in [
            "# top comment",
            "db = \"~/snomed.db\"  # inline comment",
            "# where the key came from",
            "[format]",
            "default = \"json\"",
        ] {
            assert!(
                edit.text.contains(expected),
                "lost {expected:?} from:\n{}",
                edit.text
            );
        }
        assert_eq!(parsed_key(&edit.text).as_deref(), Some("NEW"));
    }

    #[test]
    fn auth_declares_trud_above_existing_subtables() {
        // Only [trud.editions.*] exists: the new [trud] header must go above it,
        // or the assignment would land inside the subtable.
        let existing = "[trud.editions.mine]\ntrud_item = 9876\n";
        let edit = set_config_api_key(existing, "KEY123").unwrap();
        assert_eq!(parsed_key(&edit.text).as_deref(), Some("KEY123"));
        let config: Config = toml::from_str(&edit.text).unwrap();
        let editions = config.trud.unwrap().editions.unwrap();
        assert_eq!(
            editions.get("mine").unwrap().trud_item,
            9876,
            "the existing edition profile must survive"
        );
    }

    #[test]
    fn auth_does_not_touch_a_key_in_a_later_section() {
        // An `api_key` under another section must not be mistaken for ours.
        let existing = "[trud]\ndownload_dir = \"~/rel\"\n\n[other]\napi_key = \"NOTOURS\"\n";
        let edit = set_config_api_key(existing, "KEY123").unwrap();
        assert_eq!(parsed_key(&edit.text).as_deref(), Some("KEY123"));
        assert!(
            edit.text.contains("api_key = \"NOTOURS\""),
            "the unrelated key must be untouched, got:\n{}",
            edit.text
        );
    }

    #[test]
    fn auth_rejects_a_config_that_is_not_valid_toml() {
        let Err(err) = set_config_api_key("this is not toml =\n", "KEY123") else {
            panic!("a config we cannot parse must not be rewritten");
        };
        assert!(
            err.to_string().contains("not valid TOML"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn auth_escapes_a_key_needing_toml_quoting() {
        // Not a realistic TRUD key, but the writer must never emit broken TOML.
        let key = "we\"ird\\key";
        let edit = set_config_api_key("", key).unwrap();
        assert_eq!(parsed_key(&edit.text).as_deref(), Some(key));
    }

    #[test]
    fn api_key_assignment_matches_only_the_real_thing() {
        assert!(is_api_key_assignment("api_key = \"x\""));
        assert!(is_api_key_assignment("  api_key=\"x\""));
        assert!(is_api_key_assignment("\"api_key\" = \"x\""));
        assert!(!is_api_key_assignment("api_key_file = \"x\""));
        assert!(!is_api_key_assignment("# api_key = \"x\""));
        assert!(!is_api_key_assignment("download_dir = \"x\""));
    }

    // --- sct trud auth: key handling ---------------------------------------

    #[test]
    fn auth_rejects_unusable_keys() {
        assert!(validate_key_shape(String::new()).is_err());
        assert!(validate_key_shape("API key: ABC".to_string()).is_err());
        assert!(validate_key_shape("ABC\u{7}DEF".to_string()).is_err());
        assert_eq!(validate_key_shape("ABC123".to_string()).unwrap(), "ABC123");
    }

    #[test]
    fn mask_key_shows_only_the_last_four_characters() {
        assert_eq!(mask_key("ABCDEFGH"), "****EFGH");
        assert_eq!(mask_key("ABCD"), "****");
        assert_eq!(mask_key("AB"), "**");
        assert_eq!(mask_key(""), "");
        // Multi-byte input must not panic on a char boundary.
        assert_eq!(mask_key("kéy-wxyz").chars().count(), 8);
    }

    #[test]
    fn auth_config_path_prefers_the_flag() {
        let flag = PathBuf::from("/tmp/explicit.toml");
        assert_eq!(auth_config_path(Some(&flag)), flag);
    }

    fn release_with_filename(name: &str) -> serde_json::Result<TrudRelease> {
        serde_json::from_value(serde_json::json!({
            "archiveFileUrl": "https://example.test/release.zip",
            "archiveFileName": name,
            "archiveFileSizeBytes": 1,
            "archiveFileSha256": "00",
            "releaseDate": "2026-01-01"
        }))
    }

    #[test]
    fn archive_filename_must_be_a_plain_filename() {
        assert_eq!(
            release_with_filename("release.zip")
                .unwrap()
                .archive_file_name,
            "release.zip"
        );
        for unsafe_name in [
            "",
            ".",
            "..",
            "../escape.zip",
            "dir/file.zip",
            "dir\\file.zip",
        ] {
            assert!(
                release_with_filename(unsafe_name).is_err(),
                "accepted unsafe filename {unsafe_name:?}"
            );
        }
    }

    // --- expand_tilde ----------------------------------------------------------

    #[test]
    fn expand_tilde_no_tilde_is_unchanged() {
        // Safe to keep standalone - does not touch the process environment.
        assert_eq!(
            paths::expand_tilde("/absolute/path"),
            PathBuf::from("/absolute/path")
        );
        assert_eq!(
            paths::expand_tilde("relative/path"),
            PathBuf::from("relative/path")
        );
    }

    // The `expand_tilde_expands_home` case was folded into
    // `env_directory_resolution_smoke` below - it mutates HOME and races
    // with the data_home/data_dir tests under parallel `cargo test`.

    // --- resolve_api_key -------------------------------------------------------

    #[test]
    fn api_key_flag_wins_over_everything() {
        let config = Config::default();
        let key = resolve_api_key(Some("flag-key"), None, &config).unwrap();
        assert_eq!(key, "flag-key");
    }

    #[test]
    fn api_key_flag_is_trimmed() {
        let config = Config::default();
        let key = resolve_api_key(Some("  trimmed  "), None, &config).unwrap();
        assert_eq!(key, "trimmed");
    }

    #[test]
    fn api_key_from_file_first_line() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "file-key   ").unwrap(); // trailing whitespace - must be trimmed
        writeln!(f, "second-line-is-ignored").unwrap();
        let config = Config::default();
        let key = resolve_api_key(None, Some(f.path()), &config).unwrap();
        assert_eq!(key, "file-key");
    }

    #[test]
    fn api_key_file_empty_is_error() {
        let f = NamedTempFile::new().unwrap();
        let config = Config::default();
        let result = resolve_api_key(None, Some(f.path()), &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn api_key_from_config_file() {
        // Only meaningful when TRUD_API_KEY is not set in the environment.
        // The env var has higher precedence and would shadow the config value.
        if std::env::var("TRUD_API_KEY").is_ok() {
            return;
        }
        let config = Config {
            trud: Some(TrudConfig {
                api_key: Some("config-key".into()),
                ..TrudConfig::default()
            }),
            ..Config::default()
        };
        let key = resolve_api_key(None, None, &config).unwrap();
        assert_eq!(key, "config-key");
    }

    #[test]
    fn api_key_missing_from_all_sources_gives_helpful_error() {
        if std::env::var("TRUD_API_KEY").is_ok() {
            return; // env var present; test not applicable
        }
        let config = Config::default();
        let err = resolve_api_key(None, None, &config).unwrap_err();
        let msg = err.to_string();
        // Error message should mention all four supply methods
        assert!(msg.contains("--api-key"));
        assert!(msg.contains("--api-key-file"));
        assert!(msg.contains("TRUD_API_KEY"));
        assert!(msg.contains("config.toml"));
        // And point to the TRUD account page
        assert!(msg.contains("isd.digital.nhs.uk"));
    }

    // --- resolve_item_id -------------------------------------------------------

    #[test]
    fn item_flag_overrides_edition() {
        let config = Config::default();
        assert_eq!(
            resolve_item_id(Some(9999), "uk_monolith", &config).unwrap(),
            9999
        );
    }

    #[test]
    fn builtin_edition_monolith() {
        let config = Config::default();
        assert_eq!(resolve_item_id(None, "uk_monolith", &config).unwrap(), 1799);
    }

    #[test]
    fn builtin_edition_clinical() {
        let config = Config::default();
        assert_eq!(resolve_item_id(None, "uk_clinical", &config).unwrap(), 101);
    }

    #[test]
    fn builtin_edition_drug() {
        let config = Config::default();
        assert_eq!(resolve_item_id(None, "uk_drug", &config).unwrap(), 105);
    }

    #[test]
    fn builtin_edition_nhs_data_migration() {
        let config = Config::default();
        assert_eq!(
            resolve_item_id(None, "nhs_data_migration", &config).unwrap(),
            9
        );
    }

    #[test]
    fn config_edition_overrides_builtin() {
        let mut editions = HashMap::new();
        editions.insert(
            "uk_monolith".to_string(),
            EditionProfile { trud_item: 9876 },
        );
        let config = Config {
            trud: Some(TrudConfig {
                editions: Some(editions),
                ..TrudConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(resolve_item_id(None, "uk_monolith", &config).unwrap(), 9876);
    }

    #[test]
    fn config_custom_edition() {
        let mut editions = HashMap::new();
        editions.insert("my_custom".to_string(), EditionProfile { trud_item: 42 });
        let config = Config {
            trud: Some(TrudConfig {
                editions: Some(editions),
                ..TrudConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(resolve_item_id(None, "my_custom", &config).unwrap(), 42);
    }

    #[test]
    fn unknown_edition_error_names_the_edition() {
        let config = Config::default();
        let err = resolve_item_id(None, "made_up_edition", &config).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("made_up_edition"));
        // Should also list the known built-in names
        assert!(msg.contains("uk_monolith"));
    }

    // --- sha256_of_file --------------------------------------------------------

    #[test]
    fn sha256_empty_file() {
        let f = NamedTempFile::new().unwrap();
        let hash = sha256_of_file(f.path()).unwrap();
        // SHA-256 of the empty string is well-known
        assert_eq!(
            hash.to_lowercase(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_is_uppercase_hex() {
        let f = NamedTempFile::new().unwrap();
        let hash = sha256_of_file(f.path()).unwrap();
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(hash.chars().all(|c| !c.is_ascii_lowercase()));
    }

    #[test]
    fn sha256_consistent_across_calls() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"deterministic test content").unwrap();
        let h1 = sha256_of_file(f.path()).unwrap();
        let h2 = sha256_of_file(f.path()).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn sha256_differs_for_different_content() {
        let mut f1 = NamedTempFile::new().unwrap();
        let mut f2 = NamedTempFile::new().unwrap();
        f1.write_all(b"content A").unwrap();
        f2.write_all(b"content B").unwrap();
        assert_ne!(
            sha256_of_file(f1.path()).unwrap(),
            sha256_of_file(f2.path()).unwrap()
        );
    }

    // --- directory resolution ---------------------------------------------------
    //
    // Every test below mutates HOME / SCT_DATA_HOME / XDG_DATA_HOME and reads
    // them back through `paths::data_home()` etc. `cargo test` runs `#[test]`
    // functions in parallel and provides no per-test environment isolation,
    // so running each case as its own `#[test]` races (one test's
    // `remove_var` can be observed by another, or vice versa). The roadmap
    // entry "De-flake trud tests' environment variables" calls this out;
    // `paths::tests` uses the same consolidation pattern.
    //
    // The whole block runs sequentially within one `#[test]` and saves/
    // restores the outer environment so it does not leak into sibling tests
    // that read these vars unintentionally.

    #[test]
    fn env_directory_resolution_smoke() {
        // Serialise against sibling env-touching tests in other modules
        // (paths::tests::env_and_cwd_chain_smoke).
        let _guard = crate::paths::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let saved_home = std::env::var_os("HOME");
        let saved_sct = std::env::var_os("SCT_DATA_HOME");
        let saved_xdg = std::env::var_os("XDG_DATA_HOME");

        // --- expand_tilde expands ~/ relative to HOME ---
        unsafe { std::env::set_var("HOME", "/users/test") };
        assert_eq!(
            paths::expand_tilde("~/foo/bar"),
            PathBuf::from("/users/test/foo/bar")
        );

        // --- data_home defaults under HOME ---
        unsafe {
            std::env::remove_var("SCT_DATA_HOME");
            std::env::remove_var("XDG_DATA_HOME");
            std::env::set_var("HOME", "/users/test");
        };
        assert_eq!(
            paths::data_home(),
            PathBuf::from("/users/test/.local/share/sct")
        );

        // --- SCT_DATA_HOME overrides everything ---
        unsafe { std::env::set_var("SCT_DATA_HOME", "/custom/sct") };
        assert_eq!(paths::data_home(), PathBuf::from("/custom/sct"));

        // --- SCT_DATA_HOME expands ~/ ---
        unsafe {
            std::env::set_var("SCT_DATA_HOME", "~/my-sct");
            std::env::set_var("HOME", "/users/test");
        };
        assert_eq!(paths::data_home(), PathBuf::from("/users/test/my-sct"));

        // --- releases_dir defaults under data_home/releases ---
        unsafe {
            std::env::remove_var("SCT_DATA_HOME");
            std::env::set_var("HOME", "/users/test");
        };
        let cfg = Config::default();
        assert_eq!(
            resolve_releases_dir(None, &cfg),
            PathBuf::from("/users/test/.local/share/sct").join(RELEASES_SUBDIR)
        );

        // --- data_dir defaults under data_home/data ---
        assert_eq!(
            resolve_data_dir(None, &cfg),
            PathBuf::from("/users/test/.local/share/sct").join(DATA_SUBDIR)
        );

        // --- releases_dir and data_dir are distinct ---
        assert_ne!(
            resolve_releases_dir(None, &cfg),
            resolve_data_dir(None, &cfg)
        );

        // --- restore the outer environment ---
        unsafe {
            match saved_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match saved_sct {
                Some(v) => std::env::set_var("SCT_DATA_HOME", v),
                None => std::env::remove_var("SCT_DATA_HOME"),
            }
            match saved_xdg {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
        }
    }

    #[test]
    fn flag_overrides_default_releases_dir() {
        let config = Config::default();
        let dir = resolve_releases_dir(Some(Path::new("/explicit/releases")), &config);
        assert_eq!(dir, PathBuf::from("/explicit/releases"));
    }

    #[test]
    fn flag_overrides_default_data_dir() {
        let config = Config::default();
        let dir = resolve_data_dir(Some(Path::new("/explicit/data")), &config);
        assert_eq!(dir, PathBuf::from("/explicit/data"));
    }

    #[test]
    fn config_download_dir_overrides_default_releases_dir() {
        let config = Config {
            trud: Some(TrudConfig {
                download_dir: Some("/config/releases".into()),
                ..TrudConfig::default()
            }),
            ..Config::default()
        };
        let dir = resolve_releases_dir(None, &config);
        assert_eq!(dir, PathBuf::from("/config/releases"));
    }

    #[test]
    fn config_data_dir_overrides_default_data_dir() {
        let config = Config {
            trud: Some(TrudConfig {
                data_dir: Some("/config/data".into()),
                ..TrudConfig::default()
            }),
            ..Config::default()
        };
        let dir = resolve_data_dir(None, &config);
        assert_eq!(dir, PathBuf::from("/config/data"));
    }

    #[test]
    fn flag_wins_over_config_releases_dir() {
        let config = Config {
            trud: Some(TrudConfig {
                download_dir: Some("/config/releases".into()),
                ..TrudConfig::default()
            }),
            ..Config::default()
        };
        let dir = resolve_releases_dir(Some(Path::new("/flag/releases")), &config);
        assert_eq!(dir, PathBuf::from("/flag/releases"));
    }

    #[test]
    fn config_parses_data_dir() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[trud]").unwrap();
        writeln!(f, r#"data_dir = "/my/data""#).unwrap();
        let config = paths::load_config_from(f.path());
        assert_eq!(config.trud.unwrap().data_dir.unwrap(), "/my/data");
    }

    // --- redact_key --------------------------------------------------------

    #[test]
    fn redact_key_strips_key_from_url_in_error_text() {
        let text = "TRUD download request failed: error sending request for url \
                     (https://isd.digital.nhs.uk/trud/api/v1/keys/SECRET123/items/1799/releases)";
        let redacted = redact_key(text, "SECRET123");
        assert!(!redacted.contains("SECRET123"));
        assert!(redacted.contains("<REDACTED>"));
    }

    #[test]
    fn redact_key_leaves_text_without_key_unchanged() {
        let text = "TRUD API returned HTTP 500";
        assert_eq!(redact_key(text, "SECRET123"), text);
    }

    #[test]
    fn redact_key_with_empty_key_is_noop() {
        let text = "some error containing SECRET123";
        assert_eq!(redact_key(text, ""), text);
    }

    // --- ping_trud (offline/logic tests only) ----------------------------------
    //
    // We cannot test actual network reachability in unit tests. We test the
    // error classification logic by checking that the two "connected" arms
    // (Ok and StatusCode) are treated identically, and that the error message
    // produced for a connection failure contains the key user-facing strings.

    #[test]
    fn ping_trud_error_message_contains_maintenance_window_hint() {
        let fake_io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let msg = unreachable_message(TRUD_HEALTH_URL, &fake_io_err.to_string(), true);

        assert!(msg.contains("maintenance"));
        assert!(msg.contains(TRUD_HEALTH_URL));
        assert!(msg.contains("refused"), "the original error must survive");

        // The windows TRUD actually publishes, as the times to *run* automation.
        assert!(msg.contains("08:00-18:00"));
        assert!(msg.contains("00:00-06:00"));
        // TRUD publishes no downtime window; we must not invent one. In
        // particular midnight-06:00 is a recommended window, not maintenance.
        assert!(
            !msg.contains("18:00-08:00") && !msg.contains("18:00–08:00"),
            "must not assert a downtime window TRUD does not publish: {msg}"
        );
    }

    #[test]
    fn dns_hint_fires_only_on_a_lookup_failure_with_no_resolver_config() {
        // The Android/Termux case: static build, no /etc/resolv.conf.
        let android = "io: failed to lookup address information: Try again";
        assert!(dns_hint(android, false).is_some());

        // Same box, but the resolver is configured - an ordinary DNS outage,
        // and the Android advice would be a red herring.
        assert!(dns_hint(android, true).is_none());

        // A connection failure is not a lookup failure, whatever the resolver
        // situation: the name resolved fine and the connection was refused.
        let refused = "io: Connection refused";
        assert!(dns_hint(refused, false).is_none());
        assert!(dns_hint(refused, true).is_none());
    }

    #[test]
    fn unreachable_message_carries_the_android_hint_when_it_applies() {
        let android = "io: failed to lookup address information: Try again";

        let with_hint = unreachable_message(TRUD_HEALTH_URL, android, false);
        assert!(with_hint.contains("/etc/resolv.conf"));
        assert!(with_hint.contains("android-termux"));
        assert!(with_hint.contains(android), "original error must survive");

        let without_hint = unreachable_message(TRUD_HEALTH_URL, android, true);
        assert!(!without_hint.contains("android-termux"));
    }

    #[test]
    fn ping_trud_health_url_is_on_expected_domain() {
        // Sanity-check the constant hasn't drifted to an unexpected host.
        assert!(TRUD_HEALTH_URL.starts_with("https://isd.digital.nhs.uk/"));
    }

    // --- paths::load_config_from ----------------------------------------------

    #[test]
    fn config_missing_file_returns_default() {
        let tmp = PathBuf::from("/tmp/sct-test-nonexistent-config-file.toml");
        let config = paths::load_config_from(&tmp);
        assert!(config.trud.is_none());
    }

    #[test]
    fn config_parses_api_key() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[trud]").unwrap();
        writeln!(f, r#"api_key = "parsed-key""#).unwrap();
        let config = paths::load_config_from(f.path());
        assert_eq!(config.trud.unwrap().api_key.unwrap(), "parsed-key");
    }

    #[test]
    fn config_parses_custom_edition() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[trud.editions.my_org]").unwrap();
        writeln!(f, "trud_item = 777").unwrap();
        let config = paths::load_config_from(f.path());
        let trud = config.trud.unwrap();
        let editions = trud.editions.unwrap();
        assert_eq!(editions["my_org"].trud_item, 777);
    }

    #[test]
    fn config_invalid_toml_returns_default() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "this is not valid toml {{!!").unwrap();
        let config = paths::load_config_from(f.path());
        // Should not panic; silently returns default
        assert!(config.trud.is_none());
    }

    // --- pipeline flag wiring (download → ndjson) ------------------------------
    //
    // `run_pipeline_if_requested` is the chaining step behind --pipeline. This
    // test confirms the ndjson-shaping flags (--include-inactive here) actually
    // reach the `sct ndjson` step, by running the pipeline over the committed
    // synthetic RF2 fixture (a directory, so no zip extraction needed) and
    // inspecting the NDJSON it writes. Regression guard: the pipeline used to
    // hard-code include_inactive = false, so the flag could never take effect.
    #[test]
    fn pipeline_passes_include_inactive_to_ndjson() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z");

        // Run only the pipeline step against the fixture, returning the main
        // NDJSON artefact's contents. `--refsets all` is used so the test also
        // exercises that flag flowing through (it writes a history sidecar too,
        // which is why we read the exact main path rather than the first match).
        let run_with = |include_inactive: bool| -> String {
            let data_dir = tempfile::tempdir().unwrap();
            let args = DownloadArgs {
                edition: "uk_monolith".into(),
                item: None,
                release: None,
                output_dir: None,
                data_dir: None,
                skip_if_current: false,
                pipeline: true,
                pipeline_full: false,
                multi_terminology: false,
                with_read2: false,
                locale: "en-GB".into(),
                include_inactive,
                refsets: crate::commands::ndjson::RefsetMode::All,
                key: KeyArgs {
                    api_key: None,
                    api_key_file: None,
                },
            };
            // Artefacts are named with the same slug `sct ndjson` would pick
            // for this input, not a bare lowercased stem.
            let stem = crate::commands::ndjson::slugify_path(&fixture);
            let db_path = run_pipeline_if_requested(&args, &fixture, data_dir.path())
                .unwrap()
                .expect("pipeline should return generated database path");
            assert_eq!(db_path, data_dir.path().join(format!("{stem}.db")));
            std::fs::read_to_string(data_dir.path().join(format!("{stem}.ndjson"))).unwrap()
        };

        // Default (flag off): the inactive concept 9468002 is absent.
        let off = run_with(false);
        assert!(
            !off.lines().any(|l| l.contains("\"id\":\"9468002\"")),
            "inactive concept must not appear without --include-inactive"
        );

        // Flag on: 9468002 appears, marked inactive.
        let on = run_with(true);
        assert!(
            on.lines()
                .any(|l| l.contains("\"id\":\"9468002\"") && l.contains("\"active\":false")),
            "inactive concept must appear (active:false) with --include-inactive"
        );
    }
}
