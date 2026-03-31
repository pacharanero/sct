//! `sct codelist` — Build, validate, and publish clinical code lists.
//!
//! Also accessible as `sct refset` and `sct valueset`.
//!
//! `.codelist` files are plain UTF-8 with YAML front-matter and a concept list body.
//! They are designed to live in version control and be reviewed like source code.
//!
//! Examples:
//!   sct codelist new codelists/asthma-diagnosis.codelist
//!   sct codelist add codelists/asthma-diagnosis.codelist 195967001 --db snomed.db
//!   sct codelist validate codelists/asthma-diagnosis.codelist --db snomed.db
//!   sct codelist stats codelists/asthma-diagnosis.codelist --db snomed.db
//!   sct codelist diff codelists/asthma-v1.codelist codelists/asthma-v2.codelist
//!   sct codelist export codelists/asthma-diagnosis.codelist --format csv

use anyhow::{bail, Context, Result};
use chrono::Local;
use clap::{Parser, Subcommand};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Verb,
}

#[derive(Subcommand, Debug)]
pub enum Verb {
    /// Scaffold a new .codelist file from template.
    New(NewArgs),
    /// Add a concept to a codelist (resolved from the SNOMED CT database).
    Add(AddArgs),
    /// Move a concept to an explicit exclusion record.
    Remove(RemoveArgs),
    /// Validate a codelist against the SNOMED CT database (CI-ready).
    Validate(ValidateArgs),
    /// Print concept count, hierarchy breakdown, and staleness info.
    Stats(StatsArgs),
    /// Human-readable diff between two .codelist files.
    Diff(DiffArgs),
    /// Export a codelist to CSV, Markdown, or other formats.
    Export(ExportArgs),
    /// Interactive FTS5 search → include/exclude concepts (requires --db).
    Search(SearchArgs),
    /// Import a codelist from OpenCodelists, CSV, or FHIR.
    Import(ImportArgs),
    /// Publish a codelist to OpenCodelists.
    Publish(PublishArgs),
}

#[derive(Parser, Debug)]
pub struct NewArgs {
    /// Path for the new .codelist file.
    pub file: PathBuf,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    /// Terminology: "SNOMED CT", "ICD-10", "dm+d", "CTV3", "BNF".
    #[arg(long, default_value = "SNOMED CT")]
    pub terminology: String,
    #[arg(long)]
    pub author: Option<String>,
    /// Skip opening $EDITOR after scaffolding.
    #[arg(long)]
    pub no_edit: bool,
}

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// Path to the .codelist file.
    pub file: PathBuf,
    /// One or more SCTIDs to add.
    pub sctids: Vec<String>,
    /// SNOMED CT SQLite database.
    #[arg(long, default_value = "snomed.db")]
    pub db: PathBuf,
    /// Also add all active descendants.
    #[arg(long)]
    pub include_descendants: bool,
    /// Inline comment to append to added lines.
    #[arg(long)]
    pub comment: Option<String>,
}

#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// Path to the .codelist file.
    pub file: PathBuf,
    /// SCTID to move to exclusion.
    pub sctid: String,
    /// Reason to append as an inline comment.
    #[arg(long)]
    pub comment: Option<String>,
}

#[derive(Parser, Debug)]
pub struct ValidateArgs {
    /// Path to the .codelist file.
    pub file: PathBuf,
    /// SNOMED CT SQLite database.
    #[arg(long, default_value = "snomed.db")]
    pub db: PathBuf,
}

#[derive(Parser, Debug)]
pub struct StatsArgs {
    /// Path to the .codelist file.
    pub file: PathBuf,
    /// SNOMED CT SQLite database.
    #[arg(long, default_value = "snomed.db")]
    pub db: PathBuf,
}

#[derive(Parser, Debug)]
pub struct DiffArgs {
    /// First .codelist file.
    pub file_a: PathBuf,
    /// Second .codelist file.
    pub file_b: PathBuf,
}

#[derive(Parser, Debug)]
pub struct ExportArgs {
    /// Path to the .codelist file.
    pub file: PathBuf,
    /// Output format: csv, opencodelists-csv, markdown, fhir-json, rf2.
    #[arg(long, default_value = "csv")]
    pub format: String,
    /// Write to file instead of stdout.
    #[arg(long, short)]
    pub output: Option<PathBuf>,
}

