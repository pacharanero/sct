//! `sct refset` — Inspect SNOMED CT simple reference sets loaded into SQLite.
//!
//! Refsets are themselves concepts in SNOMED CT, so metadata (preferred term,
//! module, FSN) is looked up from the `concepts` table by JOINing on
//! `refset_members.refset_id`.
//!
//! Subcommands:
//!   list     — all refsets that have at least one member, with member counts
//!   info     — metadata + member count for a single refset
//!   members  — concepts in a given refset
//!
//! The [`list_refsets`] and [`list_refset_members`] query helpers are shared
//! with the `sct mcp` server so the two surfaces always return the same data.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::PathBuf;

use crate::builder::strip_semantic_tag;
use crate::format::{ConceptFields, ConceptFormat};

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
    /// SQLite database produced by `sct sqlite`.
    #[arg(long, default_value = "snomed.db")]
    pub db: PathBuf,

    /// Output raw JSON instead of a human-readable table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct InfoArgs {
    /// SCTID of the refset (which is itself a SNOMED CT concept).
    pub id: String,

    /// SQLite database produced by `sct sqlite`.
    #[arg(long, default_value = "snomed.db")]
    pub db: PathBuf,

    /// Output raw JSON instead of a human-readable summary.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct MembersArgs {
    /// SCTID of the refset.
    pub id: String,

    /// SQLite database produced by `sct sqlite`.
    #[arg(long, default_value = "snomed.db")]
    pub db: PathBuf,

    /// Maximum number of members to display (default: all).
    #[arg(long)]
    pub limit: Option<usize>,

    /// Output raw JSON instead of a human-readable list.
    #[arg(long)]
    pub json: bool,

    /// Override the per-concept line template. See `sct help format` or
    /// `docs/commands/refset.md` for the variable list.
    #[arg(long)]
    pub format: Option<String>,

    /// Override the FSN suffix template (rendered only when FSN differs from PT).
    /// Pass an empty string (`--format-fsn-suffix ""`) to suppress it entirely.
    #[arg(long)]
    pub format_fsn_suffix: Option<String>,
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

fn open_db(path: &PathBuf) -> Result<Connection> {
    let conn =
        Connection::open(path).with_context(|| format!("opening database {}", path.display()))?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    Ok(conn)
}

fn run_list(args: ListArgs) -> Result<()> {
    let conn = open_db(&args.db)?;
    let rows = list_refsets(&conn, None)?;

    if rows.is_empty() {
        println!(
            "No refset members loaded. Rebuild the database with `sct ndjson --refsets simple` \
             and `sct sqlite` from an RF2 release that includes simple refset files."
        );
        return Ok(());
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    println!("{} refset(s):\n", rows.len());
    for r in &rows {
        println!(
            "  [{}] {}  ({} members)",
            r.id, r.preferred_term, r.member_count
        );
    }
    Ok(())
}

fn run_info(args: InfoArgs) -> Result<()> {
    let conn = open_db(&args.db)?;

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

    if r.member_count == 0 {
        println!(
            "Concept [{}] {} exists but has no loaded members.\n\
             (It may not be a refset, or its members weren't included in the RF2 load.)",
            r.id, r.preferred_term
        );
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&r)?);
        return Ok(());
    }

    println!("  [{}] {}", r.id, r.preferred_term);
    let fsn_clean = strip_semantic_tag(&r.fsn);
    if fsn_clean != r.preferred_term && !r.fsn.is_empty() {
        println!("  FSN: {fsn_clean}");
    }
    println!("  Module:  {}", r.module);
    println!("  Members: {}", r.member_count);
    Ok(())
}

fn run_members(args: MembersArgs) -> Result<()> {
    let conn = open_db(&args.db)?;
    let rows = list_refset_members(&conn, &args.id, args.limit.map(|n| n as i64))?;

    if rows.is_empty() {
        println!("No members found for refset {}.", args.id);
        return Ok(());
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    let format = ConceptFormat::load().with_overrides(args.format, args.format_fsn_suffix);
    for m in &rows {
        println!(
            "{}",
            format.render(&ConceptFields {
                id: &m.id,
                pt: &m.preferred_term,
                fsn: &m.fsn,
                hierarchy: &m.hierarchy,
                module: "",
                effective_time: &m.effective_time,
            })
        );
    }
    Ok(())
}
