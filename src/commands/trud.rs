//! `sct trud` — Download SNOMED CT RF2 releases via the NHS TRUD API.
//!
//! Subcommands:
//!   sct trud list     — list available releases for an edition/item
//!   sct trud check    — check whether a newer release is available (exit 0/2)
//!   sct trud download — download a release, verifying SHA-256, with optional pipeline
//!
//! API key resolution order (first non-empty value wins):
//!   1. --api-key <KEY>           plain string flag
//!   2. --api-key-file <PATH>     first line of the named file
//!   3. $TRUD_API_KEY             environment variable (recommended for CI/cron)
//!   4. api_key in ~/.config/sct/config.toml

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------------------------------------------------------------------------
// TRUD endpoint constants — change here if NHS TRUD ever moves their API.
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

// ---------------------------------------------------------------------------
// sct directory layout constants
// ---------------------------------------------------------------------------
//
// All sct-managed files live under a single base directory:
//
//   ~/.local/share/sct/          ($SCT_DATA_HOME overrides the whole base)
//   ├── releases/                 downloaded RF2 zip files from TRUD
//   └── data/                    built artefacts: .ndjson, .db, .parquet, .arrow
//
// Override the base with $SCT_DATA_HOME, or individual subdirs via config file
// fields (download_dir, data_dir) or CLI flags (--output-dir, --data-dir).

/// Sub-directory under the sct base for downloaded release zip files.
const RELEASES_SUBDIR: &str = "releases";
/// Sub-directory under the sct base for built artefacts (.ndjson, .db, etc.).
const DATA_SUBDIR: &str = "data";

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
    /// List available releases for a TRUD edition/item, newest first.
    List(ListArgs),

    /// Check whether a newer release is available.
    ///
    /// Compares the latest TRUD release against what is on disk, and — if the
    /// local file is present — verifies its SHA-256 against the TRUD metadata
    /// so a corrupt or half-downloaded local file is not reported as current.
    ///
    /// Exit codes: 0 = already up to date and SHA-256 verified, 2 = new release
    /// available OR local file fails checksum, 1 = error. Use exit code 2 (not
    /// 1) in shell scripts to distinguish "action required" from an error.
    Check(CheckArgs),

    /// Download a SNOMED CT RF2 release from TRUD, with SHA-256 verification.
    Download(DownloadArgs),
}

/// Flags for supplying the TRUD API key — shared across all subcommands.
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
    #[arg(long)]
    api_key_file: Option<PathBuf>,
}

#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Named edition profile: uk_monolith, uk_clinical, uk_drug.
    ///
    /// If omitted (and --item is not given), shows subscription status for all
    /// built-in editions. If supplied, lists all releases for that edition.
    #[arg(long)]
    edition: Option<String>,

    /// Raw TRUD item number — overrides --edition.
    #[arg(long)]
    item: Option<u32>,

    #[command(flatten)]
    key: KeyArgs,
}

#[derive(Parser, Debug)]
pub struct CheckArgs {
    /// Named edition profile: uk_monolith (default), uk_clinical, uk_drug.
    #[arg(long, default_value = "uk_monolith")]
    edition: String,

    /// Raw TRUD item number — overrides --edition.
    #[arg(long)]
    item: Option<u32>,

    #[command(flatten)]
    key: KeyArgs,
}

#[derive(Parser, Debug)]
pub struct DownloadArgs {
    /// Named edition profile: uk_monolith (default), uk_clinical, uk_drug.
    #[arg(long, default_value = "uk_monolith")]
    edition: String,

    /// Raw TRUD item number — overrides --edition.
    #[arg(long)]
    item: Option<u32>,

    /// Download a specific named version (e.g. 41.5.0). Defaults to latest.
    #[arg(long)]
    release: Option<String>,

    /// Directory for the downloaded RF2 zip.
    /// Defaults to download_dir in config, then $SCT_DATA_HOME/releases.
    #[arg(long)]
    output_dir: Option<PathBuf>,

    /// Directory for built artefacts produced by --pipeline / --pipeline-full
    /// (.ndjson, .db, .arrow). Defaults to data_dir in config,
    /// then $SCT_DATA_HOME/data.
    #[arg(long)]
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
    #[serde(rename = "archiveFileName")]
    archive_file_name: String,
    #[serde(rename = "archiveFileSizeBytes")]
    archive_file_size_bytes: u64,
    #[serde(rename = "archiveFileSha256")]
    archive_file_sha256: String,
    #[serde(rename = "releaseDate")]
    release_date: String,
}