#[derive(Parser, Debug)]
pub struct SearchArgs {
    /// Path to the .codelist file.
    pub file: PathBuf,
    /// Search query.
    pub query: String,
    /// SNOMED CT SQLite database.
    #[arg(long, default_value = "snomed.db")]
    pub db: PathBuf,
}

#[derive(Parser, Debug)]
pub struct ImportArgs {
    /// Path for the new or target .codelist file.
    pub file: PathBuf,
    /// Source type: opencodelists, csv, rf2, fhir-json.
    #[arg(long)]
    pub from: String,
    /// URL or file path of the source.
    pub source: String,
}

#[derive(Parser, Debug)]
pub struct PublishArgs {
    /// Path to the .codelist file.
    pub file: PathBuf,
    /// Destination: "opencodelists" or a sct serve URL.
    #[arg(long, default_value = "opencodelists")]
    pub to: String,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Verb::New(a) => cmd_new(a),
        Verb::Add(a) => cmd_add(a),
        Verb::Remove(a) => cmd_remove(a),
        Verb::Validate(a) => cmd_validate(a),
        Verb::Stats(a) => cmd_stats(a),
        Verb::Diff(a) => cmd_diff(a),
        Verb::Export(a) => cmd_export(a),
        Verb::Search(_) => bail!(
            "`sct codelist search` is not yet implemented.\n\
             Use `sct lexical --db <db> --query <query>` for FTS5 search,\n\
             then `sct codelist add <file> <sctid>` to add concepts."
        ),
        Verb::Import(_) => bail!("`sct codelist import` is not yet implemented."),
        Verb::Publish(_) => bail!("`sct codelist publish` is not yet implemented."),
    }
}

// ---------------------------------------------------------------------------
// .codelist file format — types
// ---------------------------------------------------------------------------

/// YAML front-matter of a `.codelist` file.
#[derive(Debug, Serialize, Deserialize)]
pub struct FrontMatter {
    pub id: String,
    pub title: String,
    pub description: String,
    pub terminology: String,
    pub created: String,
    pub updated: String,
    pub version: u32,
    pub status: String,
    pub licence: String,
    pub copyright: String,
    pub appropriate_use: String,
    pub misuse: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snomed_release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<Author>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organisation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub methodology: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signoffs: Option<Vec<serde_yml::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<Warning>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub population: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub care_setting: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencodelists_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencodelists_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Author {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orcid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affiliation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub severity: String,
    pub message: String,
}

/// A single parsed line from the concept body.
#[derive(Debug, Clone)]
pub enum ConceptLine {
    /// An active concept: `195967001    Asthma (disorder)  # optional comment`
    Active {
        id: String,
        term: String,
        comment: Option<String>,
    },
    /// An explicitly excluded concept: `# 41553006   Occupational asthma (disorder)`
    Excluded {
        id: String,
        term: String,
        comment: Option<String>,
    },
    /// Pending review: `# ? 57607007  Irritant-induced asthma (disorder)`
    PendingReview {
        id: String,
        term: String,
    },
    /// Section header or free comment: `# ── heading ──`
    Comment(String),
    /// Blank line (preserved).
    Blank,
}

impl ConceptLine {
    fn sctid(&self) -> Option<&str> {
        match self {
            ConceptLine::Active { id, .. } => Some(id),
            ConceptLine::Excluded { id, .. } => Some(id),
            ConceptLine::PendingReview { id, .. } => Some(id),
            _ => None,
        }
    }

    fn is_active(&self) -> bool {
        matches!(self, ConceptLine::Active { .. })
    }
}

/// A fully parsed `.codelist` file.
pub struct CodelistFile {
    pub front_matter: FrontMatter,
    /// All lines of the body section, in order (preserves comments/blanks).
    pub body: Vec<ConceptLine>,
}

// ---------------------------------------------------------------------------
// Parse / serialise
// ---------------------------------------------------------------------------

pub fn read_codelist(path: &Path) -> Result<CodelistFile> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    parse_codelist(&text).with_context(|| format!("parsing {}", path.display()))
}

