// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct ndjson` - Convert an RF2 Snapshot directory to a canonical NDJSON artefact.

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use crate::provenance::{self, Provenance};
use crate::rf2::{discover_rf2_files, Rf2Dataset};

/// Which reference sets to load from RF2.
#[derive(ValueEnum, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RefsetMode {
    /// Skip all generic refset membership (language and simple-map refsets still load).
    None,
    /// Load concept-level simple refsets (default).
    #[default]
    Simple,
    /// Load all supported refset families, including map payloads, AttributeValue,
    /// and Association history. Larger and slower; needed for richer terminology.
    All,
}

#[derive(Parser, Debug)]
pub struct Args {
    /// Path to an RF2 Snapshot directory, or a .zip archive of an RF2 release.
    /// May be specified multiple times to layer a base release with one or more
    /// extensions (e.g. UK clinical + drug extension).
    #[arg(long = "rf2", required = true, num_args = 1.., value_parser = crate::paths::tilde_pathbuf)]
    pub rf2_dirs: Vec<PathBuf>,

    /// BCP-47 locale for preferred term selection (e.g. en-GB, en-US).
    #[arg(long, default_value = "en-GB")]
    pub locale: String,

    /// Output file path (NDJSON). Defaults to a slugified version of the first
    /// RF2 directory name. Use `-o -` to write to stdout.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub output: Option<PathBuf>,

    /// Include inactive concepts in output (omitted by default).
    #[arg(long, default_value_t = false)]
    pub include_inactive: bool,

    /// Which reference sets to include. `simple` (default) loads concept-level
    /// Simple refsets such as SCR exclusion; `none` skips them; `all` additionally
    /// loads ComplexMap, ExtendedMap, AttributeValue, and Association refsets.
    /// See `spec/cross-terminology-mapping.md`.
    #[arg(long, value_enum, default_value_t = RefsetMode::default())]
    pub refsets: RefsetMode,
}

/// Placeholder with the exact shape and length of a real content fingerprint
/// (`sha256:` + 64 hex chars). The provenance header is written first with this
/// placeholder so records can stream straight to the file; once the true
/// fingerprint is known the placeholder is overwritten in place.
const FINGERPRINT_PLACEHOLDER: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";
const FINGERPRINT_FIELD_PREFIX: &str = "\"content_fingerprint\":\"";

fn fingerprint_offset(provenance_line: &str) -> Result<u64> {
    let offset = provenance_line
        .find(FINGERPRINT_FIELD_PREFIX)
        .context("locating content fingerprint in provenance header")?
        + FINGERPRINT_FIELD_PREFIX.len();
    anyhow::ensure!(
        provenance_line[offset..].starts_with(FINGERPRINT_PLACEHOLDER),
        "provenance header does not contain the fingerprint placeholder"
    );
    Ok(offset as u64)
}

/// Stream every concept record to `writer`. Each record is serialised exactly
/// once: the same bytes feed the content fingerprint and the output, halving
/// the serialisation work of the previous collect-then-write implementation.
fn stream_records_to(dataset: &Rf2Dataset, args: &Args, writer: &mut impl Write) -> Result<String> {
    let mut fingerprint = provenance::ContentFingerprint::new();
    crate::builder::stream_records(dataset, &args.locale, args.include_inactive, |record| {
        let encoded = serde_json::to_vec(&record).context("serialising record")?;
        fingerprint.update(&encoded);
        writer.write_all(&encoded)?;
        writer.write_all(b"\n")?;
        Ok(())
    })?;
    Ok(fingerprint.finish())
}

