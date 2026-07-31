// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct markdown` - Export a SNOMED CT NDJSON artefact to Markdown files.
//!
//! Two output modes are available:
//!
//! **concept** (default) - one file per concept:
//!   <output-dir>/
//!     clinical-finding/22298006.md
//!     procedure/173171007.md
//!
//! **hierarchy** - one file per top-level hierarchy:
//!   <output-dir>/
//!     clinical-finding.md   (all concepts in Clinical Finding)
//!     procedure.md
//!
//! Each file is human-readable and LLM-friendly, suitable for RAG indexing
//! and direct file reading via filesystem MCP tools.

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use indicatif::ProgressBar;
use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::humanize::plural_count;
use crate::schema::ConceptRecord;

/// Output grouping mode.
#[derive(ValueEnum, Debug, Clone, PartialEq)]
pub enum OutputMode {
    /// One Markdown file per concept (default).
    Concept,
    /// One Markdown file per top-level SNOMED CT hierarchy.
    Hierarchy,
}

#[derive(Parser, Debug)]
pub struct Args {
    /// NDJSON artefact produced by `sct ndjson`. Use `-` for stdin.
    #[arg(
        long = "ndjson",
        alias = "input",
        short = 'i',
        value_hint = clap::ValueHint::FilePath,
        value_name = "NDJSON",
        value_parser = crate::paths::tilde_pathbuf
    )]
    pub input: PathBuf,

    /// Output directory for Markdown files.
    ///
    /// Defaults to the input's name with a `-concepts` suffix
    /// (`uk-monolith-42.ndjson` → `uk-monolith-42-concepts/`), created in the
    /// working directory. Reading from stdin gives `snomed-concepts/`.
    #[arg(long, short, value_parser = crate::paths::tilde_pathbuf)]
    pub output: Option<PathBuf>,

    /// Output grouping: one file per concept, or one file per hierarchy.
    #[arg(long, default_value = "concept")]
    pub mode: OutputMode,
}

pub fn run(args: Args) -> Result<()> {
    let output = crate::commands::resolve_output(
        args.output.as_deref(),
        &args.input,
        crate::paths::suffix::MARKDOWN_DIR,
    );

    let (reader, pb) = crate::progress::ndjson_reader(&args.input)?;

    std::fs::create_dir_all(&output)
        .with_context(|| format!("creating output directory {}", output.display()))?;

    match args.mode {
        OutputMode::Concept => run_concept_mode(reader, &output, &pb),
        OutputMode::Hierarchy => run_hierarchy_mode(reader, &output, &pb),
    }
}

/// One file per concept under `<output>/<hierarchy-slug>/<sctid>.md`.
fn run_concept_mode<R: std::io::Read>(
    reader: BufReader<R>,
    output: &Path,
    pb: &ProgressBar,
) -> Result<()> {
    pb.set_message("Writing per-concept Markdown files...");
    let mut n = 0usize;

    for line in reader.lines() {
        let line = line.context("reading input")?;
        if line.trim().is_empty() {
            continue;
        }
        if crate::provenance::try_parse_ndjson_line(&line).is_some() {
            continue;
        }

        let record: ConceptRecord = serde_json::from_str(&line).context("parsing NDJSON record")?;

        let dir = output.join(slugify(&record.hierarchy));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating directory {}", dir.display()))?;

        let path = dir.join(format!("{}.md", record.id));
        let content = render_concept(&record);
        let mut f =
            std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        f.write_all(content.as_bytes())?;

        n += 1;
        if n.is_multiple_of(50_000) {
            pb.set_message(format!(
                "{} written...",
                plural_count(n as u64, "Markdown file")
            ));
        }
    }

    pb.finish_with_message(format!(
        "Done. {} → {}",
        plural_count(n as u64, "Markdown file"),
        output.display()
    ));
    Ok(())
}

/// One file per top-level hierarchy under `<output>/<hierarchy-slug>.md`.
///
/// All concepts in a hierarchy are collected in memory, then written as a
/// single large Markdown file with an H1 for the hierarchy and H2 per concept.
/// This is useful for bulk LLM ingestion where all related concepts should
/// share context.
fn run_hierarchy_mode<R: std::io::Read>(
    reader: BufReader<R>,
    output: &Path,
    pb: &ProgressBar,
) -> Result<()> {
    pb.set_message("Loading concepts for hierarchy grouping...");

    // Group records by hierarchy, preserving insertion order.
    let mut groups: HashMap<String, Vec<ConceptRecord>> = HashMap::new();
    let mut n = 0usize;

    for line in reader.lines() {
        let line = line.context("reading input")?;
        if line.trim().is_empty() {
            continue;
        }
        if crate::provenance::try_parse_ndjson_line(&line).is_some() {
            continue;
        }

        let record: ConceptRecord = serde_json::from_str(&line).context("parsing NDJSON record")?;
        groups
            .entry(record.hierarchy.clone())
            .or_default()
            .push(record);

        n += 1;
        if n.is_multiple_of(50_000) {
            pb.set_message(format!("{} loaded...", plural_count(n as u64, "concept")));
        }
    }

    pb.set_message(format!(
        "Writing {}...",
        plural_count(groups.len() as u64, "hierarchy file")
    ));

    let mut files_written = 0;
    for (hierarchy, concepts) in &groups {
        let filename = format!("{}.md", slugify(hierarchy));
        let path = output.join(&filename);

        let mut buf = String::with_capacity(concepts.len() * 256);
        writeln!(buf, "# {}", hierarchy).unwrap();
        writeln!(buf).unwrap();
        writeln!(
            buf,
            "> {} in this hierarchy.",
            plural_count(concepts.len() as u64, "concept")
        )
        .unwrap();
        writeln!(buf).unwrap();

        for concept in concepts {
            render_concept_hierarchy_entry(concept, &mut buf);
        }

        let mut f =
            std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        f.write_all(buf.as_bytes())?;
        files_written += 1;
    }

    pb.finish_with_message(format!(
        "Done. {} → {} ({} total)",
        plural_count(files_written, "hierarchy file"),
        output.display(),
        plural_count(n as u64, "concept")
    ));
    Ok(())
}

