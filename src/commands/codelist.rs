// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct codelist` - Build, validate, and manage clinical code lists.
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
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::codelist::{parse_body_line, split_term_comment};
pub use crate::codelist::{
    parse_codelist, parse_include_ref, read_codelist, render_codelist, resolve_include_path,
    write_codelist, Author, CodelistFile, ConceptLine, EffectiveMember, FrontMatter, IncludeRef,
    MemberSource, Warning,
};
use crate::humanize::plural_count;

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
    /// Add or remove `includes:` references to compose other codelists.
    Include(IncludeArgs),
    /// Flatten a composed codelist into a standalone snapshot (all members inline).
    Resolve(ResolveArgs),
    /// Interactive FTS5 search → include/exclude concepts (requires --db).
    Search(SearchArgs),
    /// Import a codelist from OpenCodelists, CSV, or FHIR.
    Import(ImportArgs),
}

#[derive(Parser, Debug)]
pub struct NewArgs {
    /// Path for the new .codelist file.
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
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
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file: PathBuf,
    /// One or more SCTIDs to add. Use `-` to read newline-delimited SCTIDs from
    /// stdin, e.g. `sct ecl expand "<<73211009" | sct codelist add list.codelist -`.
    pub sctids: Vec<String>,
    /// SNOMED CT SQLite database. See `docs/path-resolution.md` for the
    /// discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,
    /// Add every concept matched by an ECL expression, e.g. `--ecl "<<73211009"`.
    /// Mutually exclusive with positional SCTIDs. See `docs/commands/codelist.md`.
    #[arg(long, conflicts_with_all = ["sctids", "include_descendants"])]
    pub ecl: Option<String>,
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
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
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
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file: PathBuf,
    /// SNOMED CT SQLite database. See `docs/path-resolution.md` for the
    /// discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,
    /// Registry directory bare-id `includes:` entries resolve against
    /// (default `./codelists`, or `$SCT_CODELISTS` / `[codelists] dir`).
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub codelists: Option<PathBuf>,
    /// Re-fetch URL includes instead of using the local cache.
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Parser, Debug)]
pub struct StatsArgs {
    /// Path to the .codelist file.
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file: PathBuf,
    /// SNOMED CT SQLite database. See `docs/path-resolution.md` for the
    /// discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,
    /// Registry directory bare-id `includes:` entries resolve against.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub codelists: Option<PathBuf>,
}

#[derive(Parser, Debug)]
pub struct DiffArgs {
    /// First .codelist file.
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file_a: PathBuf,
    /// Second .codelist file.
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file_b: PathBuf,
    /// Registry directory bare-id `includes:` entries resolve against.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub codelists: Option<PathBuf>,
}

#[derive(Parser, Debug)]
pub struct ExportArgs {
    /// Path to the .codelist file.
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file: PathBuf,
    /// Output format: csv (default), opencodelists-csv, markdown, or fhir-json
    /// (a FHIR R4 ValueSet resource). RF2 is deferred; see issue #60.
    #[arg(long, default_value = "csv")]
    pub format: String,
    /// Write to file instead of stdout.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub output: Option<PathBuf>,
    /// Canonical base URL for `--format fhir-json`. The ValueSet's `url` becomes
    /// `<URL>/ValueSet/<id>`, matching how `sct serve` publishes it. When unset,
    /// the codelist's `opencodelists_url` is used if present, otherwise `url` is
    /// omitted (it is optional in FHIR).
    #[arg(long)]
    pub url: Option<String>,
    /// Comma-separated list of crosswalk terminologies to append as extra columns:
    /// `ctv3`, `read2` (any DB), and `icd10`, `opcs4` (need `sct ndjson --refsets all`).
    /// Requires `--db`. Multiple codes per SCTID in one terminology are joined with
    /// `|`. Not supported for `opencodelists-csv`.
    #[arg(long, value_delimiter = ',')]
    pub include_maps: Vec<String>,
    /// SNOMED CT SQLite database (required when `--include-maps` is set).
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,
    /// Registry directory bare-id `includes:` entries resolve against.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub codelists: Option<PathBuf>,
}

#[derive(Parser, Debug)]
pub struct IncludeArgs {
    /// Path to the .codelist file to add includes to.
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file: PathBuf,
    /// Codelist references to include: a bare id, a relative path, or a URL.
    pub refs: Vec<String>,
    /// Remove the given references instead of adding them.
    #[arg(long)]
    pub remove: bool,
    /// Registry directory bare-id references resolve against (for validation).
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub codelists: Option<PathBuf>,
}

#[derive(Parser, Debug)]
pub struct ResolveArgs {
    /// Path to the .codelist file to flatten.
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file: PathBuf,
    /// Write the flattened codelist here (default: stdout).
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub output: Option<PathBuf>,
    /// Registry directory bare-id `includes:` entries resolve against.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub codelists: Option<PathBuf>,
    /// Re-fetch URL includes instead of using the local cache.
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Parser, Debug)]
pub struct SearchArgs {
    /// Path to the .codelist file.
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file: PathBuf,
    /// Search query.
    pub query: String,
    /// SNOMED CT SQLite database. See `docs/path-resolution.md` for the
    /// discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,
    /// Maximum number of matching concepts to show.
    #[arg(long, short, default_value_t = 20)]
    pub limit: u32,
}

#[derive(Parser, Debug)]
pub struct ImportArgs {
    /// Path for the new .codelist file (must not already exist).
    #[arg(value_parser = crate::paths::tilde_pathbuf)]
    pub file: PathBuf,
    /// Source type: csv, opencodelists-csv (or opencodelists), fhir-json, or rf2.
    #[arg(long)]
    pub from: String,
    /// URL or file path of the source. Use `-` to read from stdin.
    pub source: String,
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
        Verb::Include(a) => cmd_include(a),
        Verb::Resolve(a) => cmd_resolve(a),
        Verb::Search(a) => cmd_search(a),
        Verb::Import(a) => cmd_import(a),
    }
}

pub fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// Composition (includes)
// ---------------------------------------------------------------------------

/// Compute the effective active member set of a codelist: its own `Active`
/// concepts plus, recursively, the effective members of every `includes:`
/// entry, minus this file's own `Excluded` concepts (a parent exclusion
/// overrides an inherited inclusion). `PendingReview` lines are never members.
///
/// `including_file_dir` is the directory of `cl`'s own file (for relative path
/// refs); `registry` is the directory bare-id refs resolve against. `visited`
/// carries the set of already-entered canonical file paths for cycle detection;
/// pass a fresh `HashSet` at the top level. Order is preserved: included members
/// first (in `includes:` then body order), then this file's own direct members
/// in body order - so a list with no `includes:` yields exactly its body order.
pub fn resolve_effective_members(
    cl: &CodelistFile,
    including_file_dir: &Path,
    registry: &Path,
    refresh: bool,
    visited: &mut HashSet<PathBuf>,
) -> Result<Vec<EffectiveMember>> {
    crate::codelist::resolve_effective_members_with_resolver(
        cl,
        including_file_dir,
        registry,
        refresh,
        visited,
        &mut fetch_url_codelist,
    )
}

/// Convenience: resolve a file's effective members, deriving the including
/// directory from the file path and using `registry` for bare-id refs. When
/// `refresh` is set, URL includes are re-fetched rather than read from cache.
pub fn effective_members_of(
    cl: &CodelistFile,
    file: &Path,
    registry: &Path,
    refresh: bool,
) -> Result<Vec<EffectiveMember>> {
    crate::codelist::effective_members_of_with_resolver(
        cl,
        file,
        registry,
        refresh,
        fetch_url_codelist,
    )
}

/// Fetch a remote `.codelist` into the local cache and return its path. Uses the
/// cached copy unless `refresh` is set or it is absent. The cache lives under
/// `$SCT_DATA_HOME/cache/codelists/` keyed by a hash of the URL.
fn fetch_url_codelist(url: &str, refresh: bool) -> Result<PathBuf> {
    let cache_dir = crate::paths::data_home().join("cache").join("codelists");
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("creating cache dir {}", cache_dir.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    // sha2 0.11's finalize() output type dropped its LowerHex impl; format
    // byte-by-byte instead of relying on the hasher's return type.
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let cached = cache_dir.join(format!("{hex}.codelist"));
    if refresh || !cached.exists() {
        let body = ureq::get(url)
            .call()
            .with_context(|| format!("fetching {url}"))?
            .into_body()
            .read_to_string()
            .with_context(|| format!("reading body of {url}"))?;
        // Validate it parses as a codelist before caching.
        parse_codelist(&body).with_context(|| format!("parsing remote codelist {url}"))?;
        std::fs::write(&cached, body)
            .with_context(|| format!("caching {url} to {}", cached.display()))?;
    }
    Ok(cached)
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
            .replace(['-', '_'], " ")
    });

    let id = args
        .file
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
            message: "Validate against the current SNOMED release before use in research."
                .to_string(),
        });
    }

    if args.terminology == "dm+d" {
        warnings.push(Warning {
            code: "dmd-currency".to_string(),
            severity: "warning".to_string(),
            message: "dm+d codes change frequently. Check VMP code changes since snomed_release."
                .to_string(),
        });
        warnings.push(Warning {
            code: "dmd-vmp-code-change".to_string(),
            severity: "caution".to_string(),
            message: "VMP codes may have been superseded. Validate against current dm+d release."
                .to_string(),
        });
    }

    let authors = args.author.map(|name| {
        vec![Author {
            name,
            orcid: None,
            affiliation: None,
            role: Some("author".to_string()),
        }]
    });

    let fm = FrontMatter {
        id,
        title: title.clone(),
        description: args
            .description
            .unwrap_or_else(|| format!("{} codes", title)),
        terminology: args.terminology,
        created: today.clone(),
        updated: today,
        version: 1,
        status: "draft".to_string(),
        licence: "CC-BY-4.0".to_string(),
        copyright:
            "Copyright holder. SNOMED CT content © IHTSDO, used under NHS England national licence."
                .to_string(),
        appropriate_use: "Describe appropriate use here.".to_string(),
        misuse: "Describe misuse here.".to_string(),
        includes: None,
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
            let _ = std::process::Command::new(&editor).arg(&args.file).status();
        }
    }

    Ok(())
}