pub fn run(args: Args) -> Result<()> {
    // --- Resolve each --rf2 path, extracting ZIPs to temp dirs as needed ---
    // _temp_dirs keeps the TempDir values alive until we finish writing output.
    let mut _temp_dirs: Vec<tempfile::TempDir> = Vec::new();
    let mut resolved_dirs: Vec<PathBuf> = Vec::new();
    for path in &args.rf2_dirs {
        let (dir, maybe_tmp) = maybe_extract_zip(path)?;
        if let Some(tmp) = maybe_tmp {
            _temp_dirs.push(tmp);
        }
        resolved_dirs.push(dir);
    }

    // --- Discover RF2 files across all supplied directories ---
    let mut all_files = crate::rf2::Rf2Files::default();
    for dir in &resolved_dirs {
        eprintln!("Scanning {}", dir.display());
        let found =
            discover_rf2_files(dir).with_context(|| format!("scanning {}", dir.display()))?;
        all_files.concept_files.extend(found.concept_files);
        all_files.description_files.extend(found.description_files);
        all_files
            .relationship_files
            .extend(found.relationship_files);
        all_files.lang_refset_files.extend(found.lang_refset_files);
        all_files.simple_map_files.extend(found.simple_map_files);
        all_files.refset_files.extend(found.refset_files);
        all_files
            .extended_map_files
            .extend(found.extended_map_files);
        all_files.complex_map_files.extend(found.complex_map_files);
        all_files
            .attribute_value_files
            .extend(found.attribute_value_files);
        all_files.association_files.extend(found.association_files);
    }

    // Refset mode gates the heavier payload and history families, which load
    // only under `--refsets all`.
    match args.refsets {
        RefsetMode::None => {
            all_files.refset_files.clear();
            all_files.extended_map_files.clear();
            all_files.complex_map_files.clear();
            all_files.attribute_value_files.clear();
            all_files.association_files.clear();
        }
        RefsetMode::Simple => {
            all_files.extended_map_files.clear();
            all_files.complex_map_files.clear();
            all_files.attribute_value_files.clear();
            all_files.association_files.clear();
        }
        RefsetMode::All => {}
    }

    if all_files.concept_files.is_empty() {
        anyhow::bail!(
            "No sct2_Concept_Snapshot_*.txt files found. \
             Check that the supplied path(s) point to an RF2 Snapshot directory."
        );
    }

    eprintln!(
        "Found: {} concept, {} description, {} relationship, {} lang refset, {} simple map, {} simple refset, {} extended map, {} complex map, {} attribute value, {} association file(s)",
        all_files.concept_files.len(),
        all_files.description_files.len(),
        all_files.relationship_files.len(),
        all_files.lang_refset_files.len(),
        all_files.simple_map_files.len(),
        all_files.refset_files.len(),
        all_files.extended_map_files.len(),
        all_files.complex_map_files.len(),
        all_files.attribute_value_files.len(),
        all_files.association_files.len(),
    );

    // --- Load dataset ---
    eprintln!("Loading RF2 data...");
    let dataset =
        Rf2Dataset::load(&all_files, args.include_inactive).context("loading RF2 files")?;

    // --- Build + write output records (single streaming pass) ---
    // Records are built and written one at a time instead of materialising the
    // full record set: on a national edition that set runs to gigabytes and
    // previously dominated peak memory alongside the loaded dataset.
    eprintln!(
        "Building and writing {} concept records (locale={}, include_inactive={})...",
        dataset.concepts.len(),
        args.locale,
        args.include_inactive
    );

    // Resolve output path. "-" means explicit stdout.
    let output_path: Option<PathBuf> = match &args.output {
        Some(p) if p.as_os_str() == "-" => None,
        Some(p) => Some(p.clone()),
        None => {
            let slug = slugify_path(&args.rf2_dirs[0]);
            let filename = format!("{}.ndjson", slug);
            eprintln!("Output: {}", filename);
            Some(PathBuf::from(filename))
        }
    };

    let payload_refset_count = dataset.extended_map_members.len()
        + dataset.complex_map_members.len()
        + dataset.attribute_value_members.len();
    anyhow::ensure!(
        output_path.is_some() || (payload_refset_count == 0 && dataset.history.is_empty()),
        "--refsets all found payload/history records that require companion NDJSON files; use --output <FILE> instead of stdout"
    );

    // Provenance header line. Emitted before any concept records so that
    // downstream tools (`sct sqlite`, `sct info`, etc.) can cite the source
    // edition and release date without the user having to remember them.
    //
    // The content fingerprint is only known after every record has been
    // serialised, but records stream to keep memory flat. For file output the
    // header therefore carries a fixed-length placeholder fingerprint that is
    // overwritten in place afterwards; for stdout (not seekable) records are
    // spooled to a temp file and copied out after the real header.
    let payload_refset_fingerprint = if payload_refset_count > 0 {
        Some(fingerprint_refset_records(&dataset)?)
    } else {
        None
    };
    let history_fingerprint = if dataset.history.is_empty() {
        None
    } else {
        Some(fingerprint_history_records(&dataset)?)
    };

    let mut provenance = Provenance::from_rf2_paths(&args.rf2_dirs);
    if let Some(fingerprint) = &payload_refset_fingerprint {
        provenance.companions.push(provenance::CompanionArtifact {
            kind: provenance::COMPANION_PAYLOAD_REFSETS.to_string(),
            schema_version: crate::schema::REFSET_SIDECAR_SCHEMA_VERSION,
            record_count: payload_refset_count as u64,
            content_fingerprint: fingerprint.clone(),
        });
    }
    if let Some(fingerprint) = &history_fingerprint {
        provenance.companions.push(provenance::CompanionArtifact {
            kind: provenance::COMPANION_HISTORY.to_string(),
            schema_version: crate::schema::HISTORY_SIDECAR_SCHEMA_VERSION,
            record_count: dataset.history.len() as u64,
            content_fingerprint: fingerprint.clone(),
        });
    }
    provenance.content_fingerprint = Some(FINGERPRINT_PLACEHOLDER.to_string());
    let prov_line = serde_json::to_string(&provenance).context("serialising provenance")?;

    let (fingerprint, pending_main) = match &output_path {
        Some(path) => {
            let file = temporary_output(path, "output file")?;
            let mut writer = BufWriter::new(file);
            let fp_offset = fingerprint_offset(&prov_line)?;
            writer.write_all(prov_line.as_bytes())?;
            writer.write_all(b"\n")?;

            let fingerprint = stream_records_to(&dataset, &args, &mut writer)?;

            let mut file = writer
                .into_inner()
                .map_err(|e| anyhow::anyhow!("flushing output: {e}"))?;
            file.as_file_mut().seek(SeekFrom::Start(fp_offset))?;
            file.write_all(fingerprint.as_bytes())?;
            file.as_file()
                .sync_all()
                .with_context(|| format!("syncing output file for {}", path.display()))?;
            (fingerprint, Some(file))
        }
        None => {
            // Stdout is not seekable: spool records to an unnamed temp file
            // while fingerprinting, then emit the real header and copy.
            let mut spool = tempfile::tempfile().context("creating stdout spool file")?;
            let fingerprint = {
                let mut w = BufWriter::new(&mut spool);
                let fp = stream_records_to(&dataset, &args, &mut w)?;
                w.flush()?;
                fp
            };
            provenance.content_fingerprint = Some(fingerprint.clone());
            let prov_line = serde_json::to_string(&provenance).context("serialising provenance")?;
            let stdout = std::io::stdout();
            let mut out = BufWriter::new(stdout.lock());
            out.write_all(prov_line.as_bytes())?;
            out.write_all(b"\n")?;
            spool.seek(SeekFrom::Start(0))?;
            std::io::copy(&mut spool, &mut out).context("copying spooled records to stdout")?;
            out.flush()?;
            (fingerprint, None)
        }
    };
    provenance.content_fingerprint = Some(fingerprint);

    // Prepare every file before replacing any existing bundle member. Companion
    // files publish first and the manifest-bearing main stream publishes last.
    if let Some(path) = &output_path {
        let refset_sidecar = refset_sidecar_path(path);
        let pending_refsets = payload_refset_fingerprint
            .as_deref()
            .map(|expected| write_refset_sidecar(&refset_sidecar, &dataset, &provenance, expected))
            .transpose()?;

        let history_sidecar = history_sidecar_path(path);
        let pending_history = history_fingerprint
            .as_deref()
            .map(|expected| write_history_sidecar(&history_sidecar, &dataset, expected))
            .transpose()?;

        let wrote_refsets = pending_refsets.is_some();
        let wrote_history = pending_history.is_some();
        publish_outputs(vec![
            PendingOutput {
                path: refset_sidecar.clone(),
                temp: pending_refsets,
                label: "payload-refset companion",
            },
            PendingOutput {
                path: history_sidecar.clone(),
                temp: pending_history,
                label: "history companion",
            },
            PendingOutput {
                path: path.clone(),
                temp: Some(pending_main.context("missing pending main NDJSON output")?),
                label: "main NDJSON artefact",
            },
        ])?;

        if wrote_refsets {
            eprintln!(
                "Wrote {} payload refset rows to {}",
                payload_refset_count,
                refset_sidecar.display()
            );
        }
        if wrote_history {
            eprintln!(
                "Wrote {} history rows to {}",
                dataset.history.len(),
                history_sidecar.display()
            );
        }
    }

    crate::progress::debug_mem("ndjson written");
    eprintln!("Done.");
    Ok(())
}

