// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

use sct_rs::sdk::{Snomed, Terminology};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "snomed.db".to_string());
    let mut snomed = Snomed::open(path)?;
    if let Some(fst) = args.next() {
        snomed.attach_fst(fst)?;
    }

    if let Some(provenance) = snomed.provenance() {
        println!("{} {}", provenance.edition_label, provenance.release_date);
    }

    if let Some(concept) = snomed.concept("22298006")? {
        println!("[{}] {}", concept.id, concept.preferred_term);
    }

    for hit in snomed.search("heart attack", 10)? {
        println!("[{}] {}", hit.id, hit.preferred_term);
    }

    let descendants = snomed.expand("<<73211009")?;
    println!("Diabetes hierarchy: {} concepts", descendants.len());

    let children = snomed.children("73211009", 20)?;
    println!("Diabetes direct children: {}", children.len());

    for mapping in snomed.map(Terminology::Snomed, "22298006", Terminology::Icd10)? {
        println!("ICD-10: {}", mapping.target);
    }

    println!("Loaded refsets: {}", snomed.refsets()?.len());

    if snomed.has_fst() {
        for hit in snomed.autocomplete("myoc", 10, true)? {
            println!("Autocomplete: [{}] {}", hit.id, hit.display);
        }
    }

    Ok(())
}
