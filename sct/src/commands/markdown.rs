//! `sct markdown` — Export a SNOMED CT NDJSON artefact to per-concept Markdown files.
//!
//! Output layout:
//!   <output-dir>/
//!     clinical-finding/22298006.md
//!     procedure/173171007.md
//!     ...
//!
//! Each file is human-readable and LLM-friendly, suitable for RAG indexing
//! and direct file reading via filesystem MCP tools.

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::fmt::Write as FmtWrite;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::Duration;

use crate::schema::ConceptRecord;

#[derive(Parser, Debug)]
pub struct Args {
    /// Input NDJSON file produced by `sct ndjson`. Use `-` for stdin.
    #[arg(long, short)]
    pub input: PathBuf,

    /// Output directory for Markdown files.
    #[arg(long, short, default_value = "snomed-concepts")]
    pub output: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let input: Box<dyn std::io::Read> = if args.input.as_os_str() == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(
            std::fs::File::open(&args.input)
                .with_context(|| format!("opening {}", args.input.display()))?,
        )
    };

    let reader = BufReader::new(input);

    std::fs::create_dir_all(&args.output)
        .with_context(|| format!("creating output directory {}", args.output.display()))?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} [{elapsed_precise}] {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(120));
    pb.set_message("Writing Markdown files...");

    let mut n = 0usize;

    for line in reader.lines() {
        let line = line.context("reading input")?;
        if line.trim().is_empty() {
            continue;
        }

        let record: ConceptRecord = serde_json::from_str(&line).context("parsing NDJSON record")?;

        let dir = args.output.join(slugify(&record.hierarchy));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating directory {}", dir.display()))?;

        let path = dir.join(format!("{}.md", record.id));
        let content = render_concept(&record);
        let mut f =
            std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        f.write_all(content.as_bytes())?;

        n += 1;
        if n.is_multiple_of(50_000) {
            pb.set_message(format!("{} files written...", n));
        }
    }

    pb.finish_with_message(format!(
        "Done. {} Markdown files → {}",
        n,
        args.output.display()
    ));
    Ok(())
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

/// Strip semantic tag from an FSN. "Myocardial infarction (disorder)" → "Myocardial infarction"
fn strip_tag(fsn: &str) -> &str {
    if let Some(pos) = fsn.rfind(" (") {
        &fsn[..pos]
    } else {
        fsn
    }
}

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