/// Derive the history sidecar path from an NDJSON path:
/// `foo.ndjson` → `foo.history.ndjson`.
pub fn history_sidecar_path(ndjson: &Path) -> PathBuf {
    let s = ndjson.to_string_lossy();
    let base = s.strip_suffix(".ndjson").unwrap_or(&s);
    PathBuf::from(format!("{base}.history.ndjson"))
}

/// Derive the payload-refset sidecar path from an NDJSON path:
/// `foo.ndjson` -> `foo.refsets.ndjson`.
pub fn refset_sidecar_path(ndjson: &Path) -> PathBuf {
    let s = ndjson.to_string_lossy();
    let base = s.strip_suffix(".ndjson").unwrap_or(&s);
    PathBuf::from(format!("{base}.refsets.ndjson"))
}

fn for_each_refset_record(
    dataset: &Rf2Dataset,
    mut f: impl FnMut(crate::schema::RefsetMemberRecord) -> Result<()>,
) -> Result<()> {
    use crate::schema::RefsetMemberRecord;

    for member in &dataset.complex_map_members {
        f(RefsetMemberRecord::ComplexMap(member.clone()))?;
    }
    for member in &dataset.extended_map_members {
        f(RefsetMemberRecord::ExtendedMap(member.clone()))?;
    }
    for member in &dataset.attribute_value_members {
        f(RefsetMemberRecord::AttributeValue(member.clone()))?;
    }
    Ok(())
}

