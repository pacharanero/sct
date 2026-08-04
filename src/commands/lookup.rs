// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct lookup` - Look up a SNOMED CT concept by SCTID or CTV3 code.
//!
//! Accepts a bare SCTID (numeric) and returns full concept details.
//! Also accepts a CTV3 code and attempts reverse lookup via the
//! concept_maps table (requires a UK Monolith-derived database).
//!
//! Examples:
//!   sct lookup 22298006
//!   sct lookup --db snomed.db 22298006
//!   sct lookup XE0Uh

use anyhow::{bail, Context, Result};
use clap::Parser;
use rusqlite::{params, Connection};
use std::path::PathBuf;

use crate::builder::strip_semantic_tag;
use crate::commands::batch::{self, BatchItem, LineMode};
use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, ProvenanceFlags};
use crate::sdk::{Concept, Snomed};

#[derive(Parser, Debug)]
pub struct Args {
    /// SCTID (numeric) or CTV3 code to look up. Pass `-` to read one code per
    /// line from stdin.
    pub code: String,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Deprecated alias for `--format json`.
    #[arg(long, hide = true, conflicts_with = "ids")]
    pub json: bool,

    /// Emit only the resolved SCTID(s), newline-delimited, for piping. With a
    /// CTV3 code this prints the mapped SNOMED concept id(s).
    #[arg(long, conflicts_with = "format")]
    pub ids: bool,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

pub fn run(args: Args) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let snomed = Snomed::open(&db)?;
    let prov = snomed.provenance().cloned();
    let format = args.format.or_json_flag(args.json);
    let mode = if format.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let code = args.code.trim();
    if code == "-" {
        return run_batch(&snomed, format, prov.as_ref(), show_prov, args.ids);
    }

    // `--ids`: machine output for pipes - resolved SCTID(s) only.
    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for id in resolve_ids(&snomed, code)? {
            writeln!(out, "{id}")?;
        }
        return Ok(());
    }

    // If the code looks numeric, try SCTID first.
    if code.chars().all(|c| c.is_ascii_digit()) {
        if let Some(concept) = snomed.concept(code)? {
            return print_concept(concept, format, prov.as_ref(), show_prov);
        }
        bail!("Concept {code} not found.");
    }

    // Non-numeric: try CTV3 mapping.
    let mapped = lookup_ctv3(snomed.connection(), code)?;
    if mapped.is_empty() {
        bail!(
            "No SNOMED CT mapping found for CTV3 code '{code}'.\n\
             Mappings are only present when the database was built from a UK Monolith RF2 release."
        );
    }

    if format.is_structured() {
        let mut concepts = mapped
            .iter()
            .map(|(id, _, _, _)| {
                snomed
                    .concept(id)?
                    .with_context(|| format!("CTV3 code '{code}' maps to missing concept {id}"))
            })
            .collect::<Result<Vec<_>>>()?;
        if concepts.len() == 1 {
            return print_concept(
                concepts.pop().expect("one mapped concept"),
                format,
                prov.as_ref(),
                show_prov,
            );
        }
        let results = serde_json::to_value(concepts)?;
        let value = if show_prov {
            let mut value = serde_json::json!({ "results": results });
            provenance::inject_into_json(&mut value, prov.as_ref(), true);
            value
        } else {
            results
        };
        format.print(&value)?;
        return Ok(());
    }

    if mapped.len() == 1 {
        // Single mapping - show full concept detail.
        if let Some(concept) = snomed.concept(&mapped[0].0)? {
            println!("CTV3 {code} → SCTID {}\n", mapped[0].0);
            return print_concept(concept, format, prov.as_ref(), show_prov);
        }
    }

    // Multiple mappings - list them, then show full detail for each.
    println!(
        "CTV3 {code} maps to {} SNOMED CT concept{}:\n",
        mapped.len(),
        if mapped.len() == 1 { "" } else { "s" }
    );
    for (id, pt, fsn, hierarchy) in &mapped {
        println!("  [{id}] {pt}");
        let fsn_clean = strip_semantic_tag(fsn);
        if fsn_clean != pt && !fsn.is_empty() {
            println!("        FSN: {fsn_clean}");
        }
        println!("        {hierarchy}");
    }

    if mapped.len() > 1 {
        println!("\nUse `sct lookup <SCTID>` for full details on a specific concept.");
    }

    provenance::print_human_footer(prov.as_ref(), show_prov);

    Ok(())
}