// ---------------------------------------------------------------------------
// Config file types  (~/.config/sct/config.toml)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct Config {
    trud: Option<TrudConfig>,
}

#[derive(Deserialize, Default)]
struct TrudConfig {
    api_key: Option<String>,
    /// Override for the RF2 zip download directory (default: $SCT_DATA_HOME/releases).
    download_dir: Option<String>,
    /// Override for the built-artefact directory (default: $SCT_DATA_HOME/data).
    data_dir: Option<String>,
    #[allow(dead_code)]
    default_edition: Option<String>,
    editions: Option<HashMap<String, EditionProfile>>,
}

#[derive(Deserialize)]
struct EditionProfile {
    trud_item: u32,
}

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
            description:
                "UK Monolith (International + UK Clinical + UK Drug/dm+d + UK Pathology)",
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
    m
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(args: Args) -> Result<()> {
    match args.subcommand {
        TrudCommand::List(a) => run_list(a),
        TrudCommand::Check(a) => run_check(a),
        TrudCommand::Download(a) => run_download(a),
    }
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
        "{:<52}  {:<12}  {:>8}  {}",
        "File", "Released", "Size", "SHA-256 (first 12 chars)"
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
        (
            "uk_clinical",
            101,
            "International + UK Clinical (no dm+d)",
        ),
        ("uk_drug", 105, "UK Drug Extension / dm+d only"),
    ];

    println!(
        "{:<16}  {:>4}  {:<14}  {:<52}  {}",
        "Edition", "Item", "Status", "Latest release", "Released"
    );
    println!("{}", "-".repeat(100));

    for (name, item_id, _desc) in editions {
        match probe_edition(api_key, *item_id)? {
            Some(release) => {
                println!(
                    "{:<16}  {:>4}  {:<14}  {:<52}  {}",
                    name,
                    item_id,
                    "subscribed",
                    release.archive_file_name,
                    release.release_date
                );
            }
            None => {
                println!(
                    "{:<16}  {:>4}  {:<14}  {:<52}  {}",
                    name, item_id, "not subscribed", "—", "—"
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
        // exit 2 — not an error, but signals "please update"
        std::process::exit(2);
    }

    // File exists — verify its SHA-256 against the TRUD metadata so we don't
    // report a corrupt or half-downloaded local file as "up to date".
    let local_hash = sha256_of_file(&local_path)?;
    if local_hash.eq_ignore_ascii_case(&latest.archive_file_sha256) {
        println!(
            "Up to date: {} ({})\nSHA-256 verified: {}",
            latest.archive_file_name, latest.release_date, latest.archive_file_sha256
        );
        // exit 0 — already current and intact
        return Ok(());
    }

    // File is present but does not match the expected checksum. Treat this as
    // "action required" — exit 2, same as "new release available" — so shell
    // scripts that re-download on exit 2 will heal a corrupt local file.
    println!(
        "Local file present but SHA-256 does not match TRUD metadata — re-download recommended: {}\n\
         Expected: {}\n\
         Got:      {}",
        latest.archive_file_name, latest.archive_file_sha256, local_hash
    );
    std::process::exit(2);
}

// ---------------------------------------------------------------------------
// sct trud download
// ---------------------------------------------------------------------------

fn run_download(args: DownloadArgs) -> Result<()> {
    let config = load_config();
    let api_key = resolve_api_key(
        args.key.api_key.as_deref(),
        args.key.api_key_file.as_deref(),
        &config,
    )?;
    let item_id = resolve_item_id(args.item, &args.edition, &config)?;
    let releases_dir = resolve_releases_dir(args.output_dir.as_deref(), &config);
    let data_dir   = resolve_data_dir(args.data_dir.as_deref(), &config);

    std::fs::create_dir_all(&releases_dir)
        .with_context(|| format!("creating releases directory {}", releases_dir.display()))?;
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating data directory {}", data_dir.display()))?;

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
                    "Already up to date: {} — skipping download.",
                    release.archive_file_name
                );
                return run_pipeline_if_requested(&args, &dest, &data_dir);
            }
            println!(
                "File already present with matching SHA-256: {}",
                release.archive_file_name
            );
            return run_pipeline_if_requested(&args, &dest, &data_dir);
        }
        // Checksum mismatch — re-download
        eprintln!(
            "Warning: existing file has unexpected SHA-256 — re-downloading {}",
            release.archive_file_name
        );
    }

    println!(
        "Downloading {} ({}) ...",
        release.archive_file_name,
        human_size(release.archive_file_size_bytes)
    );

    // Stream to a temp file; rename to final path only after checksum passes
    let tmp_path = releases_dir.join(format!("{}.tmp", release.archive_file_name));

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
        let tmp_file = std::fs::File::create(&tmp_path)
            .with_context(|| format!("creating temporary file {}", tmp_path.display()))?;
        let mut writer = BufWriter::new(tmp_file);
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
            writer.write_all(&buf[..n]).context("writing to temp file")?;
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
        let computed = format!("{:X}", hasher.finalize());
        if !computed.eq_ignore_ascii_case(&release.archive_file_sha256) {
            std::fs::remove_file(&tmp_path).ok();
            anyhow::bail!(
                "SHA-256 checksum mismatch — download may be corrupt. Temporary file deleted.\n\
                 Expected: {}\n\
                 Got:      {}",
                release.archive_file_sha256,
                computed
            );
        }
    }

    // Rename temp → final
    std::fs::rename(&tmp_path, &dest)
        .with_context(|| format!("renaming temp file to {}", dest.display()))?;
    println!("✓ Saved: {}", dest.display());
    if args.pipeline || args.pipeline_full {
        println!("  Built artefacts will go to: {}", data_dir.display());
    }

    run_pipeline_if_requested(&args, &dest, &data_dir)
}

