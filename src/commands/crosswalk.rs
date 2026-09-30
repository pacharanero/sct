// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cross-terminology equivalents engine (behind `sct map`, alias `crosswalk`):
//! show all cross-terminology equivalents of a single code at
//! once (the text equivalent of the NHS Data Migration Workbench's
//! tri-terminology BROWSE view). Built on the same maps as `sct transcode`.

use anyhow::{bail, Result};
use rusqlite::Connection;

use crate::commands::transcode::{transcode_one, SYSTEMS};

/// All cross-terminology equivalents of one code: its SNOMED pivot + preferred
/// term, and the equivalent code(s) in every other terminology.
pub struct Crosswalk {
    pub snomed: String,
    pub display: String,
    /// `(system, codes)` for every terminology other than the source.
    pub equivalents: Vec<(&'static str, Vec<String>)>,
}

/// Resolve every cross-terminology equivalent of `code` (in terminology `from`).
/// The pure core of `sct crosswalk` (no I/O), exposed for tests and reuse.
pub fn equivalents(conn: &Connection, from: &str, code: &str) -> Result<Crosswalk> {
    if !SYSTEMS.contains(&from) {
        bail!(
            "unknown terminology {from:?}; expected one of {}",
            SYSTEMS.join(", ")
        );
    }
    let pivots = transcode_one(conn, from, code, "snomed", false)?;
    let (snomed, display) = pivots
        .first()
        .map(|m| (m.snomed.clone(), m.display.clone().unwrap_or_default()))
        .unwrap_or_default();

    let classification_maps = crate::provenance::classification_maps_loaded(conn);
    let mut equivalents = Vec::new();
    for to in SYSTEMS {
        if to == from {
            continue;
        }
        // Without loaded evidence a classification column is unknown, not empty;
        // `sct map` says so on stderr, so leave the column blank rather than fail.
        if crate::mapping::is_classification(to) && !classification_maps {
            equivalents.push((to, Vec::new()));
            continue;
        }
        let mut codes: Vec<String> = transcode_one(conn, from, code, to, false)?
            .into_iter()
            .map(|m| m.target)
            .collect();
        codes.sort();
        codes.dedup();
        equivalents.push((to, codes));
    }
    Ok(Crosswalk {
        snomed,
        display,
        equivalents,
    })
}
