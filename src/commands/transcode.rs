// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CLI input handling and compatibility exports for cross-terminology mapping.

use anyhow::{Context, Result};
use std::io::BufRead;

pub(crate) use crate::mapping::{is_classification, table_exists, SYSTEMS};
pub use crate::mapping::{transcode_one, Mapped};

/// Read codes from a file or stdin. The leading whitespace-delimited token of
/// each non-blank, non-`#` line is taken as the code (so `sct ecl expand`,
/// `cut`, `grep` output pipes straight in).
pub(crate) fn read_codes(input: Option<&std::path::Path>) -> Result<Vec<String>> {
    let reader: Box<dyn BufRead> = match input {
        Some(p) => Box::new(std::io::BufReader::new(
            std::fs::File::open(p).with_context(|| format!("opening {}", p.display()))?,
        )),
        None => Box::new(std::io::BufReader::new(std::io::stdin())),
    };
    let mut codes = Vec::new();
    for line in reader.lines() {
        let line = line.context("reading input")?;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(tok) = t.split_whitespace().next() {
            codes.push(tok.to_string());
        }
    }
    Ok(codes)
}