fn parse_codelist(text: &str) -> Result<CodelistFile> {
    // Split on YAML front-matter delimiters.
    let text = text.trim_start_matches('\u{feff}'); // strip BOM if present
    let after_first = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n"))
        .context("codelist file must start with '---'")?;
    let (yaml_part, body_part) = after_first.split_once("\n---")
        .context("codelist file missing closing '---' after front-matter")?;
    let body_part = body_part.trim_start_matches(['\n', '\r']);

    let front_matter: FrontMatter = serde_yml::from_str(yaml_part)
        .context("parsing YAML front-matter")?;

    let body = parse_body(body_part);
    Ok(CodelistFile { front_matter, body })
}

fn parse_body(text: &str) -> Vec<ConceptLine> {
    text.lines().map(parse_body_line).collect()
}

fn parse_body_line(line: &str) -> ConceptLine {
    let trimmed = line.trim();

    if trimmed.is_empty() {
        return ConceptLine::Blank;
    }

    // Line starts with `#`
    if let Some(rest) = trimmed.strip_prefix('#') {
        let rest = rest.trim();

        // Pending review: `# ? <digits> term`
        if let Some(rest) = rest.strip_prefix('?') {
            let rest = rest.trim();
            if let Some((id, term)) = split_id_term(rest) {
                return ConceptLine::PendingReview { id, term };
            }
        }

        // Excluded concept: `# <digits> term`
        if rest.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            if let Some((id, rest_of_line)) = rest.split_once(|c: char| c.is_whitespace()) {
                let (term, comment) = split_term_comment(rest_of_line.trim());
                return ConceptLine::Excluded { id: id.to_string(), term, comment };
            }
        }

        // Section comment or header
        return ConceptLine::Comment(trimmed.to_string());
    }

    // Active concept: `<digits> term [# comment]`
    if trimmed.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        if let Some((id, rest_of_line)) = trimmed.split_once(|c: char| c.is_whitespace()) {
            let (term, comment) = split_term_comment(rest_of_line.trim());
            return ConceptLine::Active { id: id.to_string(), term, comment };
        }
    }

    // Unrecognised — treat as comment
    ConceptLine::Comment(trimmed.to_string())
}

/// Split `"preferred term [# inline comment]"` into `(term, Option<comment>)`.
fn split_term_comment(s: &str) -> (String, Option<String>) {
    if let Some(idx) = s.find(" #") {
        let term = s[..idx].trim().to_string();
        let comment = s[idx + 2..].trim().to_string();
        (term, if comment.is_empty() { None } else { Some(comment) })
    } else {
        (s.trim().to_string(), None)
    }
}

/// Split `"12345 preferred term"` into `(id, term)`.
fn split_id_term(s: &str) -> Option<(String, String)> {
    let (id, rest) = s.split_once(|c: char| c.is_whitespace())?;
    if id.chars().all(|c| c.is_ascii_digit()) {
        Some((id.to_string(), rest.trim().to_string()))
    } else {
        None
    }
}

fn write_codelist(cl: &CodelistFile, path: &Path) -> Result<()> {
    let yaml = serde_yml::to_string(&cl.front_matter)
        .context("serialising YAML front-matter")?;
    let mut out = format!("---\n{}---\n", yaml);
    if !cl.body.is_empty() {
        out.push('\n');
        for line in &cl.body {
            out.push_str(&render_body_line(line));
            out.push('\n');
        }
    }
    std::fs::write(path, out)
        .with_context(|| format!("writing {}", path.display()))
}

