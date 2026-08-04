// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct refset` - Inspect SNOMED CT simple reference sets loaded into SQLite.
//!
//! Refsets are themselves concepts in SNOMED CT, so metadata (preferred term,
//! module, FSN) is looked up from the `concepts` table by JOINing on
//! `refset_members.refset_id`.
//!
//! Subcommands:
//!   list     - all refsets that have at least one member, with member counts
//!   info     - metadata + member count for a single refset
//!   members  - concepts in a given refset
//!   compare  - membership diff between two refsets (only-in-A / only-in-B / in-both)
//!   profile  - breakdown of a refset's members by top-level hierarchy
//!
//! The [`list_refsets`] and [`list_refset_members`] query helpers are shared
//! with the `sct mcp` server so the two surfaces always return the same data.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[cfg(test)]
use rusqlite::{params, Connection};

pub use crate::refset::{
    compare_refsets, list_refset_members, list_refsets, profile_refset_by_hierarchy,
    refset_summary, HierarchyCount, RefsetComparison, RefsetDiffSet, RefsetMember, RefsetSummary,
};

use crate::builder::strip_semantic_tag;
use crate::commands::batch::{self, BatchItem, LineMode};
use crate::format::{ConceptFields, ConceptFormat};
use crate::humanize::plural_count;
use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, ProvenanceFlags};
use crate::sdk::Snomed;

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// List all refsets that have at least one loaded member, with counts.
    List(ListArgs),

    /// Show metadata and member count for a single refset.
    Info(InfoArgs),

    /// List concepts belonging to a refset.
    Members(MembersArgs),

    /// Compare membership of two refsets (only-in-A, only-in-B, in-both).
    Compare(CompareArgs),

    /// Profile a refset's members by top-level hierarchy.
    Profile(ProfileArgs),
}

#[derive(Parser, Debug)]
pub struct ListArgs {
    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Deprecated alias for `--format json`.
    #[arg(long, hide = true)]
    pub json: bool,

    /// Override the per-refset line template (text output only).
    /// Default: `{id} | {pt} (<member count>)`. See `docs/commands/refset.md`.
    #[arg(long)]
    pub template: Option<String>,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

#[derive(Parser, Debug)]
pub struct InfoArgs {
    /// SCTID of the refset (which is itself a SNOMED CT concept). Pass `-` to
    /// read one refset SCTID per line from stdin.
    pub id: String,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Deprecated alias for `--format json`.
    #[arg(long, hide = true)]
    pub json: bool,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

#[derive(Parser, Debug)]
pub struct MembersArgs {
    /// SCTID of the refset. Pass `-` to read one refset SCTID per line from
    /// stdin.
    pub id: String,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Maximum number of members to display (default: all).
    #[arg(long)]
    pub limit: Option<usize>,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Deprecated alias for `--format json`.
    #[arg(long, hide = true, conflicts_with = "ids")]
    pub json: bool,

    /// Emit only member SCTIDs (newline-delimited) for piping, e.g.
    /// `sct refset members 447562003 --ids | sct codelist add list.codelist -`.
    #[arg(long, conflicts_with = "format")]
    pub ids: bool,

    /// Override the per-concept line template (text output only). See
    /// `docs/commands/refset.md` for the variable list.
    #[arg(long)]
    pub template: Option<String>,

    /// Override the FSN suffix template (rendered only when FSN differs from PT).
    /// Pass an empty string (`--template-fsn-suffix ""`) to suppress it entirely.
    #[arg(long)]
    pub template_fsn_suffix: Option<String>,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

/// Which set(s) to print full member details for in `compare`'s text output.
/// JSON/YAML output always includes all three sets (each subject to `--limit`).
#[derive(clap::ValueEnum, Clone, Debug, Default, PartialEq, Eq)]
pub enum CompareShow {
    /// Only print the counts (default).
    #[default]
    Counts,
    /// Also list members found only in the first refset.
    OnlyA,
    /// Also list members found only in the second refset.
    OnlyB,
    /// Also list members found in both refsets.
    Both,
    /// List all three sets in full.
    All,
}

#[derive(Parser, Debug)]
pub struct CompareArgs {
    /// SCTID of the first refset.
    pub id_a: String,

    /// SCTID of the second refset.
    pub id_b: String,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Maximum number of members to list per set (default: all).
    #[arg(long)]
    pub limit: Option<usize>,