fn fingerprint_refset_records(dataset: &Rf2Dataset) -> Result<String> {
    let mut fingerprint = provenance::ContentFingerprint::new();
    for_each_refset_record(dataset, |record| {
        let encoded = serde_json::to_vec(&record).context("serialising refset member")?;
        fingerprint.update(&encoded);
        Ok(())
    })?;
    Ok(fingerprint.finish())
}

fn history_record(source: &str, association: &str, target: &str) -> crate::schema::HistoryRecord {
    crate::schema::HistoryRecord {
        source: source.to_string(),
        association: association.to_string(),
        target: target.to_string(),
    }
}

fn fingerprint_history_records(dataset: &Rf2Dataset) -> Result<String> {
    let mut fingerprint = provenance::ContentFingerprint::new();
    for (source, association, target) in &dataset.history {
        let encoded = serde_json::to_vec(&history_record(source, association, target))
            .context("serialising history record")?;
        fingerprint.update(&encoded);
    }
    Ok(fingerprint.finish())
}

fn write_refset_sidecar(
    path: &Path,
    dataset: &Rf2Dataset,
    source: &Provenance,
    expected_fingerprint: &str,
) -> Result<NamedTempFile> {
    let header = crate::schema::RefsetSidecarProvenance::new(
        source.clone(),
        expected_fingerprint.to_string(),
    );
    let header_line = serde_json::to_string(&header).context("serialising refset provenance")?;

    let mut file = temporary_output(path, "payload-refset companion")?;
    let mut writer = BufWriter::new(file.as_file_mut());
    writer.write_all(header_line.as_bytes())?;
    writer.write_all(b"\n")?;

    let mut fingerprint = provenance::ContentFingerprint::new();
    for_each_refset_record(dataset, |record| {
        let encoded = serde_json::to_vec(&record).context("serialising refset member")?;
        fingerprint.update(&encoded);
        writer.write_all(&encoded)?;
        writer.write_all(b"\n")?;
        Ok(())
    })?;
    let actual_fingerprint = fingerprint.finish();
    anyhow::ensure!(
        actual_fingerprint == expected_fingerprint,
        "refset records changed while writing companion stream"
    );
    writer.flush()?;
    drop(writer);
    file.as_file()
        .sync_all()
        .with_context(|| format!("syncing refset sidecar for {}", path.display()))?;
    Ok(file)
}