fn run_batch(
    snomed: &Snomed,
    format: OutputFormat,
    prov: Option<&provenance::Provenance>,
    show_prov: bool,
    ids_only: bool,
) -> Result<()> {
    let codes = batch::read_stdin(LineMode::FirstToken, "codes")?;
    if ids_only {
        let mut resolved = Vec::with_capacity(codes.len());
        let mut budget = batch::ResultBudget::new();
        for code in codes {
            let ids = resolve_ids(snomed, &code)?;
            budget.retain(ids.len(), "lookup")?;
            resolved.extend(ids);
        }
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for id in resolved {
            writeln!(out, "{id}")?;
        }
        return Ok(());
    }

    let mut items = Vec::with_capacity(codes.len());
    let mut budget = batch::ResultBudget::new();
    for code in codes {
        let concepts = resolve_concepts(snomed, &code)?;
        budget.retain(concepts.len(), "lookup")?;
        items.push(BatchItem::new(code, concepts));
    }

    if format.is_structured() {
        let mut value = serde_json::json!({ "items": items });
        provenance::inject_into_json(&mut value, prov, show_prov);
        format.print(&value)?;
        return Ok(());
    }

    for item in &items {
        for concept in &item.result {
            println!(
                "{} | {} | {}",
                item.input, concept.id, concept.preferred_term
            );
        }
    }
    provenance::print_human_footer(prov, show_prov);
    Ok(())
}

fn resolve_ids(snomed: &Snomed, code: &str) -> Result<Vec<String>> {
    if code.chars().all(|c| c.is_ascii_digit()) {
        let exists: bool = snomed.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM concepts WHERE id = ?1)",
            [code],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("Concept {code} not found.");
        }
        return Ok(vec![code.to_string()]);
    }

    let mapped = lookup_ctv3_ids(snomed.connection(), code)?;
    if mapped.is_empty() {
        bail!(
            "No SNOMED CT mapping found for CTV3 code '{code}'.\n\
             Mappings are only present when the database was built from a UK Monolith RF2 release."
        );
    }
    Ok(mapped)
}

fn resolve_concepts(snomed: &Snomed, code: &str) -> Result<Vec<Concept>> {
    if code.chars().all(|c| c.is_ascii_digit()) {
        return snomed
            .concept(code)?
            .map(|concept| vec![concept])
            .with_context(|| format!("Concept {code} not found."));
    }

    let mapped = lookup_ctv3(snomed.connection(), code)?;
    if mapped.is_empty() {
        bail!(
            "No SNOMED CT mapping found for CTV3 code '{code}'.\n\
             Mappings are only present when the database was built from a UK Monolith RF2 release."
        );
    }
    mapped
        .into_iter()
        .map(|(id, _, _, _)| {
            snomed
                .concept(&id)?
                .with_context(|| format!("CTV3 code '{code}' maps to missing concept {id}"))
        })
        .collect()
}

/// Reverse-lookup a CTV3 code → SNOMED concept(s) via concept_maps.
fn lookup_ctv3(conn: &Connection, code: &str) -> Result<Vec<(String, String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy
         FROM concept_maps m
         JOIN concepts c ON c.id = m.concept_id
         WHERE m.code = ?1 AND m.terminology = 'ctv3'
         ORDER BY CAST(c.id AS INTEGER)",
    )?;

    let rows = stmt
        .query_map(params![code], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(rows)
}

