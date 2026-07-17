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
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::builder::strip_semantic_tag;
use crate::format::{ConceptFields, ConceptFormat};
use crate::output::OutputFormat;
use crate::provenance::{self, OutputMode, ProvenanceFlags};

/// Sentinel passed to SQLite `LIMIT ?` meaning "no limit".
const SQLITE_NO_LIMIT: i64 = -1;

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
    /// Default: `{id} | {pt} ({count} members)`. See `docs/commands/refset.md`.
    #[arg(long)]
    pub template: Option<String>,

    #[command(flatten)]
    pub prov: ProvenanceFlags,
}

#[derive(Parser, Debug)]
pub struct InfoArgs {
    /// SCTID of the refset (which is itself a SNOMED CT concept).
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
    /// SCTID of the refset.
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
    #[arg(long)]
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
    /// SCTID of the refset.
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
// Shared query helpers (also used by src/commands/mcp.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct RefsetSummary {
    pub id: String,
    pub preferred_term: String,
    pub fsn: String,
    pub module: String,
    pub member_count: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct RefsetMember {
    pub id: String,
    pub preferred_term: String,
    pub fsn: String,
    pub hierarchy: String,
    pub effective_time: String,
}

/// List all refsets with at least one loaded member, ordered by preferred term.
/// Pass `limit = None` for no limit.
pub(crate) fn list_refsets(conn: &Connection, limit: Option<i64>) -> Result<Vec<RefsetSummary>> {
    let mut stmt = conn.prepare(
        "SELECT rm.refset_id,
                COALESCE(c.preferred_term, '(unknown refset)'),
                COALESCE(c.fsn, ''),
                COALESCE(c.module, ''),
                COUNT(*) AS n
         FROM refset_members rm
         LEFT JOIN concepts c ON c.id = rm.refset_id
         GROUP BY rm.refset_id
         ORDER BY c.preferred_term
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit.unwrap_or(SQLITE_NO_LIMIT)], |row| {
            Ok(RefsetSummary {
                id: row.get(0)?,
                preferred_term: row.get(1)?,
                fsn: row.get(2)?,
                module: row.get(3)?,
                member_count: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// List concepts belonging to a refset, ordered by preferred term.
/// Pass `limit = None` for no limit.
pub(crate) fn list_refset_members(
    conn: &Connection,
    refset_id: &str,
    limit: Option<i64>,
) -> Result<Vec<RefsetMember>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy, c.effective_time
         FROM refset_members rm
         JOIN concepts c ON c.id = rm.referenced_component_id
         WHERE rm.refset_id = ?1
         ORDER BY c.preferred_term
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(
            params![refset_id, limit.unwrap_or(SQLITE_NO_LIMIT)],
            |row| {
                Ok(RefsetMember {
                    id: row.get(0)?,
                    preferred_term: row.get(1)?,
                    fsn: row.get(2)?,
                    hierarchy: row.get(3)?,
                    effective_time: row.get(4)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Look up a single refset's metadata + member count. `None` if the id isn't
/// a concept in the database at all (distinct from a concept with 0 members).
pub(crate) fn refset_summary(conn: &Connection, id: &str) -> Result<Option<RefsetSummary>> {
    let r = conn
        .query_row(
            "SELECT c.id, c.preferred_term, c.fsn, c.module,
                    (SELECT COUNT(*) FROM refset_members WHERE refset_id = c.id)
             FROM concepts c
             WHERE c.id = ?1",
            params![id],
            |row| {
                Ok(RefsetSummary {
                    id: row.get(0)?,
                    preferred_term: row.get(1)?,
                    fsn: row.get(2)?,
                    module: row.get(3)?,
                    member_count: row.get(4)?,
                })
            },
        )
        .ok();
    Ok(r)
}

/// Which side of a two-refset membership comparison a query targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffSet {
    /// In `a`, not in `b`.
    OnlyA,
    /// In `b`, not in `a`.
    OnlyB,
    /// In both `a` and `b`.
    Both,
}

impl DiffSet {
    /// SQL keyword for the correlated subquery clause: `NOT EXISTS` for the
    /// "only in one side" sets, `EXISTS` for the intersection.
    fn sql_condition(self) -> &'static str {
        match self {
            DiffSet::OnlyA | DiffSet::OnlyB => "NOT EXISTS",
            DiffSet::Both => "EXISTS",
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct RefsetDiffSet {
    pub count: i64,
    pub members: Vec<RefsetMember>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RefsetComparison {
    pub refset_a: RefsetSummary,
    pub refset_b: RefsetSummary,
    pub only_in_a: RefsetDiffSet,
    pub only_in_b: RefsetDiffSet,
    pub in_both: RefsetDiffSet,
}

/// Count + (optionally limited) member list for one side of a refset
/// membership comparison. `primary`/`other` are refset ids; for `Both` the
/// order doesn't matter, for `OnlyA`/`OnlyB` `primary` is the refset the
/// members must belong to and `other` is the one they must be absent from.
fn refset_diff_set(
    conn: &Connection,
    primary: &str,
    other: &str,
    which: DiffSet,
    limit: Option<i64>,
) -> Result<RefsetDiffSet> {
    let exists = which.sql_condition();
    let count_sql = format!(
        "SELECT COUNT(*)
         FROM refset_members rm
         WHERE rm.refset_id = ?1
           AND {exists} (
               SELECT 1 FROM refset_members rm2
               WHERE rm2.refset_id = ?2 AND rm2.referenced_component_id = rm.referenced_component_id
           )"
    );
    let count: i64 = conn.query_row(&count_sql, params![primary, other], |row| row.get(0))?;

    let members_sql = format!(
        "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy, c.effective_time
         FROM refset_members rm
         JOIN concepts c ON c.id = rm.referenced_component_id
         WHERE rm.refset_id = ?1
           AND {exists} (
               SELECT 1 FROM refset_members rm2
               WHERE rm2.refset_id = ?2 AND rm2.referenced_component_id = rm.referenced_component_id
           )
         ORDER BY c.preferred_term
         LIMIT ?3"
    );
    let mut stmt = conn.prepare(&members_sql)?;
    let members = stmt
        .query_map(
            params![primary, other, limit.unwrap_or(SQLITE_NO_LIMIT)],
            |row| {
                Ok(RefsetMember {
                    id: row.get(0)?,
                    preferred_term: row.get(1)?,
                    fsn: row.get(2)?,
                    hierarchy: row.get(3)?,
                    effective_time: row.get(4)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(RefsetDiffSet { count, members })
}

/// Compare membership of two refsets. Pass `limit = None` to list every
/// member of each set; the reported `count` is always exact regardless of
/// `limit` (it comes from a separate `COUNT(*)` query, not `members.len()`).
pub(crate) fn compare_refsets(
    conn: &Connection,
    id_a: &str,
    id_b: &str,
    limit: Option<i64>,
) -> Result<RefsetComparison> {
    Ok(RefsetComparison {
        refset_a: refset_summary(conn, id_a)?.unwrap_or(RefsetSummary {
            id: id_a.to_string(),
            preferred_term: "(unknown refset)".into(),
            fsn: String::new(),
            module: String::new(),
            member_count: 0,
        }),
        refset_b: refset_summary(conn, id_b)?.unwrap_or(RefsetSummary {
            id: id_b.to_string(),
            preferred_term: "(unknown refset)".into(),
            fsn: String::new(),
            module: String::new(),
            member_count: 0,
        }),
        only_in_a: refset_diff_set(conn, id_a, id_b, DiffSet::OnlyA, limit)?,
        only_in_b: refset_diff_set(conn, id_b, id_a, DiffSet::OnlyB, limit)?,
        in_both: refset_diff_set(conn, id_a, id_b, DiffSet::Both, limit)?,
    })
}

#[derive(Debug, Serialize)]
pub(crate) struct HierarchyCount {
    pub hierarchy: String,
    pub count: i64,
}

/// Breakdown of a refset's members by top-level hierarchy, ordered by count
/// descending (ties broken by hierarchy name for stable output).
pub(crate) fn profile_refset_by_hierarchy(
    conn: &Connection,
    refset_id: &str,
) -> Result<Vec<HierarchyCount>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(c.hierarchy, ''), '(unknown)') AS h, COUNT(*) AS n
         FROM refset_members rm
         JOIN concepts c ON c.id = rm.referenced_component_id
         WHERE rm.refset_id = ?1
         GROUP BY h
         ORDER BY n DESC, h ASC",
    )?;
    let rows = stmt
        .query_map(params![refset_id], |row| {
            Ok(HierarchyCount {
                hierarchy: row.get(0)?,
                count: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
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

fn open_db(path: &Path) -> Result<Connection> {
    crate::commands::open_db_readonly(path, None)
}

fn run_list(args: ListArgs) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = open_db(&db)?;
    let prov = provenance::read_sqlite(&conn).unwrap_or(None);
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let rows = list_refsets(&conn, None)?;

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

    let format = ConceptFormat {
        line: "{id} | {pt} ({count} members)".into(),
        fsn_suffix: String::new(),
    }
    .with_overrides(args.template, Some(String::new()));

    for r in &rows {
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
    }
    provenance::print_human_footer(prov.as_ref(), show_prov);
    Ok(())
}

fn run_info(args: InfoArgs) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = open_db(&db)?;
    let prov = provenance::read_sqlite(&conn).unwrap_or(None);
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let meta = refset_summary(&conn, &args.id)?;

    let r = match meta {
        Some(r) => r,
        None => {
            println!("Refset {} not found in concepts table.", args.id);
            return Ok(());
        }
    };

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
    let conn = open_db(&db)?;
    let prov = provenance::read_sqlite(&conn).unwrap_or(None);
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let rows = list_refset_members(&conn, &args.id, args.limit.map(|n| n as i64))?;

    // `--ids`: machine output for pipes - just member SCTIDs on stdout.
    if args.ids {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        for m in &rows {
            writeln!(out, "{}", m.id)?;
        }
        return Ok(());
    }

    if rows.is_empty() && !out.is_structured() {
        println!("No members found for refset {}.", args.id);
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
    let conn = open_db(&db)?;
    let prov = provenance::read_sqlite(&conn).unwrap_or(None);
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let cmp = compare_refsets(&conn, &args.id_a, &args.id_b, args.limit.map(|n| n as i64))?;

    if out.is_structured() {
        let mut value = serde_json::to_value(&cmp)?;
        provenance::inject_into_json(&mut value, prov.as_ref(), show_prov);
        if let Some(s) = out.render(&value)? {
            println!("{s}");
        }
        return Ok(());
    }

    println!("  A: [{}] {}", cmp.refset_a.id, cmp.refset_a.preferred_term);
    println!("  B: [{}] {}", cmp.refset_b.id, cmp.refset_b.preferred_term);
    println!();
    println!("  Only in A: {}", cmp.only_in_a.count);
    println!("  Only in B: {}", cmp.only_in_b.count);
    println!("  In both:   {}", cmp.in_both.count);

    let format = ConceptFormat::load();
    let print_set = |label: &str, set: &RefsetDiffSet| {
        println!("\n  {label} ({}):", set.count);
        for m in &set.members {
            println!(
                "    {}",
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
    let conn = open_db(&db)?;
    let prov = provenance::read_sqlite(&conn).unwrap_or(None);
    let out = args.format.or_json_flag(args.json);
    let mode = if out.is_structured() {
        OutputMode::Json
    } else {
        OutputMode::HumanText
    };
    let show_prov = provenance::should_show(args.prov, mode);

    let refset = refset_summary(&conn, &args.id)?;
    let refset = match refset {
        Some(r) => r,
        None => {
            println!("Refset {} not found in concepts table.", args.id);
            return Ok(());
        }
    };
    let hierarchies = profile_refset_by_hierarchy(&conn, &args.id)?;

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

    println!("  [{}] {}", refset.id, refset.preferred_term);
    println!("  Members: {}", refset.member_count);

    if hierarchies.is_empty() {
        println!("\n  No members loaded for this refset.");
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