fn cmd_add(args: AddArgs) -> Result<()> {
    if args.sctids.is_empty() && args.ecl.is_none() {
        bail!("provide at least one SCTID, or an ECL expression with --ecl");
    }

    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = open_db(&db)?;
    let mut cl = read_codelist(&args.file)?;
    let parsed_ecl = args
        .ecl
        .as_deref()
        .map(|ecl| crate::ecl::parse(ecl).with_context(|| format!("parsing ECL {ecl:?}")))
        .transpose()?;
    let needs_transitive_query = args.include_descendants
        || parsed_ecl
            .as_ref()
            .is_some_and(crate::ecl::eval::uses_transitive_hierarchy);
    let _snapshot = needs_transitive_query
        .then(|| crate::ecl::eval::ReadSnapshot::begin(&conn))
        .transpose()?;
    let tct = if needs_transitive_query {
        Some(crate::ecl::warn_if_tct_unusable(
            &conn,
            "transitive codelist hierarchy expansion",
        )?)
    } else {
        None
    };

    // Auto-populate snomed_release from the DB's provenance the first time
    // we touch this codelist with a real DB. Don't overwrite an existing
    // value - the user may have set it deliberately to a different release.
    if cl.front_matter.snomed_release.is_none() {
        if let Ok(Some(p)) = crate::provenance::read_sqlite(&conn) {
            if !p.release_date.is_empty() {
                cl.front_matter.snomed_release = Some(p.release_date.clone());
            }
        }
    }

    // Collect existing active IDs to deduplicate.
    let mut existing: HashSet<String> = cl
        .body
        .iter()
        .filter_map(|l| {
            if l.is_active() {
                l.sctid().map(String::from)
            } else {
                None
            }
        })
        .collect();

    let mut all_ids: Vec<String> = if let Some(ecl) = &args.ecl {
        let parsed = parsed_ecl.as_ref().expect("ECL was parsed above");
        let ids: Vec<String> = match tct {
            Some(usable) => crate::ecl::eval::evaluate_with_tct(&conn, parsed, usable),
            None => crate::ecl::eval::evaluate(&conn, parsed),
        }
        .with_context(|| format!("expanding ECL {ecl:?}"))?
        .into_iter()
        .map(|id| id.to_string())
        .collect();
        if ids.is_empty() {
            println!("ECL {ecl:?} matched no concepts.");
            return Ok(());
        }
        println!(
            "ECL {ecl:?} matched {}.",
            plural_count(ids.len() as u64, "concept")
        );
        ids
    } else {
        // Explicit SCTIDs, plus any read from stdin when `-` is given. This is
        // what makes `sct ecl expand … | sct codelist add <file> -` work.
        let mut ids: Vec<String> = args.sctids.iter().filter(|s| *s != "-").cloned().collect();
        if args.sctids.iter().any(|s| s == "-") {
            ids.extend(read_sctids_from_stdin()?);
        }
        ids
    };

    if args.include_descendants {
        let roots = all_ids.clone();
        let mut expanded: HashSet<String> = all_ids.into_iter().collect();
        for sctid in &roots {
            expanded.extend(get_all_descendants_with_tct(
                &conn,
                sctid,
                tct.expect("descendant expansion requires TCT status"),
            )?);
        }
        all_ids = expanded.into_iter().collect();
        all_ids.sort();
    }

    let mut added = 0usize;
    for id in &all_ids {
        if existing.contains(id) {
            continue;
        }
        let term = lookup_preferred_term(&conn, id)
            .with_context(|| format!("SCTID {} not found in {}", id, db.display()))?;

        cl.body.push(ConceptLine::Active {
            id: id.clone(),
            term,
            comment: args.comment.clone(),
        });
        existing.insert(id.clone());
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchResult {
    id: String,
    term: String,
    hierarchy: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchChoice {
    Include,
    Exclude,
}

/// Search active concepts and record explicitly reviewed decisions. This is
/// terminal-only so a stray stdin pipe cannot modify a clinical codelist.
fn cmd_search(args: SearchArgs) -> Result<()> {
    use std::io::{IsTerminal, Write};

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!(
            "`sct codelist search` requires an interactive terminal. Use `sct lexical --ids | sct codelist add` in scripts."
        );
    }

    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = open_db(&db)?;
    let results = search_codelist_concepts(&conn, &args.query, args.limit)?;
    if results.is_empty() {
        eprintln!("No results for {:?}.", args.query);
        return Ok(());
    }

    println!("Results for {:?}:", args.query);
    for (index, result) in results.iter().enumerate() {
        println!(
            "  {:>2}. {} | {} | {}",
            index + 1,
            result.id,
            result.term,
            result.hierarchy
        );
    }
    print!("\nSelect numbers to include; prefix a number with - to exclude (for example, 1,3,-4). Press Enter to cancel: ");
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("reading selection")?;
    let choices = parse_search_choices(&input, results.len())?;
    if choices.is_empty() {
        println!("No changes.");
        return Ok(());
    }

    let mut codelist = read_codelist(&args.file)?;
    let changed = apply_search_choices(&mut codelist, &results, &choices);
    if changed == 0 {
        println!("No changes (the selected decisions were already recorded).");
        return Ok(());
    }

    set_snomed_release_if_missing(&mut codelist, &conn);
    codelist.front_matter.updated = today();
    codelist.front_matter.version += 1;
    write_codelist(&codelist, &args.file)?;
    println!(
        "Recorded {changed} reviewed decision(s) in {}.",
        args.file.display()
    );
    Ok(())
}

fn search_codelist_concepts(
    conn: &Connection,
    query: &str,
    limit: u32,
) -> Result<Vec<SearchResult>> {
    let fts_query = sanitise_fts_query(query);
    let mut stmt = conn.prepare(
        "SELECT c.id, c.preferred_term, c.hierarchy
         FROM concepts_fts
         JOIN concepts c ON concepts_fts.rowid = c.rowid
         WHERE concepts_fts MATCH ?1 AND c.active = 1
         ORDER BY rank
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![fts_query, limit], |row| {
        Ok(SearchResult {
            id: row.get(0)?,
            term: row.get(1)?,
            hierarchy: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn sanitise_fts_query(query: &str) -> String {
    let has_operators = query.contains('"')
        || query.contains('*')
        || query.contains('^')
        || query.to_uppercase().contains(" AND ")
        || query.to_uppercase().contains(" OR ")
        || query.to_uppercase().contains(" NOT ");
    if has_operators {
        query.to_string()
    } else {
        format!("\"{}\"", query.replace('"', "\"\""))
    }
}

fn parse_search_choices(input: &str, result_count: usize) -> Result<Vec<(usize, SearchChoice)>> {
    let mut choices = Vec::new();
    let mut seen = HashSet::new();
    for token in input.split(|c: char| c == ',' || c.is_whitespace()) {
        if token.is_empty() {
            continue;
        }
        let (choice, number) = match token.strip_prefix('-') {
            Some(number) => (SearchChoice::Exclude, number),
            None => (SearchChoice::Include, token),
        };
        let index = number.parse::<usize>().with_context(|| {
            format!("invalid selection {token:?}; use a result number such as 1 or -2")
        })?;
        if index == 0 || index > result_count {
            bail!("selection {token:?} is outside the displayed result range 1..={result_count}");
        }
        if !seen.insert(index) {
            bail!("result {index} was selected more than once");
        }
        choices.push((index - 1, choice));
    }
    Ok(choices)
}

fn apply_search_choices(
    codelist: &mut CodelistFile,
    results: &[SearchResult],
    choices: &[(usize, SearchChoice)],
) -> usize {
    let mut changed = 0;
    for &(index, choice) in choices {
        let result = &results[index];
        let existing = codelist
            .body
            .iter()
            .position(|line| line.sctid() == Some(result.id.as_str()));
        match (existing, choice) {
            (Some(position), SearchChoice::Include) => match &codelist.body[position] {
                ConceptLine::Active { .. } => {}
                ConceptLine::Excluded { comment, .. } => {
                    let comment = comment.clone();
                    codelist.body[position] = ConceptLine::Active {
                        id: result.id.clone(),
                        term: result.term.clone(),
                        comment,
                    };
                    changed += 1;
                }
                ConceptLine::PendingReview { .. } => {
                    codelist.body[position] = ConceptLine::Active {
                        id: result.id.clone(),
                        term: result.term.clone(),
                        comment: None,
                    };
                    changed += 1;
                }
                _ => unreachable!("sctid-bearing lines are active, excluded, or pending"),
            },
            (Some(position), SearchChoice::Exclude) => match &codelist.body[position] {
                ConceptLine::Excluded { .. } => {}
                ConceptLine::Active { comment, .. } => {
                    let comment = comment.clone();
                    codelist.body[position] = ConceptLine::Excluded {
                        id: result.id.clone(),
                        term: result.term.clone(),
                        comment,
                    };
                    changed += 1;
                }
                ConceptLine::PendingReview { .. } => {
                    codelist.body[position] = ConceptLine::Excluded {
                        id: result.id.clone(),
                        term: result.term.clone(),
                        comment: None,
                    };
                    changed += 1;
                }
                _ => unreachable!("sctid-bearing lines are active, excluded, or pending"),
            },
            (None, SearchChoice::Include) => {
                codelist.body.push(ConceptLine::Active {
                    id: result.id.clone(),
                    term: result.term.clone(),
                    comment: None,
                });
                changed += 1;
            }
            (None, SearchChoice::Exclude) => {
                codelist.body.push(ConceptLine::Excluded {
                    id: result.id.clone(),
                    term: result.term.clone(),
                    comment: None,
                });
                changed += 1;
            }
        }
    }
    changed
}

fn set_snomed_release_if_missing(codelist: &mut CodelistFile, conn: &Connection) {
    if codelist.front_matter.snomed_release.is_none() {
        if let Ok(Some(provenance)) = crate::provenance::read_sqlite(conn) {
            if !provenance.release_date.is_empty() {
                codelist.front_matter.snomed_release = Some(provenance.release_date);
            }
        }
    }
}

#[derive(Debug, Default)]
struct ImportPayload {
    included: Vec<(String, String)>,
    excluded: Vec<(String, String)>,
    title: Option<String>,
    description: Option<String>,
    copyright: Option<String>,
    source_url: Option<String>,
    source_version: Option<String>,
    source_status: Option<String>,
}

fn cmd_import(args: ImportArgs) -> Result<()> {
    if args.file.exists() {
        bail!(
            "{} already exists; import creates a new codelist and will not overwrite it",
            args.file.display()
        );
    }

    let format = args.from.trim().to_ascii_lowercase();
    if format == "rf2" {
        bail!(
            "`rf2` codelist import is not implemented. RF2 reference sets need namespace, refsetId, moduleId, and member-identity decisions that are still open. See https://github.com/pacharanero/sct/issues/60 to follow the design or request that it be expedited. Supported today: csv, opencodelists-csv, fhir-json."
        );
    }

    let source_text = read_import_source(&args.source)?;
    let payload = match format.as_str() {
        "csv" => parse_import_csv(&source_text, "sctid", "preferred_term")?,
        "opencodelists" | "opencodelists-csv" => {
            parse_import_csv(&source_text, "code", "term")?
        }
        "fhir" | "fhir-json" => parse_import_fhir(&source_text)?,
        other => bail!(
            "unsupported import format: {other}\nSupported: csv, opencodelists-csv, fhir-json. RF2 is deferred; see https://github.com/pacharanero/sct/issues/60"
        ),
    };

    if payload.included.is_empty() && payload.excluded.is_empty() {
        bail!("the source contains no explicit SNOMED CT concepts");
    }

    let codelist = build_imported_codelist(&args.file, &args.source, &format, payload)?;
    if let Some(parent) = args.file.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    let included = codelist.body.iter().filter(|line| line.is_active()).count();
    let excluded = codelist
        .body
        .iter()
        .filter(|line| matches!(line, ConceptLine::Excluded { .. }))
        .count();
    write_codelist(&codelist, &args.file)?;
    println!(
        "Imported {included} included and {excluded} excluded concept(s) to {}",
        args.file.display()
    );
    Ok(())
}

fn read_import_source(source: &str) -> Result<String> {
    if source == "-" {
        use std::io::Read;
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .context("reading import source from stdin")?;
        return Ok(text);
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        let public_source = source_for_provenance(source);
        return ureq::get(source)
            .call()
            .with_context(|| format!("fetching {public_source}"))?
            .into_body()
            .read_to_string()
            .with_context(|| format!("reading body of {public_source}"));
    }
    let path = crate::paths::expand_tilde(source);
    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

fn parse_import_csv(text: &str, code_header: &str, term_header: &str) -> Result<ImportPayload> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers = reader.headers().context("reading CSV header")?.clone();
    let find_header = |wanted: &str| {
        headers.iter().position(|header| {
            header
                .trim_start_matches('\u{feff}')
                .trim()
                .eq_ignore_ascii_case(wanted)
        })
    };
    let code_index = find_header(code_header).with_context(|| {
        format!(
            "CSV is missing required `{code_header}` column (headers: {})",
            headers.iter().collect::<Vec<_>>().join(", ")
        )
    })?;
    let term_index = find_header(term_header).with_context(|| {
        format!(
            "CSV is missing required `{term_header}` column (headers: {})",
            headers.iter().collect::<Vec<_>>().join(", ")
        )
    })?;

    let mut concepts = indexmap::IndexMap::new();
    for (row_index, record) in reader.records().enumerate() {
        let record = record.with_context(|| format!("reading CSV row {}", row_index + 2))?;
        let code = record.get(code_index).unwrap_or("").trim();
        let term = record.get(term_index).unwrap_or("").trim();
        insert_import_concept(
            &mut concepts,
            code,
            term,
            &format!("CSV row {}", row_index + 2),
        )?;
    }
    Ok(ImportPayload {
        included: concepts.into_iter().collect(),
        ..ImportPayload::default()
    })
}

fn parse_import_fhir(text: &str) -> Result<ImportPayload> {
    let value: Value = serde_json::from_str(text).context("parsing FHIR ValueSet JSON")?;
    if value.get("resourceType").and_then(Value::as_str) != Some("ValueSet") {
        bail!("FHIR import requires a ValueSet resource (`resourceType`: `ValueSet`)");
    }
    let compose = value.get("compose").and_then(Value::as_object).context(
        "FHIR ValueSet has no `compose` object; expansion-only resources cannot be imported",
    )?;

    let mut included = parse_fhir_compose_groups(compose.get("include"), "include")?;
    let excluded = parse_fhir_compose_groups(compose.get("exclude"), "exclude")?;
    for code in excluded.keys() {
        included.shift_remove(code);
    }

    Ok(ImportPayload {
        included: included.into_iter().collect(),
        excluded: excluded.into_iter().collect(),
        title: value
            .get("title")
            .or_else(|| value.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        copyright: value
            .get("copyright")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_url: value.get("url").and_then(Value::as_str).map(str::to_string),
        source_version: value
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_status: value
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn parse_fhir_compose_groups(
    groups: Option<&Value>,
    group_name: &str,
) -> Result<indexmap::IndexMap<String, String>> {
    let mut concepts = indexmap::IndexMap::new();
    let Some(groups) = groups else {
        return Ok(concepts);
    };
    let groups = groups
        .as_array()
        .with_context(|| format!("FHIR `compose.{group_name}` must be an array"))?;
    for (group_index, group) in groups.iter().enumerate() {
        let group = group.as_object().with_context(|| {
            format!("FHIR `compose.{group_name}[{group_index}]` must be an object")
        })?;
        if group.get("filter").is_some_and(value_is_nonempty)
            || group.get("valueSet").is_some_and(value_is_nonempty)
        {
            bail!(
                "FHIR `compose.{group_name}[{group_index}]` uses filters or imported ValueSets. Only explicit SNOMED CT `concept` entries can be converted without changing meaning. Expand the ValueSet first, review it, then import an extensional ValueSet."
            );
        }
        let system = group
            .get("system")
            .and_then(Value::as_str)
            .with_context(|| {
                format!("FHIR `compose.{group_name}[{group_index}]` has no code system")
            })?;
        if system != SNOMED_SYSTEM {
            bail!(
                "FHIR `compose.{group_name}[{group_index}]` uses unsupported system `{system}`; codelist format v1 accepts SNOMED CT only"
            );
        }
        let entries = group
            .get("concept")
            .and_then(Value::as_array)
            .with_context(|| {
                format!(
                    "FHIR `compose.{group_name}[{group_index}]` has no explicit `concept` array"
                )
            })?;
        for (concept_index, concept) in entries.iter().enumerate() {
            let code = concept.get("code").and_then(Value::as_str).unwrap_or("");
            let term = concept
                .get("display")
                .and_then(Value::as_str)
                .unwrap_or(code);
            insert_import_concept(
                &mut concepts,
                code,
                term,
                &format!("FHIR compose.{group_name}[{group_index}].concept[{concept_index}]"),
            )?;
        }
    }
    Ok(concepts)
}

fn value_is_nonempty(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(items) => !items.is_empty(),
        Value::String(item) => !item.is_empty(),
        _ => true,
    }
}

fn insert_import_concept(
    concepts: &mut indexmap::IndexMap<String, String>,
    code: &str,
    term: &str,
    location: &str,
) -> Result<()> {
    if code.is_empty() || !code.chars().all(|character| character.is_ascii_digit()) {
        bail!("{location}: expected a numeric SNOMED CT code, got {code:?}");
    }
    if term.is_empty() {
        bail!("{location}: concept {code} has no term/display");
    }
    if let Some(existing) = concepts.get(code) {
        if existing != term {
            bail!("{location}: concept {code} has conflicting terms {existing:?} and {term:?}");
        }
        return Ok(());
    }
    concepts.insert(code.to_string(), term.to_string());
    Ok(())
}

fn build_imported_codelist(
    target: &Path,
    source: &str,
    format: &str,
    payload: ImportPayload,
) -> Result<CodelistFile> {
    let id = target
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .context("import target must have a filename")?
        .to_string();
    let default_title = id.replace(['-', '_'], " ");
    let title = payload.title.unwrap_or(default_title);
    let today = today();
    let public_source = source_for_provenance(source);
    let mut methodology =
        format!("Imported from {public_source} using sct codelist import --from {format}.");
    if let Some(url) = &payload.source_url {
        methodology.push_str(&format!(
            " Source canonical URL: {}.",
            source_for_provenance(url)
        ));
    }
    if let Some(version) = &payload.source_version {
        methodology.push_str(&format!(" Source version: {version}."));
    }
    if let Some(status) = &payload.source_status {
        methodology.push_str(&format!(" Source status: {status}."));
    }
    let mut body = vec![
        ConceptLine::Blank,
        ConceptLine::Comment("# concepts".to_string()),
        ConceptLine::Blank,
    ];
    body.extend(
        payload
            .included
            .into_iter()
            .map(|(id, term)| ConceptLine::Active {
                id,
                term,
                comment: None,
            }),
    );
    if !payload.excluded.is_empty() {
        body.push(ConceptLine::Blank);
        body.push(ConceptLine::Comment("# excluded in source".to_string()));
        body.push(ConceptLine::Blank);
        body.extend(
            payload
                .excluded
                .into_iter()
                .map(|(id, term)| ConceptLine::Excluded {
                    id,
                    term,
                    comment: Some("excluded in imported source".to_string()),
                }),
        );
    }

    Ok(CodelistFile {
        front_matter: FrontMatter {
            id,
            title: title.clone(),
            description: payload
                .description
                .unwrap_or_else(|| format!("Imported {title} codelist.")),
            terminology: "SNOMED CT".to_string(),
            created: today.clone(),
            updated: today,
            version: 1,
            status: "draft".to_string(),
            licence: "NOASSERTION".to_string(),
            copyright: payload.copyright.unwrap_or_else(|| {
                "Source copyright not supplied. SNOMED CT content © IHTSDO.".to_string()
            }),
            appropriate_use: "Review and describe appropriate use before publishing.".to_string(),
            misuse: "Do not use clinically until the imported concepts and source provenance have been reviewed.".to_string(),
            includes: None,
            snomed_release: None,
            authors: None,
            organisation: None,
            methodology: Some(methodology),
            signoffs: None,
            warnings: Some(vec![
                Warning {
                    code: "imported-needs-review".to_string(),
                    severity: "warning".to_string(),
                    message: "Imported concepts and metadata have not been clinically reviewed in this repository.".to_string(),
                },
                Warning {
                    code: "not-universal-definition".to_string(),
                    severity: "info".to_string(),
                    message: "This codelist may have been developed for a specific purpose and may not meet the needs of other studies.".to_string(),
                },
                Warning {
                    code: "snomed-release-age".to_string(),
                    severity: "caution".to_string(),
                    message: "Validate imported SCTIDs and terms against the intended SNOMED CT release.".to_string(),
                },
            ]),
            population: None,
            care_setting: None,
            tags: Some(vec!["imported".to_string()]),
            opencodelists_id: None,
            opencodelists_url: None,
        },
        body,
    })
}

fn source_for_provenance(source: &str) -> String {
    if !source.starts_with("http://") && !source.starts_with("https://") {
        return source.to_string();
    }
    let end = source.find(['?', '#']).unwrap_or(source.len());
    let mut public = source[..end].to_string();
    if let Some(scheme_end) = public.find("://") {
        let authority_start = scheme_end + 3;
        let authority_end = public[authority_start..]
            .find('/')
            .map(|offset| authority_start + offset)
            .unwrap_or(public.len());
        if let Some(at) = public[authority_start..authority_end].rfind('@') {
            public.replace_range(authority_start..authority_start + at + 1, "***@");
        }
    }
    public
}

/// Read newline-delimited SCTIDs from stdin (for `sct codelist add <file> -`).
fn read_sctids_from_stdin() -> Result<Vec<String>> {
    use std::io::Read;
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .context("reading SCTIDs from stdin")?;
    Ok(parse_sctid_lines(&s))
}

/// Parse SCTIDs from free-form lines: take the first whitespace token of each
/// non-empty, non-comment line. Tolerates `id` or `id  Some term` lines, and
/// `#`-prefixed comments - so the output of `sct ecl expand` (bare ids) and
/// loosely-formatted lists both work.
fn parse_sctid_lines(s: &str) -> Vec<String> {
    s.lines()
        .filter_map(|line| {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                return None;
            }
            t.split_whitespace().next().map(str::to_string)
        })
        .collect()
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
        bail!(
            "SCTID {} not found as an active concept in {}",
            args.sctid,
            args.file.display()
        );
    }

    cl.front_matter.updated = today();
    cl.front_matter.version += 1;
    write_codelist(&cl, &args.file)?;
    println!(
        "Moved {} to excluded in {}",
        args.sctid,
        args.file.display()
    );
    Ok(())
}

fn cmd_validate(args: ValidateArgs) -> Result<()> {
    let cl = read_codelist(&args.file)?;
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = open_db(&db)?;

    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // Check required fields.
    let fm = &cl.front_matter;
    for (field, val) in [
        ("appropriate_use", fm.appropriate_use.as_str()),
        ("misuse", fm.misuse.as_str()),
        ("licence", fm.licence.as_str()),
    ] {
        if val.trim().is_empty() || val.starts_with("Describe") || val == "NOASSERTION" {
            if fm.status == "published" {
                errors.push(format!(
                    "published codelist must have a non-empty `{field}`"
                ));
            } else {
                warnings.push(format!(
                    "`{field}` is a placeholder - fill in before publishing"
                ));
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

    // Validate that any `includes:` resolve (missing file, cycle, parse error).
    // The included lists' own concepts are validated by validating those files;
    // here we just ensure composition is sound and report the effective count.
    let registry = crate::paths::codelist_registry(args.codelists.as_deref());
    let effective = match effective_members_of(&cl, &args.file, &registry, args.refresh) {
        Ok(m) => Some(m),
        Err(e) => {
            errors.push(format!("includes do not resolve: {e:#}"));
            None
        }
    };

    // Check active concepts against the database.
    for line in &cl.body {
        match line {
            ConceptLine::Active { id, term, .. } => match lookup_concept_row(&conn, id)? {
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
            },
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

    let active_count = effective
        .as_ref()
        .map(|m| m.len())
        .unwrap_or_else(|| cl.body.iter().filter(|l| l.is_active()).count());
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
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = open_db(&db)?;

    let fm = &cl.front_matter;
    println!("File:        {}", args.file.display());
    println!("Title:       {}", fm.title);
    println!("Terminology: {}", fm.terminology);
    println!("Version:     {}", fm.version);
    println!("Status:      {}", fm.status);
    println!("Updated:     {}", fm.updated);

    // Effective active set (own + included, minus exclusions).
    let registry = crate::paths::codelist_registry(args.codelists.as_deref());
    let members = effective_members_of(&cl, &args.file, &registry, false)?;
    let active: Vec<&str> = members.iter().map(|m| m.id.as_str()).collect();
    let direct = members
        .iter()
        .filter(|m| m.source == MemberSource::Direct)
        .count();
    let inherited = members.len() - direct;
    let excluded: Vec<&str> = cl
        .body
        .iter()
        .filter_map(|l| {
            if matches!(l, ConceptLine::Excluded { .. }) {
                l.sctid()
            } else {
                None
            }
        })
        .collect();
    let pending: Vec<&str> = cl
        .body
        .iter()
        .filter_map(|l| {
            if matches!(l, ConceptLine::PendingReview { .. }) {
                l.sctid()
            } else {
                None
            }
        })
        .collect();

    if let Some(includes) = &fm.includes {
        if !includes.is_empty() {
            println!("\nIncludes ({}):", includes.len());
            for inc in includes {
                println!("  - {inc}");
            }
        }
    }

    println!("\nConcept counts:");
    if inherited > 0 {
        println!(
            "  Active:         {} ({} direct + {} inherited)",
            active.len(),
            direct,
            inherited
        );
    } else {
        println!("  Active:         {}", active.len());
    }
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
            println!(
                "\nLeaf nodes:         {} ({:.0}%)",
                leaf_count,
                100.0 * leaf_count as f64 / active.len() as f64
            );
            println!(
                "Intermediate nodes: {} ({:.0}%)",
                intermediate_count,
                100.0 * intermediate_count as f64 / active.len() as f64
            );
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
                println!("  ⚠ Release is more than 12 months old - consider rebuilding");
            }
        }
    }

    Ok(())
}

fn cmd_diff(args: DiffArgs) -> Result<()> {
    let a = read_codelist(&args.file_a)?;
    let b = read_codelist(&args.file_b)?;
    let registry = crate::paths::codelist_registry(args.codelists.as_deref());

    // Compare effective (composed) member sets so a diff reflects what each
    // list actually resolves to, including any `includes:`.
    let a_active: HashMap<String, String> =
        effective_members_of(&a, &args.file_a, &registry, false)?
            .into_iter()
            .map(|m| (m.id, m.term))
            .collect();
    let b_active: HashMap<String, String> =
        effective_members_of(&b, &args.file_b, &registry, false)?
            .into_iter()
            .map(|m| (m.id, m.term))
            .collect();

    let b_excluded: HashSet<String> = b
        .body
        .iter()
        .filter_map(|l| {
            if matches!(l, ConceptLine::Excluded { .. }) {
                l.sctid().map(String::from)
            } else {
                None
            }
        })
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

/// SNOMED CT code system URI, as used in FHIR resources.
pub const SNOMED_SYSTEM: &str = "http://snomed.info/sct";

/// Map a `.codelist` `status` onto the FHIR `ValueSet.status` required value set
/// (`draft` | `active` | `retired` | `unknown`). Unknown inputs map to `unknown`
/// rather than being rejected, so a lightly-populated list still exports.
pub fn fhir_status(status: &str) -> &'static str {
    match status {
        "draft" => "draft",
        "active" | "published" => "active",
        "retired" | "inactive" => "retired",
        _ => "unknown",
    }
}

/// Build a FHIR R4 `ValueSet` resource from a codelist's front-matter and its
/// effective members. When `include_concepts` is true the members are emitted as
/// an extensional `compose.include[0].concept[]` over SNOMED CT; otherwise a
/// metadata-only resource is returned. `canonical_url` sets `ValueSet.url` when
/// `Some` and omits it when `None` (the element is optional in FHIR).
///
/// This is the single source of truth for how `sct` renders a codelist as a
/// ValueSet: both `sct codelist export --format fhir-json` and the stored
/// ValueSets served by `sct serve` go through here, so the two never diverge.
pub fn fhir_valueset(
    fm: &FrontMatter,
    members: &[(&str, &str)],
    canonical_url: Option<&str>,
    include_concepts: bool,
) -> Value {
    let mut vs = json!({
        "resourceType": "ValueSet",
        "id": fm.id,
        "version": fm.version.to_string(),
        "name": fm.id,
        "title": fm.title,
        "status": fhir_status(&fm.status),
        "description": fm.description,
    });
    if let Some(url) = canonical_url {
        vs["url"] = json!(url);
    }
    if !fm.copyright.is_empty() {
        vs["copyright"] = json!(fm.copyright);
    }
    if include_concepts {
        let concepts: Vec<Value> = members
            .iter()
            .map(|(id, term)| json!({ "code": id, "display": term }))
            .collect();
        vs["compose"] = json!({
            "include": [ { "system": SNOMED_SYSTEM, "concept": concepts } ]
        });
    }
    vs
}

/// Render a codelist as a pretty-printed FHIR R4 `ValueSet` JSON document (with
/// a trailing newline). The canonical `url` is `<url_base>/ValueSet/<id>` when a
/// base is given, otherwise the list's `opencodelists_url` if present, otherwise
/// omitted.
fn export_fhir_json(fm: &FrontMatter, active: &[(&str, &str)], url_base: Option<&str>) -> String {
    let canonical: Option<String> = match url_base {
        Some(base) => Some(format!("{}/ValueSet/{}", base.trim_end_matches('/'), fm.id)),
        None => fm
            .opencodelists_url
            .as_deref()
            .filter(|u| !u.is_empty())
            .map(str::to_string),
    };
    let vs = fhir_valueset(fm, active, canonical.as_deref(), true);
    let mut s = serde_json::to_string_pretty(&vs).expect("serialising a JSON value is infallible");
    s.push('\n');
    s
}

fn cmd_export(args: ExportArgs) -> Result<()> {
    let cl = read_codelist(&args.file)?;
    let registry = crate::paths::codelist_registry(args.codelists.as_deref());
    // Effective members flatten any `includes:`; for a plain list this is just
    // the file's own active concepts in body order.
    let members = effective_members_of(&cl, &args.file, &registry, false)?;
    let active: Vec<(&str, &str)> = members
        .iter()
        .map(|m| (m.id.as_str(), m.term.as_str()))
        .collect();

    let terminologies: Vec<String> = args
        .include_maps
        .iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();

    if !terminologies.is_empty() && args.format == "opencodelists-csv" {
        bail!("--include-maps is not supported for opencodelists-csv (fixed schema: code,term)");
    }

    let maps: Option<CrosswalkMaps> = if terminologies.is_empty() {
        None
    } else {
        let db = crate::paths::resolve_db(args.db.as_deref())
            .context("--include-maps needs a SNOMED CT database to resolve crosswalks")?
            .path;
        let conn = open_db(&db)?;
        let sctids: Vec<&str> = active.iter().map(|(id, _)| *id).collect();
        Some(lookup_crosswalks(&conn, &sctids, &terminologies)?)
    };

    if !terminologies.is_empty() && matches!(args.format.as_str(), "fhir-json" | "rf2") {
        bail!("--include-maps is only supported for the csv and markdown formats");
    }

    let output = match args.format.as_str() {
        "csv" => export_csv_with_maps(&active, &terminologies, maps.as_ref()),
        "markdown" => {
            export_markdown_with_maps(&cl.front_matter, &active, &terminologies, maps.as_ref())
        }
        "opencodelists-csv" => export_opencodelists_csv(&active),
        "fhir-json" => export_fhir_json(&cl.front_matter, &active, args.url.as_deref()),
        "rf2" => bail!(
            "`rf2` export is not yet implemented.\n\
             Emitting a codelist as an RF2 Simple Reference Set needs a real SNOMED CT \
             namespace - a refsetId and moduleId (and member row UUIDs) that a codelist \
             does not carry - so it cannot be produced correctly without that input. \
             See https://github.com/pacharanero/sct/issues/60 to follow the design or \
             request that it be expedited. Use `--format fhir-json` for a portable, \
             standards-based export today."
        ),
        other => bail!(
            "unsupported export format: {other}\n\
             Supported: csv, opencodelists-csv, markdown, fhir-json (RF2 is deferred; \
             see https://github.com/pacharanero/sct/issues/60)."
        ),
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

fn cmd_include(args: IncludeArgs) -> Result<()> {
    if args.refs.is_empty() {
        bail!("provide at least one codelist reference to include or remove");
    }
    let mut cl = read_codelist(&args.file)?;
    let registry = crate::paths::codelist_registry(args.codelists.as_deref());
    let dir = args
        .file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let mut includes = cl.front_matter.includes.take().unwrap_or_default();

    if args.remove {
        let before = includes.len();
        includes.retain(|i| !args.refs.iter().any(|r| r.trim() == i.trim()));
        println!(
            "Removed {} include(s) from {}",
            before - includes.len(),
            args.file.display()
        );
    } else {
        for raw in &args.refs {
            let raw = raw.trim().to_string();
            if includes.iter().any(|i| i.trim() == raw) {
                eprintln!("note: {raw:?} is already included; skipping");
                continue;
            }
            match parse_include_ref(&raw) {
                IncludeRef::Url(u) => {
                    eprintln!("note: URL includes are not yet resolvable ({u}); recorded anyway");
                }
                r => {
                    let path = resolve_include_path(&r, &dir, &registry)?;
                    if !path.exists() {
                        bail!("include {raw:?} -> {} does not exist", path.display());
                    }
                }
            }
            includes.push(raw);
        }
        println!(
            "{} now composes {} included list(s)",
            args.file.display(),
            includes.len()
        );
    }

    cl.front_matter.includes = if includes.is_empty() {
        None
    } else {
        Some(includes)
    };
    cl.front_matter.updated = today();
    write_codelist(&cl, &args.file)
}

fn cmd_resolve(args: ResolveArgs) -> Result<()> {
    let cl = read_codelist(&args.file)?;
    let registry = crate::paths::codelist_registry(args.codelists.as_deref());
    let members = effective_members_of(&cl, &args.file, &registry, args.refresh)?;
    let include_count = cl.front_matter.includes.as_ref().map_or(0, |v| v.len());

    // Flatten into a standalone codelist: drop `includes`, inline every member.
    let mut fm = cl.front_matter;
    fm.includes = None;
    fm.updated = today();

    let mut body = vec![
        ConceptLine::Comment(format!(
            "# Resolved snapshot of {}: {} concept(s){}",
            args.file.display(),
            members.len(),
            if include_count > 0 {
                format!(" flattened from {include_count} include(s)")
            } else {
                String::new()
            }
        )),
        ConceptLine::Blank,
        ConceptLine::Comment("# concepts".to_string()),
    ];
    for m in &members {
        body.push(ConceptLine::Active {
            id: m.id.clone(),
            term: m.term.clone(),
            comment: None,
        });
    }
    let resolved = CodelistFile {
        front_matter: fm,
        body,
    };

    match args.output {
        Some(path) => {
            write_codelist(&resolved, &path)?;
            println!(
                "Resolved {} concept(s) to {}",
                members.len(),
                path.display()
            );
        }
        None => print!("{}", render_codelist(&resolved)?),
    }
    Ok(())
}

pub fn export_csv(active: &[(&str, &str)]) -> String {
    export_csv_with_maps(active, &[], None)
}

pub fn export_csv_with_maps(
    active: &[(&str, &str)],
    terminologies: &[String],
    maps: Option<&CrosswalkMaps>,
) -> String {
    let mut out = String::from("sctid,preferred_term");
    for t in terminologies {
        out.push(',');
        out.push_str(t);
    }
    out.push('\n');
    for (id, term) in active {
        out.push_str(&format!("{},{}", id, csv_escape(term)));
        for t in terminologies {
            let joined = maps.map(|m| m.codes_for(id, t)).unwrap_or_default();
            out.push(',');
            out.push_str(&csv_escape(&joined));
        }
        out.push('\n');
    }
    out
}

pub fn export_opencodelists_csv(active: &[(&str, &str)]) -> String {
    let mut out = String::from("code,term\n");
    for (id, term) in active {
        out.push_str(&format!("{},{}\n", id, csv_escape(term)));
    }
    out
}

pub fn export_markdown(fm: &FrontMatter, active: &[(&str, &str)]) -> String {
    export_markdown_with_maps(fm, active, &[], None)
}

pub fn export_markdown_with_maps(
    fm: &FrontMatter,
    active: &[(&str, &str)],
    terminologies: &[String],
    maps: Option<&CrosswalkMaps>,
) -> String {
    let mut out = format!("# {}\n\n", fm.title);
    out.push_str(&format!("**Description:** {}\n\n", fm.description));
    out.push_str(&format!(
        "**Terminology:** {} | **Version:** {} | **Status:** {} | **Updated:** {}\n\n",
        fm.terminology, fm.version, fm.status, fm.updated
    ));

    out.push_str("| SCTID | Preferred Term");
    for t in terminologies {
        out.push_str(" | ");
        out.push_str(t);
    }
    out.push_str(" |\n|---|---");
    for _ in terminologies {
        out.push_str("|---");
    }
    out.push_str("|\n");

    for (id, term) in active {
        out.push_str(&format!("| `{id}` | {term}"));
        for t in terminologies {
            let joined = maps.map(|m| m.codes_for(id, t)).unwrap_or_default();
            out.push_str(" | ");
            out.push_str(&joined);
        }
        out.push_str(" |\n");
    }
    out
}

/// Crosswalk map lookup: sctid → terminology (lowercased) → sorted codes.
#[derive(Default)]
pub struct CrosswalkMaps {
    inner: HashMap<String, HashMap<String, Vec<String>>>,
}

impl CrosswalkMaps {
    /// Return all crosswalk codes for the given SCTID in the given terminology,
    /// joined with `|`. Empty string if none.
    pub fn codes_for(&self, sctid: &str, terminology: &str) -> String {
        self.inner
            .get(sctid)
            .and_then(|m| m.get(terminology))
            .map(|v| {
                let mut v = v.clone();
                v.sort();
                v.dedup();
                v.join("|")
            })
            .unwrap_or_default()
    }
}

/// Load crosswalk codes for a set of SCTIDs across the given terminologies.
///
/// Terminology names are compared case-insensitively against the lowercased
/// values stored in `crossmaps` (or, for older databases, `concept_maps`).
/// Missing terminologies are silently absent from the result (caller can detect
/// this by getting empty strings from `codes_for`); we also emit a stderr
/// warning once per missing terminology so users know the DB didn't have the
/// requested crosswalk.
pub fn lookup_crosswalks(
    conn: &Connection,
    sctids: &[&str],
    terminologies: &[String],
) -> Result<CrosswalkMaps> {
    let mut maps = CrosswalkMaps::default();
    if sctids.is_empty() || terminologies.is_empty() {
        return Ok(maps);
    }

    let has_crossmaps = table_exists(conn, "crossmaps")?;
    let has_concept_maps = table_exists(conn, "concept_maps")?;

    // Available terminologies span both map tables while old databases migrate
    // from concept_maps to the general crossmaps model.
    let mut available: HashSet<String> = HashSet::new();
    if has_concept_maps {
        let mut stmt = conn.prepare("SELECT DISTINCT lower(terminology) FROM concept_maps")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        available.extend(rows.collect::<std::result::Result<Vec<_>, _>>()?);
    }
    if has_crossmaps {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT lower(source_system) FROM crossmaps WHERE target_system = 'snomed'
             UNION
             SELECT DISTINCT lower(target_system) FROM crossmaps WHERE source_system = 'snomed'",
        )?;
        for r in stmt.query_map([], |row| row.get::<_, String>(0))? {
            available.insert(r?);
        }
    }
    for t in terminologies {
        if !available.contains(t) {
            eprintln!(
                "warning: terminology '{t}' has no maps in this database; column will be empty. \
                 Available: {} (ICD-10/OPCS-4 need `sct ndjson --refsets all`)",
                available.iter().cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }

    let id_ph = std::iter::repeat_n("?", sctids.len())
        .collect::<Vec<_>>()
        .join(",");
    let active_filter = if column_exists(conn, "crossmaps", "active")? {
        "AND active != 0"
    } else {
        ""
    };

    // Newer databases use crossmaps for both SNOMED -> classification and
    // legacy -> SNOMED rows.
    let crossmap_terms: Vec<&String> = terminologies.iter().collect();
    if has_crossmaps && !crossmap_terms.is_empty() {
        fill_maps(
            conn,
            &mut maps,
            sctids,
            &crossmap_terms,
            &format!(
                "SELECT source_code, lower(target_system), target_code FROM crossmaps
                 WHERE source_system = 'snomed'
                   AND source_code IN ({id_ph})
                   AND lower(target_system) IN ({})
                   {active_filter}",
                placeholders(crossmap_terms.len())
            ),
        )?;
        fill_maps(
            conn,
            &mut maps,
            sctids,
            &crossmap_terms,
            &format!(
                "SELECT target_code, lower(source_system), source_code FROM crossmaps
                 WHERE target_system = 'snomed'
                   AND target_code IN ({id_ph})
                   AND lower(source_system) IN ({})
                   {active_filter}",
                placeholders(crossmap_terms.len())
            ),
        )?;
    }

    // concept_maps holds legacy CTV3/Read v2 rows in older SQLite databases.
    let legacy: Vec<&String> = terminologies
        .iter()
        .filter(|t| matches!(t.as_str(), "ctv3" | "read2"))
        .collect();
    if has_concept_maps && !legacy.is_empty() {
        fill_maps(
            conn,
            &mut maps,
            sctids,
            &legacy,
            &format!(
                "SELECT concept_id, lower(terminology), code FROM concept_maps
                 WHERE concept_id IN ({id_ph}) AND lower(terminology) IN ({})",
                placeholders(legacy.len())
            ),
        )?;
    }

    Ok(maps)
}

fn placeholders(n: usize) -> String {
    std::iter::repeat_n("?", n).collect::<Vec<_>>().join(",")
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1)",
        [name],
        |row| row.get(0),
    )?;
    Ok(exists != 0)
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Run a `SELECT sctid, terminology, code` query (params = sctids then terms)
/// and accumulate the rows into `maps`.
fn fill_maps(
    conn: &Connection,
    maps: &mut CrosswalkMaps,
    sctids: &[&str],
    terms: &[&String],
    sql: &str,
) -> Result<()> {
    let mut p: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(sctids.len() + terms.len());
    for id in sctids {
        p.push(id);
    }
    for t in terms {
        p.push(*t);
    }
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(p.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (sctid, terminology, code) = row?;
        maps.inner
            .entry(sctid)
            .or_default()
            .entry(terminology)
            .or_default()
            .push(code);
    }
    Ok(())
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
    crate::commands::open_db_readonly(path, None)
}

pub fn lookup_preferred_term(conn: &Connection, id: &str) -> Result<String> {
    conn.query_row(
        "SELECT preferred_term FROM concepts WHERE id = ?1 AND active = 1",
        params![id],
        |row| row.get(0),
    )
    .with_context(|| format!("SCTID {id} not found or inactive"))
}

pub fn lookup_concept_row(conn: &Connection, id: &str) -> Result<Option<(String, bool)>> {
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

pub fn lookup_hierarchy_and_children(conn: &Connection, id: &str) -> Result<Option<(String, i64)>> {
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

#[cfg(test)]
fn get_all_descendants(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn)?;
    get_all_descendants_with_tct(conn, id, crate::ecl::eval::has_tct(conn)?)
}

fn get_all_descendants_with_tct(conn: &Connection, id: &str, tct: bool) -> Result<Vec<String>> {
    let sql = if tct {
        "SELECT CAST(ca.descendant_id AS TEXT)
         FROM concept_ancestors ca
         JOIN concepts c ON c.id = CAST(ca.descendant_id AS TEXT)
         WHERE ca.ancestor_id = ?1 AND ca.descendant_id != ?1 AND c.active = 1
         ORDER BY ca.descendant_id"
    } else {
        "WITH RECURSIVE desc(id) AS (
             SELECT DISTINCT child_id FROM concept_isa WHERE parent_id = ?1
             UNION
             SELECT ci.child_id FROM concept_isa ci JOIN desc d ON ci.parent_id = d.id
         )
         SELECT d.id FROM desc d
         JOIN concepts c ON c.id = d.id
         WHERE c.active = 1
         ORDER BY CAST(d.id AS INTEGER)"
    };
    let mut stmt = conn.prepare(sql)?;
    let ids = stmt
        .query_map(params![id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ids)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    // -----------------------------------------------------------------------
    // Fixtures
    // -----------------------------------------------------------------------

    /// Minimal `FrontMatter` for export tests; tweak fields on the returned
    /// value as needed.
    fn sample_fm(id: &str, status: &str) -> FrontMatter {
        FrontMatter {
            id: id.to_string(),
            title: format!("{id} title"),
            description: "desc".to_string(),
            terminology: "SNOMED CT".to_string(),
            created: "2026-04-18".to_string(),
            updated: "2026-04-18".to_string(),
            version: 3,
            status: status.to_string(),
            licence: String::new(),
            copyright: String::new(),
            appropriate_use: String::new(),
            misuse: String::new(),
            includes: None,
            snomed_release: None,
            authors: None,
            organisation: None,
            methodology: None,
            signoffs: None,
            warnings: None,
            population: None,
            care_setting: None,
            tags: None,
            opencodelists_id: None,
            opencodelists_url: None,
        }
    }

    const TEST_CODELIST: &str = "---
id: asthma-diagnosis
title: Asthma Diagnosis
description: Concepts for asthma diagnosis.
terminology: SNOMED CT
created: 2024-01-01
updated: 2024-06-01
version: 1
status: active
licence: CC BY 4.0
copyright: Test Organisation
appropriate_use: Research use only.
misuse: Not for clinical decision support.
---

# ── Active concepts ──
195967001      Asthma (disorder)
57607007       Occupational asthma (disorder)  # included after review

# ── Excluded ──
# 41553006      Extrinsic asthma (disorder)
# ? 266364000   Exercise-induced asthma (disorder)

# trailing comment
";

    // -----------------------------------------------------------------------
    // parse_body_line tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_active_concept() {
        let line = parse_body_line("195967001      Asthma (disorder)");
        match line {
            ConceptLine::Active { id, term, comment } => {
                assert_eq!(id, "195967001");
                assert_eq!(term, "Asthma (disorder)");
                assert!(comment.is_none());
            }
            other => panic!("expected Active, got {:?}", other),
        }
    }

    #[test]
    fn parse_active_with_inline_comment() {
        let line = parse_body_line(
            "57607007       Occupational asthma (disorder)  # included after review",
        );
        match line {
            ConceptLine::Active { id, term, comment } => {
                assert_eq!(id, "57607007");
                assert_eq!(term, "Occupational asthma (disorder)");
                assert_eq!(comment.as_deref(), Some("included after review"));
            }
            other => panic!("expected Active with comment, got {:?}", other),
        }
    }

    #[test]
    fn parse_excluded_concept() {
        let line = parse_body_line("# 41553006      Extrinsic asthma (disorder)");
        match line {
            ConceptLine::Excluded { id, term, comment } => {
                assert_eq!(id, "41553006");
                assert_eq!(term, "Extrinsic asthma (disorder)");
                assert!(comment.is_none());
            }
            other => panic!("expected Excluded, got {:?}", other),
        }
    }

    #[test]
    fn parse_excluded_with_comment() {
        let line = parse_body_line("# 41553006      Extrinsic asthma (disorder)  # too specific");
        match line {
            ConceptLine::Excluded { id, comment, .. } => {
                assert_eq!(id, "41553006");
                assert_eq!(comment.as_deref(), Some("too specific"));
            }
            other => panic!("expected Excluded with comment, got {:?}", other),
        }
    }

    #[test]
    fn parse_pending_review() {
        let line = parse_body_line("# ? 266364000   Exercise-induced asthma (disorder)");
        match line {
            ConceptLine::PendingReview { id, term } => {
                assert_eq!(id, "266364000");
                assert_eq!(term, "Exercise-induced asthma (disorder)");
            }
            other => panic!("expected PendingReview, got {:?}", other),
        }
    }

    #[test]
    fn parse_section_comment() {
        let line = parse_body_line("# ── Active concepts ──");
        match line {
            ConceptLine::Comment(s) => assert_eq!(s, "# ── Active concepts ──"),
            other => panic!("expected Comment, got {:?}", other),
        }
    }

    #[test]
    fn parse_blank_line() {
        assert!(matches!(parse_body_line(""), ConceptLine::Blank));
        assert!(matches!(parse_body_line("   "), ConceptLine::Blank));
    }

    #[test]
    fn parse_sctid_lines_from_stdin() {
        // Bare ids (as emitted by `sct ecl expand`), plus blanks, comments,
        // and "id  term" lines all reduce to the leading SCTID.
        let input = "73211009\n\n# a comment\n  46635009  Type 1 diabetes mellitus\n44054006\n";
        assert_eq!(
            parse_sctid_lines(input),
            vec!["73211009", "46635009", "44054006"]
        );
        assert!(parse_sctid_lines("\n  \n# only comments\n").is_empty());
    }

    #[test]
    fn search_choices_parse_includes_and_exclusions() {
        assert_eq!(
            parse_search_choices("1, 3 -2", 3).unwrap(),
            vec![
                (0, SearchChoice::Include),
                (2, SearchChoice::Include),
                (1, SearchChoice::Exclude),
            ]
        );
        assert!(parse_search_choices("0", 3).is_err());
        assert!(parse_search_choices("4", 3).is_err());
        assert!(parse_search_choices("1,1", 3).is_err());
    }

    #[test]
    fn search_choices_update_existing_and_new_members() {
        let mut codelist = parse_codelist(TEST_CODELIST).unwrap();
        let results = vec![
            SearchResult {
                id: "195967001".into(),
                term: "Asthma".into(),
                hierarchy: "Clinical finding".into(),
            },
            SearchResult {
                id: "41553006".into(),
                term: "Extrinsic asthma".into(),
                hierarchy: "Clinical finding".into(),
            },
            SearchResult {
                id: "999".into(),
                term: "New concept".into(),
                hierarchy: "Clinical finding".into(),
            },
        ];

        let changed = apply_search_choices(
            &mut codelist,
            &results,
            &[
                (0, SearchChoice::Exclude),
                (1, SearchChoice::Include),
                (2, SearchChoice::Exclude),
            ],
        );

        assert_eq!(changed, 3);
        assert!(matches!(
            codelist
                .body
                .iter()
                .find(|line| line.sctid() == Some("195967001")),
            Some(ConceptLine::Excluded { .. })
        ));
        assert!(matches!(
            codelist
                .body
                .iter()
                .find(|line| line.sctid() == Some("41553006")),
            Some(ConceptLine::Active { .. })
        ));
        assert!(matches!(
            codelist
                .body
                .iter()
                .find(|line| line.sctid() == Some("999")),
            Some(ConceptLine::Excluded { .. })
        ));
    }

    #[test]
    fn import_csv_reads_export_schema_and_ignores_extra_columns() {
        let csv = "\u{feff}sctid,preferred_term,icd10\n22298006,Myocardial infarction,I219\n195967001,\"Asthma, unspecified\",J45\n";
        let payload = parse_import_csv(csv, "sctid", "preferred_term").unwrap();
        assert_eq!(
            payload.included,
            vec![
                ("22298006".into(), "Myocardial infarction".into()),
                ("195967001".into(), "Asthma, unspecified".into()),
            ]
        );
    }

    #[test]
    fn import_csv_rejects_wrong_schema_and_conflicting_duplicates() {
        assert!(parse_import_csv("code,term\n123,One\n", "sctid", "preferred_term").is_err());
        assert!(parse_import_csv(
            "sctid,preferred_term\n123,One\n123,Two\n",
            "sctid",
            "preferred_term"
        )
        .is_err());
    }

    #[test]
    fn import_fhir_reads_explicit_includes_exclusions_and_metadata() {
        let value_set = r#"{
          "resourceType":"ValueSet",
          "url":"https://example.org/ValueSet/asthma",
          "version":"3.2",
          "title":"Asthma codes",
          "status":"active",
          "description":"Imported test",
          "copyright":"Example copyright",
          "compose":{
            "include":[{"system":"http://snomed.info/sct","concept":[
              {"code":"195967001","display":"Asthma"},
              {"code":"41553006","display":"Occupational asthma"}
            ]}],
            "exclude":[{"system":"http://snomed.info/sct","concept":[
              {"code":"41553006","display":"Occupational asthma"}
            ]}]
          }
        }"#;
        let payload = parse_import_fhir(value_set).unwrap();
        assert_eq!(
            payload.included,
            vec![("195967001".into(), "Asthma".into())]
        );
        assert_eq!(
            payload.excluded,
            vec![("41553006".into(), "Occupational asthma".into())]
        );
        assert_eq!(payload.title.as_deref(), Some("Asthma codes"));
        assert_eq!(payload.source_version.as_deref(), Some("3.2"));
        assert_eq!(payload.source_status.as_deref(), Some("active"));
    }

    #[test]
    fn import_fhir_rejects_intensional_or_non_snomed_content() {
        let filtered = r#"{
          "resourceType":"ValueSet",
          "compose":{"include":[{"system":"http://snomed.info/sct","filter":[
            {"property":"concept","op":"is-a","value":"195967001"}
          ]}]}
        }"#;
        assert!(parse_import_fhir(filtered)
            .unwrap_err()
            .to_string()
            .contains("filters"));

        let icd = r#"{
          "resourceType":"ValueSet",
          "compose":{"include":[{"system":"http://hl7.org/fhir/sid/icd-10","concept":[
            {"code":"J45","display":"Asthma"}
          ]}]}
        }"#;
        assert!(parse_import_fhir(icd)
            .unwrap_err()
            .to_string()
            .contains("SNOMED CT only"));
    }

    #[test]
    fn imported_codelist_is_draft_and_records_source_provenance() {
        let payload = ImportPayload {
            included: vec![("195967001".into(), "Asthma".into())],
            title: Some("Asthma source".into()),
            source_status: Some("active".into()),
            ..ImportPayload::default()
        };
        let codelist = build_imported_codelist(
            Path::new("reviewed-asthma.codelist"),
            "source.valueset.json",
            "fhir-json",
            payload,
        )
        .unwrap();
        assert_eq!(codelist.front_matter.id, "reviewed-asthma");
        assert_eq!(codelist.front_matter.status, "draft");
        assert_eq!(codelist.front_matter.licence, "NOASSERTION");
        assert!(codelist
            .front_matter
            .methodology
            .as_deref()
            .unwrap()
            .contains("Source status: active"));
        assert!(
            matches!(codelist.body.last(), Some(ConceptLine::Active { id, .. }) if id == "195967001")
        );
    }

    #[test]
    fn import_source_provenance_redacts_credentials_query_and_fragment() {
        assert_eq!(
            source_for_provenance("https://user:token@example.org/list.csv?signature=secret#part"),
            "https://***@example.org/list.csv"
        );
        assert_eq!(source_for_provenance("/tmp/list.csv"), "/tmp/list.csv");
    }

    // -----------------------------------------------------------------------
    // Full parse tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_full_codelist_structure() {
        let cl = parse_codelist(TEST_CODELIST).unwrap();
        assert_eq!(cl.front_matter.id, "asthma-diagnosis");
        assert_eq!(cl.front_matter.title, "Asthma Diagnosis");
        assert_eq!(cl.front_matter.version, 1);

        let active: Vec<_> = cl.body.iter().filter(|l| l.is_active()).collect();
        let excluded: Vec<_> = cl
            .body
            .iter()
            .filter(|l| matches!(l, ConceptLine::Excluded { .. }))
            .collect();
        let pending: Vec<_> = cl
            .body
            .iter()
            .filter(|l| matches!(l, ConceptLine::PendingReview { .. }))
            .collect();

        assert_eq!(active.len(), 2, "should have 2 active concepts");
        assert_eq!(excluded.len(), 1, "should have 1 excluded concept");
        assert_eq!(pending.len(), 1, "should have 1 pending-review concept");
    }

    #[test]
    fn parse_active_sctids() {
        let cl = parse_codelist(TEST_CODELIST).unwrap();
        let ids: Vec<&str> = cl
            .body
            .iter()
            .filter_map(|l| {
                if let ConceptLine::Active { id, .. } = l {
                    Some(id.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(ids, vec!["195967001", "57607007"]);
    }

    #[test]
    fn parse_missing_front_matter_delimiter_errors() {
        let bad = "id: test\ntitle: Test\n\n195967001 Asthma\n";
        assert!(parse_codelist(bad).is_err());
    }

    #[test]
    fn parse_bom_stripped() {
        // UTF-8 BOM (\u{feff}) at start must not cause a parse error.
        let with_bom = format!("\u{feff}{}", TEST_CODELIST);
        let cl = parse_codelist(&with_bom).unwrap();
        assert_eq!(cl.front_matter.id, "asthma-diagnosis");
    }

    // -----------------------------------------------------------------------
    // Roundtrip test (write → read back → verify)
    // -----------------------------------------------------------------------

    #[test]
    fn roundtrip_parse_write_parse() {
        let cl = parse_codelist(TEST_CODELIST).unwrap();

        let tmp = NamedTempFile::new().unwrap();
        write_codelist(&cl, tmp.path()).unwrap();

        let cl2 = read_codelist(tmp.path()).unwrap();
        assert_eq!(cl2.front_matter.id, cl.front_matter.id);
        assert_eq!(cl2.front_matter.title, cl.front_matter.title);

        let active1: Vec<&str> = cl
            .body
            .iter()
            .filter_map(|l| {
                if let ConceptLine::Active { id, .. } = l {
                    Some(id.as_str())
                } else {
                    None
                }
            })
            .collect();
        let active2: Vec<&str> = cl2
            .body
            .iter()
            .filter_map(|l| {
                if let ConceptLine::Active { id, .. } = l {
                    Some(id.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            active1, active2,
            "active concept IDs must survive roundtrip"
        );
    }

    // -----------------------------------------------------------------------
    // Export tests
    // -----------------------------------------------------------------------

    #[test]
    fn export_csv_format() {
        let active = vec![
            ("195967001", "Asthma (disorder)"),
            ("57607007", "Occupational asthma (disorder)"),
        ];
        let csv = export_csv(&active);
        let mut lines = csv.lines();
        assert_eq!(lines.next().unwrap(), "sctid,preferred_term");
        assert_eq!(lines.next().unwrap(), "195967001,Asthma (disorder)");
        assert_eq!(
            lines.next().unwrap(),
            "57607007,Occupational asthma (disorder)"
        );
        assert!(lines.next().is_none());
    }

    #[test]
    fn export_opencodelists_csv_format() {
        let active = vec![("195967001", "Asthma (disorder)")];
        let csv = export_opencodelists_csv(&active);
        let mut lines = csv.lines();
        assert_eq!(lines.next().unwrap(), "code,term");
        assert_eq!(lines.next().unwrap(), "195967001,Asthma (disorder)");
    }

    #[test]
    fn export_csv_escapes_commas_in_term() {
        // A term containing a comma must be quoted in CSV output.
        let active = vec![("123456789", "Anxiety, unspecified")];
        let csv = export_csv(&active);
        assert!(
            csv.contains(r#""Anxiety, unspecified""#),
            "comma-containing term must be CSV-quoted; got: {csv}"
        );
    }

    #[test]
    fn export_csv_with_maps_appends_crosswalk_columns() {
        let active = vec![
            ("38598009", "Administration of MMR vaccine"),
            ("170431005", "MMR booster"),
        ];
        let mut maps = CrosswalkMaps::default();
        maps.inner.insert(
            "38598009".to_string(),
            [("ctv3".to_string(), vec!["65M1.".to_string()])]
                .into_iter()
                .collect(),
        );
        // 170431005 deliberately absent -> empty column

        let terminologies = vec!["ctv3".to_string()];
        let csv = export_csv_with_maps(&active, &terminologies, Some(&maps));
        let mut lines = csv.lines();
        assert_eq!(lines.next().unwrap(), "sctid,preferred_term,ctv3");
        assert_eq!(
            lines.next().unwrap(),
            "38598009,Administration of MMR vaccine,65M1."
        );
        assert_eq!(lines.next().unwrap(), "170431005,MMR booster,");
    }

    #[test]
    fn export_csv_with_maps_joins_multiple_codes_with_pipe() {
        let active = vec![("123", "Concept with two CTV3 maps")];
        let mut maps = CrosswalkMaps::default();
        maps.inner.insert(
            "123".to_string(),
            [(
                "ctv3".to_string(),
                vec!["AAA..".to_string(), "BBB..".to_string()],
            )]
            .into_iter()
            .collect(),
        );
        let terminologies = vec!["ctv3".to_string()];
        let csv = export_csv_with_maps(&active, &terminologies, Some(&maps));
        assert!(
            csv.contains("AAA..|BBB.."),
            "multiple codes must be pipe-joined; got: {csv}"
        );
    }

    #[test]
    fn export_csv_no_maps_matches_legacy_output() {
        // With no --include-maps, export_csv_with_maps must produce identical
        // output to the legacy export_csv so existing consumers are unaffected.
        let active = vec![("195967001", "Asthma (disorder)")];
        let legacy = export_csv(&active);
        let new_path = export_csv_with_maps(&active, &[], None);
        assert_eq!(legacy, new_path);
    }

    #[test]
    fn export_markdown_with_maps_appends_columns() {
        let fm = FrontMatter {
            id: "test".to_string(),
            title: "Test".to_string(),
            description: "Test".to_string(),
            terminology: "SNOMED CT".to_string(),
            created: "2026-04-18".to_string(),
            updated: "2026-04-18".to_string(),
            version: 1,
            status: "draft".to_string(),
            licence: String::new(),
            copyright: String::new(),
            appropriate_use: String::new(),
            misuse: String::new(),
            includes: None,
            snomed_release: None,
            authors: None,
            organisation: None,
            methodology: None,
            signoffs: None,
            warnings: None,
            population: None,
            care_setting: None,
            tags: None,
            opencodelists_id: None,
            opencodelists_url: None,
        };
        let active = vec![("38598009", "Admin MMR")];
        let mut maps = CrosswalkMaps::default();
        maps.inner.insert(
            "38598009".to_string(),
            [("ctv3".to_string(), vec!["65M1.".to_string()])]
                .into_iter()
                .collect(),
        );
        let md = export_markdown_with_maps(&fm, &active, &["ctv3".to_string()], Some(&maps));
        assert!(md.contains("| SCTID | Preferred Term | ctv3 |"));
        assert!(md.contains("| `38598009` | Admin MMR | 65M1. |"));
    }

    #[test]
    fn fhir_status_maps_to_required_value_set() {
        assert_eq!(fhir_status("draft"), "draft");
        assert_eq!(fhir_status("published"), "active");
        assert_eq!(fhir_status("active"), "active");
        assert_eq!(fhir_status("retired"), "retired");
        assert_eq!(fhir_status("inactive"), "retired");
        assert_eq!(fhir_status("anything-else"), "unknown");
    }

    #[test]
    fn export_fhir_json_builds_extensional_valueset() {
        let fm = sample_fm("asthma", "published");
        let active = vec![("195967001", "Asthma"), ("389145006", "Allergic asthma")];
        let out = export_fhir_json(&fm, &active, None);
        assert!(out.ends_with('\n'), "output should end with a newline");

        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["resourceType"], "ValueSet");
        assert_eq!(v["id"], "asthma");
        assert_eq!(v["name"], "asthma");
        assert_eq!(v["title"], "asthma title");
        assert_eq!(v["version"], "3");
        assert_eq!(v["status"], "active", "published status maps to active");

        let include = &v["compose"]["include"][0];
        assert_eq!(include["system"], SNOMED_SYSTEM);
        let concepts = include["concept"].as_array().unwrap();
        assert_eq!(concepts.len(), 2);
        assert_eq!(concepts[0]["code"], "195967001");
        assert_eq!(concepts[0]["display"], "Asthma");

        // No --url and no opencodelists_url: url is omitted (optional in FHIR).
        assert!(v.get("url").is_none());
        // Empty copyright is omitted rather than emitted blank.
        assert!(v.get("copyright").is_none());
    }

    #[test]
    fn export_fhir_json_url_base_forms_canonical_without_double_slash() {
        let fm = sample_fm("asthma", "draft");
        let active = vec![("195967001", "Asthma")];
        // Trailing slash on the base must not produce `//ValueSet`.
        let out = export_fhir_json(&fm, &active, Some("https://tx.example.org/fhir/"));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["url"], "https://tx.example.org/fhir/ValueSet/asthma");
    }

    #[test]
    fn export_fhir_json_falls_back_to_opencodelists_url() {
        let mut fm = sample_fm("asthma", "draft");
        fm.opencodelists_url =
            Some("https://www.opencodelists.org/codelist/org/asthma/".to_string());
        fm.copyright = "© Example".to_string();
        let active = vec![("195967001", "Asthma")];
        // No explicit base, so the stored opencodelists_url is used verbatim.
        let out = export_fhir_json(&fm, &active, None);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["url"],
            "https://www.opencodelists.org/codelist/org/asthma/"
        );
        assert_eq!(v["copyright"], "© Example");
    }

    #[test]
    fn export_csv_escapes_quotes_in_term() {
        let active = vec![("123456789", r#"He said "yes""#)];
        let csv = export_csv(&active);
        // RFC 4180: double-quote escaping inside quoted field
        assert!(
            csv.contains(r#""He said ""yes"""#),
            "internal quotes must be doubled; got: {csv}"
        );
    }

    // -----------------------------------------------------------------------
    // split_term_comment tests
    // -----------------------------------------------------------------------

    #[test]
    fn split_term_no_comment() {
        let (term, comment) = split_term_comment("Asthma (disorder)");
        assert_eq!(term, "Asthma (disorder)");
        assert!(comment.is_none());
    }

    #[test]
    fn split_term_with_comment() {
        let (term, comment) = split_term_comment("Asthma (disorder) # added by reviewer");
        assert_eq!(term, "Asthma (disorder)");
        assert_eq!(comment.as_deref(), Some("added by reviewer"));
    }

    #[test]
    fn descendant_lookup_matches_with_and_without_transitive_closure() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (id TEXT PRIMARY KEY, active INTEGER NOT NULL);
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             INSERT INTO concepts VALUES ('1', 1), ('2', 1), ('3', 1), ('4', 0);
             INSERT INTO concept_isa VALUES ('2', '1'), ('3', '2'), ('4', '1');",
        )
        .unwrap();
        let recursive = get_all_descendants(&conn, "1").unwrap();
        assert_eq!(recursive, ["2", "3"]);

        conn.execute_batch(
            "CREATE TABLE concept_ancestors (
                 ancestor_id INTEGER NOT NULL,
                 descendant_id INTEGER NOT NULL,
                 depth INTEGER NOT NULL
             );
             INSERT INTO concept_ancestors VALUES
                 (1, 2, 1), (1, 3, 2), (1, 4, 1), (2, 3, 1);
             CREATE INDEX idx_ca_ancestor ON concept_ancestors(ancestor_id);
             CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id);
             CREATE UNIQUE INDEX idx_ca_pair
                 ON concept_ancestors(ancestor_id, descendant_id);
             CREATE TABLE concept_ancestors_meta (
                 schema_version INTEGER NOT NULL,
                 include_self INTEGER NOT NULL CHECK (include_self IN (0, 1))
             );
             INSERT INTO concept_ancestors_meta VALUES (1, 0);",
        )
        .unwrap();
        conn.execute_batch(crate::ecl::eval::TCT_INVALIDATION_TRIGGERS_SQL)
            .unwrap();
        let indexed = get_all_descendants(&conn, "1").unwrap();
        assert_eq!(indexed, recursive);
    }
}