fn write_history_sidecar(
    path: &Path,
    dataset: &Rf2Dataset,
    expected_fingerprint: &str,
) -> Result<NamedTempFile> {
    let mut file = temporary_output(path, "history companion")?;
    let mut writer = BufWriter::new(file.as_file_mut());
    let mut fingerprint = provenance::ContentFingerprint::new();
    for (source, association, target) in &dataset.history {
        let encoded = serde_json::to_vec(&history_record(source, association, target))
            .context("serialising history record")?;
        fingerprint.update(&encoded);
        writer.write_all(&encoded)?;
        writer.write_all(b"\n")?;
    }
    let actual_fingerprint = fingerprint.finish();
    anyhow::ensure!(
        actual_fingerprint == expected_fingerprint,
        "history records changed while writing companion stream"
    );
    writer.flush()?;
    drop(writer);
    file.as_file()
        .sync_all()
        .with_context(|| format!("syncing history sidecar for {}", path.display()))?;
    Ok(file)
}

fn temporary_output(path: &Path, label: &str) -> Result<NamedTempFile> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let existing_permissions = match std::fs::metadata(path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("reading metadata for {}", path.display()))
        }
    };
    let mut builder = tempfile::Builder::new();
    if let Some(permissions) = existing_permissions.clone() {
        builder.permissions(permissions);
    }
    #[cfg(unix)]
    if existing_permissions.is_none() {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    builder
        .tempfile_in(parent)
        .with_context(|| format!("creating temporary {label} in {}", parent.display()))
}

struct PendingOutput {
    path: PathBuf,
    /// `None` removes a stale companion as part of the same publication unit.
    temp: Option<NamedTempFile>,
    label: &'static str,
}

struct PublishedOutput {
    path: PathBuf,
    backup: Option<tempfile::TempPath>,
}

fn publish_outputs(outputs: Vec<PendingOutput>) -> Result<()> {
    let mut published = Vec::with_capacity(outputs.len());
    for output in outputs {
        let backup = match backup_existing_output(&output.path, output.label) {
            Ok(backup) => backup,
            Err(error) => {
                rollback_outputs(&mut published);
                return Err(error);
            }
        };
        published.push(PublishedOutput {
            path: output.path.clone(),
            backup,
        });

        if let Some(temp) = output.temp {
            if let Err(error) = temp
                .persist(&output.path)
                .map_err(|error| error.error)
                .with_context(|| {
                    format!(
                        "atomically replacing {} {}",
                        output.label,
                        output.path.display()
                    )
                })
            {
                rollback_outputs(&mut published);
                return Err(error);
            }
        }
        if let Err(error) = sync_output_directory(&output.path) {
            rollback_outputs(&mut published);
            return Err(error);
        }
    }
    Ok(())
}

fn backup_existing_output(path: &Path, label: &str) -> Result<Option<tempfile::TempPath>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("reading metadata for {}", path.display()))
        }
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!(
            "cannot replace {label} {}: path is not a file",
            path.display()
        ),
    }

    let parent = output_parent(path);
    let backup = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating backup for {}", path.display()))?
        .into_temp_path();
    std::fs::remove_file(&backup)
        .with_context(|| format!("preparing backup path for {}", path.display()))?;
    std::fs::rename(path, &backup).with_context(|| format!("backing up {}", path.display()))?;
    Ok(Some(backup))
}

fn rollback_outputs(published: &mut Vec<PublishedOutput>) {
    for output in published.drain(..).rev() {
        if output.path.is_file() || output.path.is_symlink() {
            let _ = std::fs::remove_file(&output.path);
        }
        if let Some(backup) = output.backup {
            let _ = std::fs::rename(&backup, &output.path);
        }
        let _ = sync_output_directory(&output.path);
    }
}

fn output_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn sync_output_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let parent = output_parent(path);
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("syncing output directory {}", parent.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Derive a slug from a directory path for use as a default output filename.
///
/// Examples:
///   `SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z`  →  `snomedct-monolithrf2-production-20260311t120000z`
///   `./releases/snomed-ct/`                             →  `snomed-ct`
pub fn slugify_path(path: &std::path::Path) -> String {
    let name = path
        .components()
        .filter_map(|c| {
            if let std::path::Component::Normal(s) = c {
                s.to_str()
            } else {
                None
            }
        })
        .next_back()
        .unwrap_or("snomed");

    // Strip a trailing .zip so the slug reflects the release name, not the archive extension.
    let name = name.strip_suffix(".zip").unwrap_or(name);
    let name = name.strip_suffix(".ZIP").unwrap_or(name);

    let lower = name.to_lowercase();
    let mut slug = String::with_capacity(lower.len());
    let mut prev_hyphen = false;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            prev_hyphen = false;
        } else if !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }
    slug.trim_matches('-').to_string()
}

