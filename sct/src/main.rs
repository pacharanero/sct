mod builder;
mod rf2;

use anyhow::{Context, Result};
use clap::Parser;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use builder::build_records;
use rf2::{Rf2Dataset, discover_rf2_files};

/// sct: Convert an RF2 Snapshot to a canonical NDJSON artefact.
///
/// Each output line is a self-contained JSON object representing one SNOMED CT
/// concept, with FSN, preferred term, synonyms, hierarchy path, parents,
/// children count, and attributes.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to an RF2 Snapshot directory. May be specified multiple times to
    /// layer a base release with one or more extensions (e.g. UK clinical +
    /// drug extension).
    #[arg(long = "rf2", required = true, num_args = 1..)]
    rf2_dirs: Vec<PathBuf>,

    /// BCP-47 locale for preferred term selection (e.g. en-GB, en-US).
    #[arg(long, default_value = "en-GB")]
    locale: String,

    /// Output file path (NDJSON). Defaults to a slugified version of the
    /// first RF2 directory name, e.g. `snomed-ct-uk-clinical-rf2-production-20250401.ndjson`.
    /// Use `-o -` to write to stdout.
    #[arg(long, short)]
    output: Option<PathBuf>,

    /// Include inactive concepts in output (omitted by default).
    #[arg(long, default_value_t = false)]
    include_inactive: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // --- Discover RF2 files across all supplied directories ---
    let mut all_files = rf2::Rf2Files::default();
    for dir in &args.rf2_dirs {
        eprintln!("Scanning {}", dir.display());
        let found = discover_rf2_files(dir)
            .with_context(|| format!("scanning {}", dir.display()))?;
        all_files.concept_files.extend(found.concept_files);
        all_files.description_files.extend(found.description_files);
        all_files.relationship_files.extend(found.relationship_files);
        all_files.lang_refset_files.extend(found.lang_refset_files);
    }

    if all_files.concept_files.is_empty() {
        anyhow::bail!(
            "No sct2_Concept_Snapshot_*.txt files found. \
             Check that the supplied path(s) point to an RF2 Snapshot directory."
        );
    }

    eprintln!(
        "Found: {} concept, {} description, {} relationship, {} lang refset file(s)",
        all_files.concept_files.len(),
        all_files.description_files.len(),
        all_files.relationship_files.len(),
        all_files.lang_refset_files.len(),
    );

    // --- Load dataset ---
    eprintln!("Loading RF2 data...");
    let dataset = Rf2Dataset::load(&all_files).context("loading RF2 files")?;

    // --- Build output records ---
    eprintln!(
        "Building concept records (locale={}, include_inactive={})...",
        args.locale, args.include_inactive
    );
    let records = build_records(&dataset, &args.locale, args.include_inactive)
        .context("building concept records")?;

    eprintln!("Writing {} records...", records.len());

    // --- Resolve output path ---
    // "-" means explicit stdout; no --output means derive from first RF2 dir name.
    let output_path: Option<PathBuf> = match &args.output {
        Some(p) if p.as_os_str() == "-" => None, // explicit stdout
        Some(p) => Some(p.clone()),
        None => {
            let slug = slugify_path(&args.rf2_dirs[0]);
            let filename = format!("{}.ndjson", slug);
            eprintln!("Output: {}", filename);
            Some(PathBuf::from(filename))
        }
    };

    // --- Write NDJSON ---
    let writer: Box<dyn Write> = match &output_path {
        Some(path) => {
            let f = std::fs::File::create(path)
                .with_context(|| format!("creating output file {}", path.display()))?;
            Box::new(BufWriter::new(f))
        }
        None => Box::new(BufWriter::new(std::io::stdout())),
    };

    let mut writer = writer;
    for record in &records {
        let line = serde_json::to_string(record).context("serialising record")?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;

    eprintln!("Done.");
    Ok(())
}

/// Derive a slug from a directory path for use as a default output filename.
///
/// Takes the last path component, lowercases it, replaces any run of
/// non-alphanumeric characters with a single hyphen, and trims leading/trailing
/// hyphens.
///
/// Examples:
///   `SnomedCT_UKClinicalRF2_PRODUCTION_20250401T000001Z`  →  `snomedct-ukclinicalrf2-production-20250401t000001z`
///   `./releases/snomed-ct/`                               →  `snomed-ct`
fn slugify_path(path: &std::path::Path) -> String {
    // Use the last non-empty component (handles trailing slashes)
    let name = path
        .components()
        .filter_map(|c| {
            if let std::path::Component::Normal(s) = c {
                s.to_str()
            } else {
                None
            }
        })
        .last()
        .unwrap_or("snomed");

    let lower = name.to_lowercase();
    // Replace runs of non-alphanumeric chars with a single hyphen
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
    // Trim leading/trailing hyphens
    slug.trim_matches('-').to_string()
}