    /// Which set(s) to list member details for (text output only).
    #[arg(long, value_enum, default_value_t = CompareShow::Counts)]
    pub show: CompareShow,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Deprecated alias for `--format json`.
    #[arg(long, hide = true)]
    pub json: bool,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

#[derive(Parser, Debug)]
pub struct ProfileArgs {
    /// SCTID of the refset. Pass `-` to read one refset SCTID per line from
    /// stdin.
    pub id: String,

    /// SQLite database produced by `sct sqlite`. See `docs/path-resolution.md`
    /// for the discovery order when this flag is omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Output format.
    #[arg(long, short = 'f', value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Deprecated alias for `--format json`.
    #[arg(long, hide = true)]
    pub json: bool,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

// ---------------------------------------------------------------------------
// CLI entry points
// ---------------------------------------------------------------------------

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Command::List(a) => run_list(a),
        Command::Info(a) => run_info(a),
        Command::Members(a) => run_members(a),
        Command::Compare(a) => run_compare(a),
        Command::Profile(a) => run_profile(a),
    }
}

fn run_list(args: ListArgs) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let snomed = Snomed::open(&db)?;
    let prov = snomed.provenance().cloned();
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let rows = snomed.refsets()?;

    if rows.is_empty() {
        println!(
            "No refset members loaded. Rebuild the database with `sct ndjson --refsets simple` \
             and `sct sqlite` from an RF2 release that includes simple refset files."
        );
        return Ok(());
    }

    if out.is_structured() {
        // Preserve the existing top-level array shape unless the user opts in
        // to provenance, in which case we wrap so we can attach _provenance.
        let value = if show_prov {
            let mut v = serde_json::json!({ "refsets": rows });
            provenance::inject_into_json(&mut v, prov.as_ref(), true);
            v
        } else {
            serde_json::to_value(&rows)?
        };
        if let Some(s) = out.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    let custom_format = args.template.map(|line| ConceptFormat {
        line,
        fsn_suffix: String::new(),
    });

    for r in &rows {
        if let Some(format) = &custom_format {
            println!(
                "{}",
                format.render(&ConceptFields {
                    id: &r.id,
                    pt: &r.preferred_term,
                    fsn: &r.fsn,
                    module: &r.module,
                    count: Some(r.member_count),
                    ..Default::default()
                })
            );
        } else {
            println!(
                "{} | {} ({})",
                r.id,
                r.preferred_term,
                plural_count(r.member_count as u64, "member")
            );
        }
    }
    provenance::print_human_footer(prov.as_ref(), show_prov);
    Ok(())
}

fn run_info(args: InfoArgs) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let snomed = Snomed::open(&db)?;
    let prov = snomed.provenance().cloned();
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    if args.id == "-" {
        return run_info_batch(&snomed, out, prov.as_ref(), show_prov);
    }

    let r = require_refset(&snomed, &args.id)?;

    if r.member_count == 0 && !out.is_structured() {
        println!(
            "Concept [{}] {} exists but has no loaded members.\n\
             (It may not be a refset, or its members weren't included in the RF2 load.)",
            r.id, r.preferred_term
        );
    }

    if out.is_structured() {
        let mut value = serde_json::to_value(&r)?;
        provenance::inject_into_json(&mut value, prov.as_ref(), show_prov);
        if let Some(s) = out.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    println!("  [{}] {}", r.id, r.preferred_term);
    let fsn_clean = strip_semantic_tag(&r.fsn);
    if fsn_clean != r.preferred_term && !r.fsn.is_empty() {
        println!("  FSN: {fsn_clean}");
    }
    println!("  Module:  {}", r.module);
    println!("  Members: {}", r.member_count);
    provenance::print_human_footer(prov.as_ref(), show_prov);
    Ok(())
}

fn run_members(args: MembersArgs) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let snomed = Snomed::open(&db)?;
    let prov = snomed.provenance().cloned();
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let limit = args.limit.map(|n| n.min(u32::MAX as usize) as u32);
    if args.id == "-" {
        return run_members_batch(&snomed, &args, out, prov.as_ref(), show_prov, limit);
    }
    // `--ids`: machine output for pipes - just member SCTIDs on stdout.
    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for id in snomed.refset_member_ids(&args.id, limit)? {
            writeln!(out, "{id}")?;
        }
        return Ok(());
    }
    let rows = snomed.refset_members(&args.id, limit)?;

    if rows.is_empty() && !out.is_structured() {
        eprintln!("No members found for refset {}.", args.id);
        return Ok(());
    }