fn render_body_line(line: &ConceptLine) -> String {
    match line {
        ConceptLine::Active { id, term, comment } => {
            let base = format!("{id:<14} {term}");
            match comment {
                Some(c) => format!("{base}  # {c}"),
                None => base,
            }
        }
        ConceptLine::Excluded { id, term, comment } => {
            let base = format!("# {id:<13} {term}");
            match comment {
                Some(c) => format!("{base}  # {c}"),
                None => base,
            }
        }
        ConceptLine::PendingReview { id, term } => format!("# ? {id}  {term}"),
        ConceptLine::Comment(s) => s.clone(),
        ConceptLine::Blank => String::new(),
    }
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_new(args: NewArgs) -> Result<()> {
    if args.file.exists() {
        bail!("{} already exists", args.file.display());
    }
    if let Some(parent) = args.file.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
    }

    let title = args.title.unwrap_or_else(|| {
        args.file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .replace('-', " ")
            .replace('_', " ")
    });

    let id = args.file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_lowercase()
        .replace(' ', "-");

    let today = today();

    let mut warnings = vec![
        Warning {
            code: "not-universal-definition".to_string(),
            severity: "info".to_string(),
            message: "This codelist was developed for a specific purpose and may not meet the needs of other studies.".to_string(),
        },
        Warning {
            code: "draft-not-reviewed".to_string(),
            severity: "info".to_string(),
            message: "This codelist has not yet been reviewed. Check status before use.".to_string(),
        },
    ];

    if args.terminology == "SNOMED CT" {
        warnings.push(Warning {
            code: "snomed-release-age".to_string(),
            severity: "caution".to_string(),
            message: "Validate against the current SNOMED release before use in research.".to_string(),
        });
    }

    if args.terminology == "dm+d" {
        warnings.push(Warning {
            code: "dmd-currency".to_string(),
            severity: "warning".to_string(),
            message: "dm+d codes change frequently. Check VMP code changes since snomed_release.".to_string(),
        });
        warnings.push(Warning {
            code: "dmd-vmp-code-change".to_string(),
            severity: "caution".to_string(),
            message: "VMP codes may have been superseded. Validate against current dm+d release.".to_string(),
        });
    }

    let authors = args.author.map(|name| {
        vec![Author { name, orcid: None, affiliation: None, role: Some("author".to_string()) }]
    });

    let fm = FrontMatter {
        id,
        title: title.clone(),
        description: args.description.unwrap_or_else(|| format!("{} codes", title)),
        terminology: args.terminology,
        created: today.clone(),
        updated: today,
        version: 1,
        status: "draft".to_string(),
        licence: "CC-BY-4.0".to_string(),
        copyright: "Copyright holder. SNOMED CT content © IHTSDO, used under NHS England national licence.".to_string(),
        appropriate_use: "Describe appropriate use here.".to_string(),
        misuse: "Describe misuse here.".to_string(),
        snomed_release: None,
        authors,
        organisation: None,
        methodology: None,
        signoffs: None,
        warnings: Some(warnings),
        population: None,
        care_setting: None,
        tags: None,
        opencodelists_id: None,
        opencodelists_url: None,
    };

    let cl = CodelistFile {
        front_matter: fm,
        body: vec![
            ConceptLine::Blank,
            ConceptLine::Comment("# concepts".to_string()),
            ConceptLine::Blank,
        ],
    };

    write_codelist(&cl, &args.file)?;
    println!("Created {}", args.file.display());

    if !args.no_edit {
        if let Ok(editor) = std::env::var("EDITOR").or_else(|_| std::env::var("VISUAL")) {
            let _ = std::process::Command::new(&editor)
                .arg(&args.file)
                .status();
        }
    }

    Ok(())
}

fn cmd_add(args: AddArgs) -> Result<()> {
    if args.sctids.is_empty() {
        bail!("provide at least one SCTID");
    }

    let conn = open_db(&args.db)?;
    let mut cl = read_codelist(&args.file)?;

    // Collect existing active IDs to deduplicate.
    let existing: HashSet<String> = cl.body.iter()
        .filter_map(|l| if l.is_active() { l.sctid().map(String::from) } else { None })
        .collect();

    let mut all_ids: Vec<String> = args.sctids.clone();

    if args.include_descendants {
        for sctid in &args.sctids {
            all_ids.extend(get_all_descendants(&conn, sctid)?);
        }
        all_ids.sort();
        all_ids.dedup();
    }

    let mut added = 0usize;
    for id in &all_ids {
        if existing.contains(id) {
            continue;
        }
        let term = lookup_preferred_term(&conn, id)
            .with_context(|| format!("SCTID {} not found in {}", id, args.db.display()))?;

        cl.body.push(ConceptLine::Active {
            id: id.clone(),
            term,
            comment: args.comment.clone(),
        });
        added += 1;
    }

    if added == 0 {
        println!("No new concepts to add (all already present).");
        return Ok(());
    }

    cl.front_matter.updated = today();
    cl.front_matter.version += 1;
    write_codelist(&cl, &args.file)?;
    println!("Added {added} concept(s) to {}", args.file.display());
    Ok(())
}