/// Render a concept as an H2 section inside a hierarchy file.
fn render_concept_hierarchy_entry(r: &ConceptRecord, buf: &mut String) {
    writeln!(buf, "---").unwrap();
    writeln!(buf).unwrap();
    writeln!(buf, "## {} `{}`", r.preferred_term, r.id).unwrap();
    writeln!(buf).unwrap();
    writeln!(buf, "**FSN:** {}  ", r.fsn).unwrap();

    if !r.synonyms.is_empty() {
        writeln!(buf, "**Synonyms:** {}  ", r.synonyms.join(", ")).unwrap();
    }

    if !r.attributes.is_empty() {
        for (label, refs) in &r.attributes {
            let label_human = title_case(&label.replace('_', " "));
            let values: Vec<&str> = refs.iter().map(|c| strip_tag(&c.fsn)).collect();
            writeln!(buf, "**{}:** {}  ", label_human, values.join(", ")).unwrap();
        }
    }

    writeln!(buf).unwrap();
}

/// Render a single concept to Markdown.
fn render_concept(r: &ConceptRecord) -> String {
    let mut buf = String::with_capacity(512);

    // Title
    writeln!(buf, "# {}", r.preferred_term).unwrap();
    writeln!(buf).unwrap();

    // Key fields
    writeln!(buf, "**SCTID:** {}  ", r.id).unwrap();
    writeln!(buf, "**FSN:** {}  ", r.fsn).unwrap();

    // Hierarchy breadcrumb (all but the final element, joined with " > ")
    let breadcrumb = if r.hierarchy_path.len() > 1 {
        r.hierarchy_path[..r.hierarchy_path.len() - 1].join(" > ")
    } else {
        r.hierarchy.clone()
    };
    writeln!(buf, "**Hierarchy:** {}  ", breadcrumb).unwrap();
    writeln!(buf).unwrap();

    // Synonyms
    if !r.synonyms.is_empty() {
        writeln!(buf, "## Synonyms").unwrap();
        writeln!(buf).unwrap();
        for s in &r.synonyms {
            writeln!(buf, "- {}", s).unwrap();
        }
        writeln!(buf).unwrap();
    }

    // Relationships / attributes
    if !r.attributes.is_empty() {
        writeln!(buf, "## Relationships").unwrap();
        writeln!(buf).unwrap();
        for (label, refs) in &r.attributes {
            let label_human = label.replace('_', " ");
            for c in refs {
                // Strip semantic tag from FSN for readability
                let fsn_display = strip_tag(&c.fsn);
                writeln!(
                    buf,
                    "- **{}:** {} [{}]",
                    title_case(&label_human),
                    fsn_display,
                    c.id
                )
                .unwrap();
            }
        }
        writeln!(buf).unwrap();
    }

    // Hierarchy tree
    writeln!(buf, "## Hierarchy").unwrap();
    writeln!(buf).unwrap();
    for (i, label) in r.hierarchy_path.iter().enumerate() {
        let indent = "  ".repeat(i);
        if i == r.hierarchy_path.len() - 1 {
            writeln!(buf, "{}- **{}** *(this concept)*", indent, label).unwrap();
        } else {
            writeln!(buf, "{}- {}", indent, label).unwrap();
        }
    }

    // Parents
    if !r.parents.is_empty() {
        writeln!(buf).unwrap();
        writeln!(buf, "## Parents").unwrap();
        writeln!(buf).unwrap();
        for p in &r.parents {
            writeln!(buf, "- {} `{}`", p.fsn, p.id).unwrap();
        }
    }

    buf
}

/// Slugify a string for use as a directory name.
/// "Clinical finding" → "clinical-finding"
pub fn slugify(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut slug = String::with_capacity(lower.len());
    let mut prev_hyphen = false;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            prev_hyphen = false;
        } else if !prev_hyphen && !slug.is_empty() {
            slug.push('-');
            prev_hyphen = true;
        }
    }
    slug.trim_end_matches('-').to_string()
}

use crate::builder::strip_semantic_tag as strip_tag;

/// Convert snake_case label to Title Case. "finding_site" → "Finding site"
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_clinical_finding() {
        assert_eq!(slugify("Clinical finding"), "clinical-finding");
    }

    #[test]
    fn slugify_procedure() {
        assert_eq!(slugify("Procedure"), "procedure");
    }

    #[test]
    fn strip_semantic_tag() {
        assert_eq!(
            strip_tag("Myocardial infarction (disorder)"),
            "Myocardial infarction"
        );
        assert_eq!(strip_tag("No tag here"), "No tag here");
    }
}