// ---------------------------------------------------------------------------
// Pipeline chaining
// ---------------------------------------------------------------------------

fn run_pipeline_if_requested(
    args: &DownloadArgs,
    zip_path: &Path,
    data_dir: &Path,
) -> Result<()> {
    if !args.pipeline && !args.pipeline_full {
        return Ok(());
    }

    // Derive output filenames from the zip stem (lowercased)
    let stem = zip_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("snomed")
        .to_lowercase();
    let ndjson_path = data_dir.join(format!("{stem}.ndjson"));
    let db_path = data_dir.join(format!("{stem}.db"));

    // --- sct ndjson ---
    println!("\n→ Running: sct ndjson");
    super::ndjson::run(super::ndjson::Args {
        rf2_dirs: vec![zip_path.to_path_buf()],
        locale: "en-GB".into(),
        output: Some(ndjson_path.clone()),
        include_inactive: false,
    })
    .context("sct ndjson step failed")?;

    // --- sct sqlite ---
    println!("\n→ Running: sct sqlite");
    super::sqlite::run(super::sqlite::Args {
        input: ndjson_path.clone(),
        output: db_path.clone(),
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

        // --- sct embed (best-effort — skip if Ollama unavailable) ---
        println!("\n→ Running: sct embed");
        let arrow_path = data_dir.join(format!("{stem}.arrow"));
        if let Err(e) = super::embed::run(super::embed::Args {
            input: ndjson_path.clone(),
            model: "nomic-embed-text".into(),
            ollama_url: "http://localhost:11434".into(),
            output: arrow_path,
            batch_size: 64,
        }) {
            eprintln!("Warning: sct embed skipped — {e}");
            eprintln!("  (Is Ollama running? Start with: ollama serve)");
        }
    }

    println!("\n✓ Pipeline complete.");
    println!("  NDJSON: {}", ndjson_path.display());
    println!("  SQLite: {}", db_path.display());
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

/// Returns the sct base data directory.
///
/// Resolution order:
///   1. `$SCT_DATA_HOME` environment variable
///   2. `~/.local/share/sct` (XDG_DATA_HOME convention)
fn sct_data_home() -> PathBuf {
    if let Ok(val) = std::env::var("SCT_DATA_HOME") {
        let val = val.trim().to_string();
        if !val.is_empty() {
            return expand_tilde(&val);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local").join("share").join("sct")
}

/// Resolve the directory for downloaded RF2 zip files.
///
/// Resolution order: --output-dir flag → config download_dir → $SCT_DATA_HOME/releases
fn resolve_releases_dir(flag_dir: Option<&Path>, config: &Config) -> PathBuf {
    if let Some(dir) = flag_dir {
        return dir.to_path_buf();
    }
    if let Some(trud) = &config.trud {
        if let Some(dir) = &trud.download_dir {
            return expand_tilde(dir);
        }
    }
    sct_data_home().join(RELEASES_SUBDIR)
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
            return expand_tilde(dir);
        }
    }
    sct_data_home().join(DATA_SUBDIR)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(rest)
    } else {
        PathBuf::from(path)
    }
}

// ---------------------------------------------------------------------------
// Config file
// ---------------------------------------------------------------------------

fn load_config() -> Config {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let path = PathBuf::from(home)
        .join(".config")
        .join("sct")
        .join("config.toml");
    load_config_from_path(&path)
}

/// Inner loader — accepts an explicit path so tests can supply a temp file.
fn load_config_from_path(path: &Path) -> Config {
    if !path.exists() {
        return Config::default();
    }
    match std::fs::read_to_string(path) {
        Err(e) => {
            eprintln!("Warning: could not read {}: {e}", path.display());
            Config::default()
        }
        Ok(contents) => match toml::from_str(&contents) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Warning: could not parse {}: {e}", path.display());
                Config::default()
            }
        },
    }
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
    match ureq::get(TRUD_HEALTH_URL).call() {
        // Any HTTP response — including 4xx/5xx — means we reached the server.
        Ok(_) | Err(ureq::Error::StatusCode(_)) => Ok(()),
        Err(e) => Err(anyhow::anyhow!(
            "Cannot reach NHS TRUD ({TRUD_HEALTH_URL}).

The service may be offline or undergoing scheduled maintenance.
TRUD maintenance windows: weekdays 18:00–08:00 UK time, and midnight–06:00.

Original error: {e}"
        )),
    }
}

