//! `sct lookup` — Look up a SNOMED CT concept by SCTID or CTV3 code.
//!
//! Accepts a bare SCTID (numeric) and returns full concept details.
//! Also accepts a CTV3 code and attempts reverse lookup via the
//! concept_maps table (requires a UK Monolith-derived database).
//!
//! Examples:
//!   sct lookup 22298006
//!   sct lookup --db snomed.db 22298006
//!   sct lookup XE0Uh

use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct Args {
    /// SCTID (numeric) or CTV3 code to look up.
    pub code: String,

    /// SQLite database produced by `sct sqlite`.
    #[arg(long, default_value = "snomed.db")]
    pub db: PathBuf,

    /// Output raw JSON instead of human-readable format.
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args) -> Result<()> {
    let conn = Connection::open(&args.db)
        .with_context(|| format!("opening database {}", args.db.display()))?;
    conn.execute_batch("PRAGMA query_only = ON;")?;

    let code = args.code.trim();

    // If the code looks numeric, try SCTID first.
    if code.chars().all(|c| c.is_ascii_digit()) {
        if let Some(concept) = lookup_sctid(&conn, code)? {
            return print_concept(&concept, args.json);
        }
        println!("Concept {code} not found.");
        return Ok(());
    }

    // Non-numeric: try CTV3 mapping.
    let mapped = lookup_ctv3(&conn, code)?;
    if mapped.is_empty() {
        println!("No SNOMED CT mapping found for CTV3 code '{code}'.");
        println!(
            "Mappings are only present when the database was built from a UK Monolith RF2 release."
        );
        return Ok(());
    }

    if mapped.len() == 1 {
        // Single mapping — show full concept detail.
        if let Some(concept) = lookup_sctid(&conn, &mapped[0].0)? {
            println!("CTV3 {code} → SCTID {}\n", mapped[0].0);
            return print_concept(&concept, args.json);
        }
    }

    // Multiple mappings — list them, then show full detail for each.
    println!(
        "CTV3 {code} maps to {} SNOMED CT concept{}:\n",
        mapped.len(),
        if mapped.len() == 1 { "" } else { "s" }
    );
    for (id, pt, fsn, hierarchy) in &mapped {
        println!("  [{id}] {pt}");
        if pt != fsn {
            let fsn_clean = fsn.rfind(" (").map(|p| &fsn[..p]).unwrap_or(fsn.as_str());
            if fsn_clean != pt {
                println!("        FSN: {fsn_clean}");
            }
        }
        println!("        {hierarchy}");
    }

    if mapped.len() > 1 {
        println!("\nUse `sct lookup <SCTID>` for full details on a specific concept.");
    }

    Ok(())
}

fn lookup_sctid(conn: &Connection, id: &str) -> Result<Option<Value>> {
    let result = conn.query_row(
        "SELECT id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
                parents, children_count, attributes, active, module, effective_time,
                ctv3_codes, read2_codes
         FROM concepts WHERE id = ?1",
        params![id],
        |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "fsn": row.get::<_, String>(1)?,
                "preferred_term": row.get::<_, String>(2)?,
                "synonyms": serde_json::from_str::<Value>(&row.get::<_, String>(3).unwrap_or_default()).unwrap_or(Value::Null),
                "hierarchy": row.get::<_, String>(4)?,
                "hierarchy_path": serde_json::from_str::<Value>(&row.get::<_, String>(5).unwrap_or_default()).unwrap_or(Value::Null),
                "parents": serde_json::from_str::<Value>(&row.get::<_, String>(6).unwrap_or_default()).unwrap_or(Value::Null),
                "children_count": row.get::<_, i64>(7)?,
                "attributes": serde_json::from_str::<Value>(&row.get::<_, String>(8).unwrap_or_default()).unwrap_or(Value::Null),
                "active": row.get::<_, bool>(9)?,
                "module": row.get::<_, String>(10)?,
                "effective_time": row.get::<_, String>(11)?,
                "ctv3_codes": serde_json::from_str::<Value>(&row.get::<_, String>(12).unwrap_or_default()).unwrap_or(json!([])),
                "read2_codes": serde_json::from_str::<Value>(&row.get::<_, String>(13).unwrap_or_default()).unwrap_or(json!([]))
            }))
        },
    );

    match result {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Reverse-lookup a CTV3 code → SNOMED concept(s) via concept_maps.
fn lookup_ctv3(conn: &Connection, code: &str) -> Result<Vec<(String, String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy
         FROM concept_maps m
         JOIN concepts c ON c.id = m.concept_id
         WHERE m.code = ?1 AND m.terminology = 'ctv3'
         ORDER BY c.id",
    )?;

    let rows: Vec<(String, String, String, String)> = stmt
        .query_map(params![code], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(rows)
}

fn print_concept(concept: &Value, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(concept)?);
        return Ok(());
    }

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
    if pt != fsn {
        let fsn_clean = fsn.rfind(" (").map(|p| &fsn[..p]).unwrap_or(fsn);
        if fsn_clean != pt {
            println!("  FSN: {fsn_clean}");
        }
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

    // Metadata
    println!("  Module: {module}");
    println!("  Effective: {effective_time}");

    Ok(())
}
