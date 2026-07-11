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

// ---------------------------------------------------------------------------
// CLI entry points
// ---------------------------------------------------------------------------

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Command::List(a) => run_list(a),
        Command::Info(a) => run_info(a),
        Command::Members(a) => run_members(a),
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

    let meta = conn
        .query_row(
            "SELECT c.id, c.preferred_term, c.fsn, c.module,
                    (SELECT COUNT(*) FROM refset_members WHERE refset_id = c.id)
             FROM concepts c
             WHERE c.id = ?1",
            params![args.id],
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