fn lookup_ctv3_ids(conn: &Connection, code: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT c.id
         FROM concept_maps m
         JOIN concepts c ON c.id = m.concept_id
         WHERE m.code = ?1 AND m.terminology = 'ctv3'
         ORDER BY CAST(c.id AS INTEGER)",
    )?;
    let ids = stmt
        .query_map(params![code], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

fn print_concept(
    concept: Concept,
    format: OutputFormat,
    prov: Option<&provenance::Provenance>,
    show_prov: bool,
) -> Result<()> {
    let mut concept = serde_json::to_value(concept)?;
    if format.is_structured() {
        provenance::inject_into_json(&mut concept, prov, show_prov);
        if let Some(s) = format.render(&concept)? {
            println!("{s}");
        }
        return Ok(());
    }
    let concept = &concept;

    let id = concept["id"].as_str().unwrap_or("");
    let pt = concept["preferred_term"].as_str().unwrap_or("");
    let fsn = concept["fsn"].as_str().unwrap_or("");
    let hierarchy = concept["hierarchy"].as_str().unwrap_or("");
    let active = concept["active"].as_bool().unwrap_or(false);
    let module = concept["module"].as_str().unwrap_or("");
    let effective_time = concept["effective_time"].as_str().unwrap_or("");
    let children_count = concept["children_count"].as_i64().unwrap_or(0);

    // Header
    println!("  [{id}] {pt}");
    if !active {
        println!("  ⚠ INACTIVE");
    }

    // FSN (if different from PT)
    let fsn_clean = strip_semantic_tag(fsn);
    if fsn_clean != pt && !fsn.is_empty() {
        println!("  FSN: {fsn_clean}");
    }

    // Semantic tag from FSN
    if let Some(start) = fsn.rfind(" (") {
        if fsn.ends_with(')') {
            let tag = &fsn[start + 2..fsn.len() - 1];
            println!("  Semantic tag: {tag}");
        }
    }

    println!("  Hierarchy: {hierarchy}");

    // Hierarchy path
    if let Some(path) = concept["hierarchy_path"].as_array() {
        if !path.is_empty() {
            let names: Vec<&str> = path
                .iter()
                .filter_map(|v| {
                    v.as_object()
                        .and_then(|o| o.get("term").or(o.get("preferred_term")))
                        .and_then(|t| t.as_str())
                        .or_else(|| v.as_str())
                })
                .collect();
            if !names.is_empty() {
                println!("  Path: {}", names.join(" → "));
            }
        }
    }

    // Parents
    if let Some(parents) = concept["parents"].as_array() {
        if !parents.is_empty() {
            println!("  Parents:");
            for p in parents {
                let pid = p["id"].as_str().or(p["conceptId"].as_str()).unwrap_or("?");
                let pterm = p["term"]
                    .as_str()
                    .or(p["preferred_term"].as_str())
                    .unwrap_or("?");
                println!("    [{pid}] {pterm}");
            }
        }
    }

    println!("  Children: {children_count}");

    // Synonyms
    if let Some(syns) = concept["synonyms"].as_array() {
        if !syns.is_empty() {
            println!("  Synonyms:");
            for s in syns {
                let term = s.as_str().unwrap_or("?");
                if term != pt {
                    println!("    - {term}");
                }
            }
        }
    }

    // Attributes
    if let Some(attrs) = concept["attributes"].as_object() {
        if !attrs.is_empty() {
            println!("  Attributes:");
            for (key, val) in attrs {
                if let Some(arr) = val.as_array() {
                    for v in arr {
                        let vid = v["id"].as_str().or(v["conceptId"].as_str()).unwrap_or("?");
                        let vterm = v["term"]
                            .as_str()
                            .or(v["preferred_term"].as_str())
                            .unwrap_or("?");
                        println!("    {key}: [{vid}] {vterm}");
                    }
                }
            }
        }
    }

    // Cross-maps
    let ctv3 = concept["ctv3_codes"].as_array();
    let read2 = concept["read2_codes"].as_array();
    let has_ctv3 = ctv3.is_some_and(|a| !a.is_empty());
    let has_read2 = read2.is_some_and(|a| !a.is_empty());
    if has_ctv3 || has_read2 {
        println!("  Cross-maps:");
        if let Some(codes) = ctv3 {
            let cs: Vec<&str> = codes.iter().filter_map(|c| c.as_str()).collect();
            if !cs.is_empty() {
                println!("    CTV3: {}", cs.join(", "));
            }
        }
        if let Some(codes) = read2 {
            let cs: Vec<&str> = codes.iter().filter_map(|c| c.as_str()).collect();
            if !cs.is_empty() {
                println!("    Read v2: {}", cs.join(", "));
            }
        }
    }

    // Refset memberships
    if let Some(memberships) = concept["member_of"].as_array() {
        if !memberships.is_empty() {
            println!("  Member of refsets:");
            for m in memberships {
                let rid = m["id"].as_str().unwrap_or("?");
                let rpt = m["preferred_term"].as_str().unwrap_or("?");
                println!("    [{rid}] {rpt}");
            }
        }
    }

    // Metadata
    println!("  Module: {module}");
    println!("  Effective: {effective_time}");

    provenance::print_human_footer(prov, show_prov);

    Ok(())
}