    if out.is_structured() {
        let value = if show_prov {
            let mut v = serde_json::json!({ "members": rows });
            provenance::inject_into_json(&mut v, prov.as_ref(), true);
            v
        } else {
            serde_json::to_value(&rows)?
        };
        if let Some(s) = out.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    let format = ConceptFormat::load().with_overrides(args.template, args.template_fsn_suffix);
    for m in &rows {
        println!(
            "{}",
            format.render(&ConceptFields {
                id: &m.id,
                pt: &m.preferred_term,
                fsn: &m.fsn,
                hierarchy: &m.hierarchy,
                effective_time: &m.effective_time,
                ..Default::default()
            })
        );
    }
    provenance::print_human_footer(prov.as_ref(), show_prov);
    Ok(())
}

fn run_compare(args: CompareArgs) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let snomed = Snomed::open(&db)?;
    let prov = snomed.provenance().cloned();
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let limit = args.limit.map(|n| n.min(u32::MAX as usize) as u32);
    let cmp = snomed.refset_compare(&args.id_a, &args.id_b, limit)?;

    if out.is_structured() {
        let mut value = serde_json::to_value(&cmp)?;
        provenance::inject_into_json(&mut value, prov.as_ref(), show_prov);
        if let Some(s) = out.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    println!("A: [{}] {}", cmp.refset_a.id, cmp.refset_a.preferred_term);
    println!("B: [{}] {}", cmp.refset_b.id, cmp.refset_b.preferred_term);
    println!();
    println!("Only in A: {}", cmp.only_in_a.count);
    println!("Only in B: {}", cmp.only_in_b.count);
    println!("In both:   {}", cmp.in_both.count);

    let format = ConceptFormat::load();
    let print_set = |label: &str, set: &RefsetDiffSet| {
        println!("\n{label} ({}):", set.count);
        for m in &set.members {
            println!(
                "  {}",
                format.render(&ConceptFields {
                    id: &m.id,
                    pt: &m.preferred_term,
                    fsn: &m.fsn,
                    hierarchy: &m.hierarchy,
                    effective_time: &m.effective_time,
                    ..Default::default()
                })
            );
        }
    };
    match args.show {
        CompareShow::Counts => {}
        CompareShow::OnlyA => print_set("Only in A", &cmp.only_in_a),
        CompareShow::OnlyB => print_set("Only in B", &cmp.only_in_b),
        CompareShow::Both => print_set("In both", &cmp.in_both),
        CompareShow::All => {
            print_set("Only in A", &cmp.only_in_a);
            print_set("Only in B", &cmp.only_in_b);
            print_set("In both", &cmp.in_both);
        }
    }

    provenance::print_human_footer(prov.as_ref(), show_prov);
    Ok(())
}

fn run_profile(args: ProfileArgs) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let snomed = Snomed::open(&db)?;
    let prov = snomed.provenance().cloned();
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    if args.id == "-" {
        return run_profile_batch(&snomed, out, prov.as_ref(), show_prov);
    }

    let refset = require_refset(&snomed, &args.id)?;
    let hierarchies = snomed.refset_profile(&args.id)?;