fn cmd_remove(args: RemoveArgs) -> Result<()> {
    let mut cl = read_codelist(&args.file)?;
    let mut found = false;

    for line in &mut cl.body {
        if let ConceptLine::Active { id, term, .. } = line {
            if *id == args.sctid {
                let comment = args.comment.clone();
                *line = ConceptLine::Excluded {
                    id: id.clone(),
                    term: term.clone(),
                    comment,
                };
                found = true;
                break;
            }
        }
    }

    if !found {
        bail!("SCTID {} not found as an active concept in {}", args.sctid, args.file.display());
    }

    cl.front_matter.updated = today();
    cl.front_matter.version += 1;
    write_codelist(&cl, &args.file)?;
    println!("Moved {} to excluded in {}", args.sctid, args.file.display());
    Ok(())
}

fn cmd_validate(args: ValidateArgs) -> Result<()> {
    let cl = read_codelist(&args.file)?;
    let conn = open_db(&args.db)?;

    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // Check required fields.
    let fm = &cl.front_matter;
    for (field, val) in [
        ("appropriate_use", fm.appropriate_use.as_str()),
        ("misuse", fm.misuse.as_str()),
        ("licence", fm.licence.as_str()),
    ] {
        if val.trim().is_empty() || val.starts_with("Describe") {
            if fm.status == "published" {
                errors.push(format!("published codelist must have a non-empty `{field}`"));
            } else {
                warnings.push(format!("`{field}` is a placeholder — fill in before publishing"));
            }
        }
    }

    if fm.status == "published" && fm.signoffs.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
        errors.push("published codelist must have at least one signoff".to_string());
    }

    // Check for duplicate SCTIDs.
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for line in &cl.body {
        if let Some(id) = line.sctid() {
            *seen.entry(id).or_insert(0) += 1;
        }
    }
    for (id, count) in &seen {
        if *count > 1 {
            errors.push(format!("SCTID {id} appears {count} times"));
        }
    }

    // Check active concepts against the database.
    for line in &cl.body {
        match line {
            ConceptLine::Active { id, term, .. } => {
                match lookup_concept_row(&conn, id)? {
                    None => errors.push(format!("SCTID {id} not found in database")),
                    Some((db_term, active)) => {
                        if !active {
                            errors.push(format!("SCTID {id} is inactive in database ({db_term})"));
                        } else if db_term != *term {
                            warnings.push(format!(
                                "SCTID {id}: stored term {term:?} differs from database {db_term:?}"
                            ));
                        }
                    }
                }
            }
            ConceptLine::PendingReview { id, term } => {
                warnings.push(format!("SCTID {id} ({term}) is pending review"));
            }
            _ => {}
        }
    }

    // Print results.
    let has_errors = !errors.is_empty();

    for w in &warnings {
        eprintln!("WARN  {w}");
    }
    for e in &errors {
        eprintln!("ERROR {e}");
    }

    let active_count = cl.body.iter().filter(|l| l.is_active()).count();
    println!(
        "\n{}: {} active concepts, {} warning(s), {} error(s)",
        args.file.display(),
        active_count,
        warnings.len(),
        errors.len(),
    );

    if has_errors {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_stats(args: StatsArgs) -> Result<()> {
    let cl = read_codelist(&args.file)?;
    let conn = open_db(&args.db)?;

    let fm = &cl.front_matter;
    println!("File:        {}", args.file.display());
    println!("Title:       {}", fm.title);
    println!("Terminology: {}", fm.terminology);
    println!("Version:     {}", fm.version);
    println!("Status:      {}", fm.status);
    println!("Updated:     {}", fm.updated);

    let active: Vec<&str> = cl.body.iter()
        .filter_map(|l| if l.is_active() { l.sctid() } else { None })
        .collect();
    let excluded: Vec<&str> = cl.body.iter()
        .filter_map(|l| if matches!(l, ConceptLine::Excluded { .. }) { l.sctid() } else { None })
        .collect();
    let pending: Vec<&str> = cl.body.iter()
        .filter_map(|l| if matches!(l, ConceptLine::PendingReview { .. }) { l.sctid() } else { None })
        .collect();

    println!("\nConcept counts:");
    println!("  Active:         {}", active.len());
    println!("  Excluded:       {}", excluded.len());
    println!("  Pending review: {}", pending.len());

    // Hierarchy breakdown.
    let mut by_hierarchy: HashMap<String, usize> = HashMap::new();
    let mut leaf_count = 0usize;
    let mut intermediate_count = 0usize;

    for id in &active {
        if let Some((hierarchy, children_count)) = lookup_hierarchy_and_children(&conn, id)? {
            *by_hierarchy.entry(hierarchy).or_insert(0) += 1;
            if children_count == 0 {
                leaf_count += 1;
            } else {
                intermediate_count += 1;
            }
        }
    }

    if !by_hierarchy.is_empty() {
        println!("\nBy hierarchy:");
        let mut sorted: Vec<_> = by_hierarchy.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (h, n) in sorted {
            println!("  {:<40} {}", h, n);
        }
        if !active.is_empty() {
            println!("\nLeaf nodes:         {} ({:.0}%)", leaf_count, 100.0 * leaf_count as f64 / active.len() as f64);
            println!("Intermediate nodes: {} ({:.0}%)", intermediate_count, 100.0 * intermediate_count as f64 / active.len() as f64);
        }
    }

    if let Some(release) = &fm.snomed_release {
        if let Ok(release_date) = chrono::NaiveDate::parse_from_str(release, "%Y%m%d")
            .or_else(|_| chrono::NaiveDate::parse_from_str(release, "%Y-%m-%d"))
        {
            let today = Local::now().date_naive();
            let age_days = (today - release_date).num_days();
            println!("\nSNOMED release: {} ({} days ago)", release, age_days);
            if age_days > 365 {
                println!("  ⚠ Release is more than 12 months old — consider rebuilding");
            }
        }
    }

    Ok(())
}

fn cmd_diff(args: DiffArgs) -> Result<()> {
    let a = read_codelist(&args.file_a)?;
    let b = read_codelist(&args.file_b)?;

    let a_active: HashMap<String, String> = a.body.iter()
        .filter_map(|l| if let ConceptLine::Active { id, term, .. } = l {
            Some((id.clone(), term.clone()))
        } else { None })
        .collect();

    let b_active: HashMap<String, String> = b.body.iter()
        .filter_map(|l| if let ConceptLine::Active { id, term, .. } = l {
            Some((id.clone(), term.clone()))
        } else { None })
        .collect();

    let b_excluded: HashSet<String> = b.body.iter()
        .filter_map(|l| if matches!(l, ConceptLine::Excluded { .. }) { l.sctid().map(String::from) } else { None })
        .collect();

    let mut added: Vec<(&str, &str)> = Vec::new();
    let mut removed: Vec<(&str, &str)> = Vec::new();
    let mut excluded: Vec<(&str, &str)> = Vec::new();
    let mut term_changed: Vec<(&str, &str, &str)> = Vec::new();

    for (id, term) in &b_active {
        if !a_active.contains_key(id.as_str()) {
            added.push((id, term));
        }
    }
    for (id, term) in &a_active {
        if !b_active.contains_key(id.as_str()) {
            if b_excluded.contains(id.as_str()) {
                excluded.push((id, term));
            } else {
                removed.push((id, term));
            }
        } else if let Some(b_term) = b_active.get(id.as_str()) {
            if b_term != term {
                term_changed.push((id, term, b_term));
            }
        }
    }

    added.sort_by_key(|(id, _)| *id);
    removed.sort_by_key(|(id, _)| *id);
    excluded.sort_by_key(|(id, _)| *id);
    term_changed.sort_by_key(|(id, _, _)| *id);

    println!("--- {}", args.file_a.display());
    println!("+++ {}", args.file_b.display());
    println!();

    if added.is_empty() && removed.is_empty() && excluded.is_empty() && term_changed.is_empty() {
        println!("No differences found.");
        return Ok(());
    }

    if !added.is_empty() {
        println!("Added ({}):", added.len());
        for (id, term) in &added {
            println!("  + {id:<14} {term}");
        }
        println!();
    }
    if !removed.is_empty() {
        println!("Removed ({}):", removed.len());
        for (id, term) in &removed {
            println!("  - {id:<14} {term}");
        }
        println!();
    }
    if !excluded.is_empty() {
        println!("Moved to excluded ({}):", excluded.len());
        for (id, term) in &excluded {
            println!("  ~ {id:<14} {term}");
        }
        println!();
    }
    if !term_changed.is_empty() {
        println!("Preferred term changed ({}):", term_changed.len());
        for (id, old_term, new_term) in &term_changed {
            println!("  {id}:");
            println!("    - {old_term}");
            println!("    + {new_term}");
        }
        println!();
    }

    Ok(())
}

fn cmd_export(args: ExportArgs) -> Result<()> {
    let cl = read_codelist(&args.file)?;
    let active: Vec<(&str, &str)> = cl.body.iter()
        .filter_map(|l| if let ConceptLine::Active { id, term, .. } = l {
            Some((id.as_str(), term.as_str()))
        } else { None })
        .collect();

    let output = match args.format.as_str() {
        "csv" => export_csv(&active),
        "markdown" => export_markdown(&cl.front_matter, &active),
        "opencodelists-csv" => export_opencodelists_csv(&active),
        other => bail!("unsupported export format: {other}\nSupported: csv, opencodelists-csv, markdown"),
    };

    match args.output {
        Some(path) => {
            std::fs::write(&path, &output)
                .with_context(|| format!("writing {}", path.display()))?;
            println!("Exported {} concept(s) to {}", active.len(), path.display());
        }
        None => print!("{}", output),
    }
    Ok(())
}

fn export_csv(active: &[(&str, &str)]) -> String {
    let mut out = String::from("sctid,preferred_term\n");
    for (id, term) in active {
        out.push_str(&format!("{},{}\n", id, csv_escape(term)));
    }
    out
}

fn export_opencodelists_csv(active: &[(&str, &str)]) -> String {
    let mut out = String::from("code,term\n");
    for (id, term) in active {
        out.push_str(&format!("{},{}\n", id, csv_escape(term)));
    }
    out
}

fn export_markdown(fm: &FrontMatter, active: &[(&str, &str)]) -> String {
    let mut out = format!("# {}\n\n", fm.title);
    out.push_str(&format!("**Description:** {}\n\n", fm.description));
    out.push_str(&format!(
        "**Terminology:** {} | **Version:** {} | **Status:** {} | **Updated:** {}\n\n",
        fm.terminology, fm.version, fm.status, fm.updated
    ));
    out.push_str("| SCTID | Preferred Term |\n|---|---|\n");
    for (id, term) in active {
        out.push_str(&format!("| `{id}` | {term} |\n"));
    }
    out
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Database helpers
// ---------------------------------------------------------------------------

fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("opening database {}", path.display()))?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    Ok(conn)
}

fn lookup_preferred_term(conn: &Connection, id: &str) -> Result<String> {
    conn.query_row(
        "SELECT preferred_term FROM concepts WHERE id = ?1 AND active = 1",
        params![id],
        |row| row.get(0),
    )
    .with_context(|| format!("SCTID {id} not found or inactive"))
}

fn lookup_concept_row(conn: &Connection, id: &str) -> Result<Option<(String, bool)>> {
    match conn.query_row(
        "SELECT preferred_term, active FROM concepts WHERE id = ?1",
        params![id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
    ) {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn lookup_hierarchy_and_children(conn: &Connection, id: &str) -> Result<Option<(String, i64)>> {
    match conn.query_row(
        "SELECT hierarchy, children_count FROM concepts WHERE id = ?1",
        params![id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    ) {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn get_all_descendants(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE desc(id) AS (
             SELECT child_id FROM concept_isa WHERE parent_id = ?1
             UNION ALL
             SELECT ci.child_id FROM concept_isa ci JOIN desc d ON ci.parent_id = d.id
         )
         SELECT DISTINCT d.id FROM desc d
         JOIN concepts c ON c.id = d.id
         WHERE c.active = 1",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![id], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(ids)
}