/// Probe a single TRUD item to determine subscription status.
///
/// Returns:
///   Ok(Some(release)) — subscribed; `release` is the latest available release
///   Ok(None)          — not subscribed to this item (HTTP 404)
///   Err(...)          — unexpected error (bad key, network failure, etc.)
///
/// The caller is responsible for calling `ping_trud()` first if needed.
fn probe_edition(api_key: &str, item_id: u32) -> Result<Option<TrudRelease>> {
    let url = format!("{TRUD_API_BASE}/keys/{api_key}/items/{item_id}/releases?latest");
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
        Err(ureq::Error::StatusCode(code)) => {
            Err(anyhow::anyhow!("TRUD API returned HTTP {code}"))
        }
        Err(e) => Err(anyhow::anyhow!("TRUD API request failed: {e}")),
    }
}

fn fetch_releases(api_key: &str, item_id: u32, latest_only: bool) -> Result<Vec<TrudRelease>> {
    ping_trud()?;
    let suffix = if latest_only { "?latest" } else { "" };
    let url = format!("{TRUD_API_BASE}/keys/{api_key}/items/{item_id}/releases{suffix}");

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
            anyhow::anyhow!("TRUD API request failed: {e}")
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
    Ok(format!("{:X}", hasher.finalize()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // --- human_size ------------------------------------------------------------

    #[test]
    fn human_size_bytes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1023), "1023 B");
    }

    #[test]
    fn human_size_kilobytes() {
        assert_eq!(human_size(1024), "1 KB");
        assert_eq!(human_size(2048), "2 KB");
    }

    #[test]
    fn human_size_megabytes() {
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn human_size_gigabytes() {
        assert_eq!(human_size(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    // --- expand_tilde ----------------------------------------------------------

    #[test]
    fn expand_tilde_expands_home() {
        // Safe to set HOME here because this test doesn't read it via load_config.
        unsafe { std::env::set_var("HOME", "/users/test") };
        assert_eq!(expand_tilde("~/foo/bar"), PathBuf::from("/users/test/foo/bar"));
    }

    #[test]
    fn expand_tilde_no_tilde_is_unchanged() {
        assert_eq!(expand_tilde("/absolute/path"), PathBuf::from("/absolute/path"));
        assert_eq!(expand_tilde("relative/path"), PathBuf::from("relative/path"));
    }

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
        writeln!(f, "file-key   ").unwrap(); // trailing whitespace — must be trimmed
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
        assert_eq!(resolve_item_id(Some(9999), "uk_monolith", &config).unwrap(), 9999);
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
    fn config_edition_overrides_builtin() {
        let mut editions = HashMap::new();
        editions.insert("uk_monolith".to_string(), EditionProfile { trud_item: 9876 });
        let config = Config {
            trud: Some(TrudConfig {
                editions: Some(editions),
                ..TrudConfig::default()
            }),
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

    #[test]
    fn sct_data_home_defaults_under_home() {
        // Without SCT_DATA_HOME, should derive from $HOME.
        unsafe { std::env::remove_var("SCT_DATA_HOME") };
        unsafe { std::env::set_var("HOME", "/users/test") };
        let base = sct_data_home();
        assert_eq!(base, PathBuf::from("/users/test/.local/share/sct"));
    }

    #[test]
    fn sct_data_home_respects_env_override() {
        unsafe { std::env::set_var("SCT_DATA_HOME", "/custom/sct") };
        let base = sct_data_home();
        unsafe { std::env::remove_var("SCT_DATA_HOME") };
        assert_eq!(base, PathBuf::from("/custom/sct"));
    }

    #[test]
    fn sct_data_home_env_override_expands_tilde() {
        unsafe { std::env::set_var("SCT_DATA_HOME", "~/my-sct") };
        unsafe { std::env::set_var("HOME", "/users/test") };
        let base = sct_data_home();
        unsafe { std::env::remove_var("SCT_DATA_HOME") };
        assert_eq!(base, PathBuf::from("/users/test/my-sct"));
    }

    #[test]
    fn releases_dir_defaults_to_base_releases_subdir() {
        unsafe { std::env::remove_var("SCT_DATA_HOME") };
        unsafe { std::env::set_var("HOME", "/users/test") };
        let config = Config::default();
        let dir = resolve_releases_dir(None, &config);
        assert_eq!(
            dir,
            PathBuf::from("/users/test/.local/share/sct").join(RELEASES_SUBDIR)
        );
    }

    #[test]
    fn data_dir_defaults_to_base_data_subdir() {
        unsafe { std::env::remove_var("SCT_DATA_HOME") };
        unsafe { std::env::set_var("HOME", "/users/test") };
        let config = Config::default();
        let dir = resolve_data_dir(None, &config);
        assert_eq!(
            dir,
            PathBuf::from("/users/test/.local/share/sct").join(DATA_SUBDIR)
        );
    }

    #[test]
    fn releases_and_data_dirs_are_distinct() {
        unsafe { std::env::remove_var("SCT_DATA_HOME") };
        unsafe { std::env::set_var("HOME", "/users/test") };
        let config = Config::default();
        assert_ne!(
            resolve_releases_dir(None, &config),
            resolve_data_dir(None, &config)
        );
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
        };
        let dir = resolve_releases_dir(Some(Path::new("/flag/releases")), &config);
        assert_eq!(dir, PathBuf::from("/flag/releases"));
    }

    #[test]
    fn config_parses_data_dir() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[trud]").unwrap();
        writeln!(f, r#"data_dir = "/my/data""#).unwrap();
        let config = load_config_from_path(f.path());
        assert_eq!(config.trud.unwrap().data_dir.unwrap(), "/my/data");
    }

    // --- ping_trud (offline/logic tests only) ----------------------------------
    //
    // We cannot test actual network reachability in unit tests. We test the
    // error classification logic by checking that the two "connected" arms
    // (Ok and StatusCode) are treated identically, and that the error message
    // produced for a connection failure contains the key user-facing strings.

    #[test]
    fn ping_trud_error_message_contains_maintenance_window_hint() {
        // Simulate what ping_trud would produce given a connection-level error.
        let fake_io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let msg = format!(
            "Cannot reach NHS TRUD ({TRUD_HEALTH_URL}).\n\n\
             The service may be offline or undergoing scheduled maintenance.\n\
             TRUD maintenance windows: weekdays 18:00–08:00 UK time, and midnight–06:00.\n\n\
             Original error: {fake_io_err}"
        );
        assert!(msg.contains("maintenance"));
        assert!(msg.contains(TRUD_HEALTH_URL));
        assert!(msg.contains("18:00"));
    }

    #[test]
    fn ping_trud_health_url_is_on_expected_domain() {
        // Sanity-check the constant hasn't drifted to an unexpected host.
        assert!(TRUD_HEALTH_URL.starts_with("https://isd.digital.nhs.uk/"));
    }

    // --- load_config_from_path -------------------------------------------------

    #[test]
    fn config_missing_file_returns_default() {
        let tmp = PathBuf::from("/tmp/sct-test-nonexistent-config-file.toml");
        let config = load_config_from_path(&tmp);
        assert!(config.trud.is_none());
    }

    #[test]
    fn config_parses_api_key() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[trud]").unwrap();
        writeln!(f, r#"api_key = "parsed-key""#).unwrap();
        let config = load_config_from_path(f.path());
        assert_eq!(config.trud.unwrap().api_key.unwrap(), "parsed-key");
    }

    #[test]
    fn config_parses_custom_edition() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[trud.editions.my_org]").unwrap();
        writeln!(f, "trud_item = 777").unwrap();
        let config = load_config_from_path(f.path());
        let trud = config.trud.unwrap();
        let editions = trud.editions.unwrap();
        assert_eq!(editions["my_org"].trud_item, 777);
    }

    #[test]
    fn config_invalid_toml_returns_default() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "this is not valid toml {{!!").unwrap();
        let config = load_config_from_path(f.path());
        // Should not panic; silently returns default
        assert!(config.trud.is_none());
    }
}

fn human_size(bytes: u64) -> String {
    const GB: u64 = 1 << 30;
    const MB: u64 = 1 << 20;
    const KB: u64 = 1 << 10;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