    if out.is_structured() {
        let mut value = serde_json::json!({
            "refset": refset,
            "hierarchies": hierarchies,
        });
        provenance::inject_into_json(&mut value, prov.as_ref(), show_prov);
        if let Some(s) = out.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    println!("[{}] {}", refset.id, refset.preferred_term);
    println!("Members: {}", refset.member_count);

    if hierarchies.is_empty() {
        println!("\nNo members loaded for this refset.");
        provenance::print_human_footer(prov.as_ref(), show_prov);
        return Ok(());
    }

    println!();
    let total = refset.member_count.max(1) as f64;
    for h in &hierarchies {
        let pct = 100.0 * h.count as f64 / total;
        println!("  {:<40} {:>6}  ({pct:.1}%)", h.hierarchy, h.count);
    }

    provenance::print_human_footer(prov.as_ref(), show_prov);
    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct RefsetProfile {
    refset: RefsetSummary,
    hierarchies: Vec<HierarchyCount>,
}

fn require_refset(snomed: &Snomed, id: &str) -> Result<RefsetSummary> {
    snomed
        .refset(id)?
        .ok_or_else(|| anyhow::anyhow!("Refset {id} not found in concepts table."))
}

fn run_info_batch(
    snomed: &Snomed,
    out: OutputFormat,
    prov: Option<&provenance::Provenance>,
    show_prov: bool,
) -> Result<()> {
    let ids = batch::read_stdin(LineMode::FirstToken, "refset SCTIDs")?;
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let summary = require_refset(snomed, &id)?;
        items.push(BatchItem::new(id, summary));
    }

    if out.is_structured() {
        let mut value = serde_json::json!({ "items": items });
        provenance::inject_into_json(&mut value, prov, show_prov);
        out.print(&value)?;
        return Ok(());
    }

    for item in &items {
        println!(
            "{} | {} ({})",
            item.result.id,
            item.result.preferred_term,
            plural_count(item.result.member_count as u64, "member")
        );
    }
    provenance::print_human_footer(prov, show_prov);
    Ok(())
}

fn run_members_batch(
    snomed: &Snomed,
    args: &MembersArgs,
    out: OutputFormat,
    prov: Option<&provenance::Provenance>,
    show_prov: bool,
    limit: Option<u32>,
) -> Result<()> {
    let ids = batch::read_stdin(LineMode::FirstToken, "refset SCTIDs")?;
    if args.ids {
        let mut member_ids = Vec::new();
        let mut budget = batch::ResultBudget::new();
        for id in ids {
            let ids = snomed.refset_member_ids(&id, Some(budget.query_limit(limit)))?;
            budget.retain(ids.len(), "refset members")?;
            member_ids.extend(ids);
        }
        use std::io::Write;
        let mut output = std::io::stdout().lock();
        for id in member_ids {
            writeln!(output, "{id}")?;
        }
        return Ok(());
    }

    let mut items = Vec::with_capacity(ids.len());
    let mut budget = batch::ResultBudget::new();
    for id in ids {
        let members = snomed.refset_members(&id, Some(budget.query_limit(limit)))?;
        budget.retain(members.len(), "refset members")?;
        items.push(BatchItem::new(id, members));
    }

    if out.is_structured() {
        let mut value = serde_json::json!({ "items": items });
        provenance::inject_into_json(&mut value, prov, show_prov);
        out.print(&value)?;
        return Ok(());
    }

    let format = ConceptFormat::load()
        .with_overrides(args.template.clone(), args.template_fsn_suffix.clone());
    for item in &items {
        if item.result.is_empty() {
            eprintln!("No members found for refset {}.", item.input);
        }
        for member in &item.result {
            println!(
                "{}",
                format.render(&ConceptFields {
                    id: &member.id,
                    pt: &member.preferred_term,
                    fsn: &member.fsn,
                    hierarchy: &member.hierarchy,
                    effective_time: &member.effective_time,
                    ..Default::default()
                })
            );
        }
    }
    provenance::print_human_footer(prov, show_prov);
    Ok(())
}

fn run_profile_batch(
    snomed: &Snomed,
    out: OutputFormat,
    prov: Option<&provenance::Provenance>,
    show_prov: bool,
) -> Result<()> {
    let ids = batch::read_stdin(LineMode::FirstToken, "refset SCTIDs")?;
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        let result = RefsetProfile {
            refset: require_refset(snomed, &id)?,
            hierarchies: snomed.refset_profile(&id)?,
        };
        items.push(BatchItem::new(id, result));
    }

    if out.is_structured() {
        let mut value = serde_json::json!({ "items": items });
        provenance::inject_into_json(&mut value, prov, show_prov);
        out.print(&value)?;
        return Ok(());
    }