/// If `path` has a `.zip` extension, extract the archive to a temporary directory
/// and return the path to use (the single top-level subdirectory, if any) together
/// with the `TempDir` handle that the caller must keep alive.
///
/// If `path` is already a directory, return it as-is with no `TempDir`.
fn maybe_extract_zip(path: &PathBuf) -> Result<(PathBuf, Option<tempfile::TempDir>)> {
    let is_zip = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);

    if !is_zip {
        return Ok((path.clone(), None));
    }

    eprintln!("Extracting {} ...", path.display());
    let tmp = tempfile::tempdir().context("creating temporary extraction directory")?;
    extract_zip(path, tmp.path()).with_context(|| format!("extracting {}", path.display()))?;

    // If the archive contains exactly one top-level directory, use that -
    // SNOMED CT ZIPs normally contain a single directory named after the release.
    let top_dirs: Vec<_> = std::fs::read_dir(tmp.path())
        .context("reading extraction directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    let rf2_dir = if top_dirs.len() == 1 {
        top_dirs[0].path()
    } else {
        tmp.path().to_path_buf()
    };

    eprintln!("Extracted to {}", rf2_dir.display());
    Ok((rf2_dir, Some(tmp)))
}

/// Extract a ZIP archive to `dest`, guarding against path traversal.
fn extract_zip(zip_path: &PathBuf, dest: &Path) -> Result<()> {
    let file =
        std::fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("reading zip archive {}", zip_path.display()))?;

    let total = archive.len();
    for i in 0..total {
        let mut entry = archive.by_index(i)?;

        // enclosed_name() returns None for unsafe paths (e.g. ../escape).
        let entry_path = match entry.enclosed_name() {
            Some(p) => dest.join(p),
            None => {
                eprintln!("  skipping unsafe zip entry: {}", entry.name());
                continue;
            }
        };

        if entry.is_dir() {
            std::fs::create_dir_all(&entry_path)
                .with_context(|| format!("creating directory {}", entry_path.display()))?;
        } else {
            if let Some(parent) = entry_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&entry_path)
                .with_context(|| format!("creating {}", entry_path.display()))?;
            std::io::copy(&mut entry, &mut out)?;
        }

        if (i + 1) % 5000 == 0 || i + 1 == total {
            eprint!("\r  {}/{} entries extracted", i + 1, total);
        }
    }
    eprintln!(); // newline after progress
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn slugify_monolith_dir() {
        assert_eq!(
            slugify_path(Path::new(
                "SnomedCT_MonolithRF2_PRODUCTION_20260311T120000Z"
            )),
            "snomedct-monolithrf2-production-20260311t120000z"
        );
    }

    #[test]
    fn slugify_trailing_slash() {
        assert_eq!(
            slugify_path(Path::new("./releases/snomed-ct/")),
            "snomed-ct"
        );
    }

    #[test]
    fn slugify_uk_clinical() {
        assert_eq!(
            slugify_path(Path::new(
                "SnomedCT_UKClinicalRF2_PRODUCTION_20250401T000001Z"
            )),
            "snomedct-ukclinicalrf2-production-20250401t000001z"
        );
    }

    #[test]
    fn fingerprint_offset_targets_content_fingerprint_field() {
        let mut provenance = Provenance::from_rf2_paths(&[PathBuf::from(FINGERPRINT_PLACEHOLDER)]);
        provenance.content_fingerprint = Some(FINGERPRINT_PLACEHOLDER.to_string());
        let mut line = serde_json::to_string(&provenance).unwrap();
        let fingerprint = "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        let offset = fingerprint_offset(&line).unwrap() as usize;
        line.replace_range(offset..offset + FINGERPRINT_PLACEHOLDER.len(), fingerprint);

        let parsed: Provenance = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed.release_id, FINGERPRINT_PLACEHOLDER);
        assert_eq!(parsed.source_paths, [FINGERPRINT_PLACEHOLDER]);
        assert_eq!(parsed.content_fingerprint.as_deref(), Some(fingerprint));
    }
}
