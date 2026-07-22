// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

use sct_rs::sdk::{Snomed, Terminology};
use std::path::Path;

fn exercise_sdk(db: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let snomed = Snomed::open(db)?;
    let _concept = snomed.concept("22298006")?;
    let _hits = snomed.search("heart attack", 20)?;
    let _expanded = snomed.expand("<<73211009")?;
    let _children = snomed.children("73211009", 20)?;
    let _relationship = snomed.subsumes("73211009", "46635009")?;
    let _mappings = snomed.map(
        Terminology::Snomed,
        "22298006",
        Terminology::Icd10,
    )?;
    Ok(())
}

fn main() {
    let _ = exercise_sdk;
}
