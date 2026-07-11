// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct dmwb` - read NHS Data Migration Workbench `.mdb` (Microsoft Access)
//! files via the pure-Rust `jetdb` reader. Source of the DMWB-unique maps that
//! are not in standard RF2 (chiefly the Read v2 cross-maps). Feature-gated on
//! `dmwb`. See `spec/cross-terminology-mapping.md`.
//!
//! NHS terminology/map data is Crown Copyright under the Open Government Licence;
//! `sct` reads the user's own TRUD-acquired files locally and never redistributes
//! them.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use jetdb::{read_catalog, read_table_def, read_table_rows, PageReader, Value};

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: DmwbCommand,
}

#[derive(Subcommand, Debug)]
pub enum DmwbCommand {
    /// List the tables in a DMWB `.mdb` file (introspection / validation).
    Tables {
        /// Path to a DMWB `.mdb` file.
        #[arg(value_parser = crate::paths::tilde_pathbuf)]
        mdb: PathBuf,
    },
    /// Print the columns and first rows of a table (introspection / validation).
    Dump {
        /// Path to a DMWB `.mdb` file.
        #[arg(value_parser = crate::paths::tilde_pathbuf)]
        mdb: PathBuf,
        /// Table name (e.g. RCTSCTMAP).
        table: String,
        /// Number of rows to print.
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        DmwbCommand::Tables { mdb } => tables(&mdb),
        DmwbCommand::Dump { mdb, table, limit } => dump(&mdb, &table, limit),
    }
}

fn open(mdb: &std::path::Path) -> Result<PageReader> {
    PageReader::open(mdb).with_context(|| format!("opening {}", mdb.display()))
}

fn tables(mdb: &std::path::Path) -> Result<()> {
    let mut reader = open(mdb)?;
    let catalog = read_catalog(&mut reader).context("reading catalog")?;
    for entry in &catalog {
        if !entry.name.starts_with("MSys") {
            println!("{}", entry.name);
        }
    }
    Ok(())
}

fn dump(mdb: &std::path::Path, table: &str, limit: usize) -> Result<()> {
    let mut reader = open(mdb)?;
    let catalog = read_catalog(&mut reader).context("reading catalog")?;
    let entry = catalog
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(table))
        .with_context(|| format!("table {table:?} not found"))?;
    let def = read_table_def(&mut reader, &entry.name, entry.table_page)
        .with_context(|| format!("reading table def for {table}"))?;
    let cols: Vec<String> = def.columns.iter().map(|c| c.name.clone()).collect();
    println!("columns: {}", cols.join("\t"));

    println!(
        "column types: {}",
        def.columns
            .iter()
            .map(|c| format!("{}={:?}", c.name, c.col_type))
            .collect::<Vec<_>>()
            .join("  ")
    );

    let result = read_table_rows(&mut reader, &def).context("reading rows")?;
    println!("rows: {}", result.rows.len());
    let binary_cols: Vec<&str> = def
        .columns
        .iter()
        .filter(|c| format!("{:?}", c.col_type) == "Binary")
        .map(|c| c.name.as_str())
        .collect();
    if !binary_cols.is_empty() {
        eprintln!(
            "note: column(s) {} are Binary; jetdb 0.3 does not decode Binary cells \
             (returns empty). DMWB stores the Read v2 code in a Binary `SCUI` column, \
             so it is not yet importable this way - see spec/cross-terminology-mapping.md.",
            binary_cols.join(", ")
        );
    }
    for row in result.rows.iter().take(limit) {
        let cells: Vec<String> = row.iter().map(value_to_string).collect();
        println!("{}", cells.join("\t"));
    }
    Ok(())
}

/// Best-effort string rendering of a jetdb cell value.
pub fn value_to_string(v: &Value) -> String {
    match v {
        Value::Text(s) => s.clone(),
        Value::Long(n) => n.to_string(),
        Value::Byte(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => format!("{other:?}"),
    }
}