    for item in &items {
        if item.result.hierarchies.is_empty() {
            eprintln!("No members loaded for refset {}.", item.input);
        }
        for hierarchy in &item.result.hierarchies {
            println!(
                "{} | {} | {}",
                item.result.refset.id, hierarchy.hierarchy, hierarchy.count
            );
        }
    }
    provenance::print_human_footer(prov, show_prov);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (
                id             TEXT PRIMARY KEY,
                fsn            TEXT NOT NULL,
                preferred_term TEXT NOT NULL,
                hierarchy      TEXT,
                module         TEXT,
                effective_time TEXT
            );
            CREATE TABLE refset_members (
                refset_id                TEXT NOT NULL,
                referenced_component_id  TEXT NOT NULL,
                PRIMARY KEY (refset_id, referenced_component_id)
            );",
        )
        .unwrap();
        conn
    }

    fn insert_concept(conn: &Connection, id: &str, pt: &str, hierarchy: &str) {
        conn.execute(
            "INSERT INTO concepts (id, fsn, preferred_term, hierarchy, module, effective_time)
             VALUES (?1, ?2, ?3, ?4, '900000000000207008', '20260101')",
            params![id, format!("{pt} ({hierarchy})"), pt, hierarchy],
        )
        .unwrap();
    }

    fn insert_member(conn: &Connection, refset_id: &str, concept_id: &str) {
        conn.execute(
            "INSERT INTO refset_members (refset_id, referenced_component_id) VALUES (?1, ?2)",
            params![refset_id, concept_id],
        )
        .unwrap();
    }

    /// Two refsets sharing concepts 2 and 3; A also has 1, B also has 4.
    /// Concepts 1 and 3 are "Clinical finding", 2 and 4 are "Procedure".
    fn build_test_db() -> Connection {
        let conn = test_conn();
        insert_concept(&conn, "900001", "Refset A", "Foundation metadata concept");
        insert_concept(&conn, "900002", "Refset B", "Foundation metadata concept");
        insert_concept(&conn, "1", "Concept One", "Clinical finding");
        insert_concept(&conn, "2", "Concept Two", "Procedure");
        insert_concept(&conn, "3", "Concept Three", "Clinical finding");
        insert_concept(&conn, "4", "Concept Four", "Procedure");

        insert_member(&conn, "900001", "1");
        insert_member(&conn, "900001", "2");
        insert_member(&conn, "900001", "3");
        insert_member(&conn, "900002", "2");
        insert_member(&conn, "900002", "3");
        insert_member(&conn, "900002", "4");
        conn
    }

    #[test]
    fn refset_summary_found_and_missing() {
        let conn = build_test_db();
        let r = refset_summary(&conn, "900001").unwrap().unwrap();
        assert_eq!(r.preferred_term, "Refset A");
        assert_eq!(r.member_count, 3);

        assert!(refset_summary(&conn, "does-not-exist").unwrap().is_none());
    }

    #[test]
    fn unknown_refsets_have_a_stable_id_tie_breaker() {
        let conn = build_test_db();
        insert_member(&conn, "900004", "1");
        insert_member(&conn, "900003", "1");
        let ids: Vec<_> = list_refsets(&conn, None)
            .unwrap()
            .into_iter()
            .filter(|refset| refset.preferred_term == "(unknown refset)")
            .map(|refset| refset.id)
            .collect();
        assert_eq!(ids, ["900003", "900004"]);
    }

    #[test]
    fn compare_refsets_partitions_membership() {
        let conn = build_test_db();
        let cmp = compare_refsets(&conn, "900001", "900002", None).unwrap();

        assert_eq!(cmp.only_in_a.count, 1);
        assert_eq!(
            cmp.only_in_a
                .members
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1"]
        );

        assert_eq!(cmp.only_in_b.count, 1);
        assert_eq!(
            cmp.only_in_b
                .members
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["4"]
        );

        assert_eq!(cmp.in_both.count, 2);
        let mut both_ids: Vec<&str> = cmp.in_both.members.iter().map(|m| m.id.as_str()).collect();
        both_ids.sort();
        assert_eq!(both_ids, vec!["2", "3"]);
    }

    #[test]
    fn compare_refsets_respects_limit_but_reports_exact_count() {
        let conn = build_test_db();
        let cmp = compare_refsets(&conn, "900001", "900002", Some(1)).unwrap();
        // 2 members are in both, but the list is capped at 1...
        assert_eq!(cmp.in_both.members.len(), 1);
        // ...while the count still reflects the true total.
        assert_eq!(cmp.in_both.count, 2);
    }

    #[test]
    fn compare_refsets_unknown_refset_is_treated_as_empty() {
        let conn = build_test_db();
        let cmp = compare_refsets(&conn, "900001", "does-not-exist", None).unwrap();
        assert_eq!(cmp.refset_b.preferred_term, "(unknown refset)");
        // Nothing can be "in both" or "only in B" if B has no members at all.
        assert_eq!(cmp.in_both.count, 0);
        assert_eq!(cmp.only_in_b.count, 0);
        assert_eq!(cmp.only_in_a.count, 3);
    }

    #[test]
    fn profile_groups_by_hierarchy_sorted_by_count_desc() {
        let conn = build_test_db();
        let hierarchies = profile_refset_by_hierarchy(&conn, "900001").unwrap();
        // Refset A: concepts 1 (Clinical finding), 2 (Procedure), 3 (Clinical finding).
        assert_eq!(hierarchies.len(), 2);
        assert_eq!(hierarchies[0].hierarchy, "Clinical finding");
        assert_eq!(hierarchies[0].count, 2);
        assert_eq!(hierarchies[1].hierarchy, "Procedure");
        assert_eq!(hierarchies[1].count, 1);
    }

    #[test]
    fn profile_empty_refset_returns_empty_list() {
        let conn = build_test_db();
        insert_concept(&conn, "900003", "Refset C", "Foundation metadata concept");
        let hierarchies = profile_refset_by_hierarchy(&conn, "900003").unwrap();
        assert!(hierarchies.is_empty());
    }
}
