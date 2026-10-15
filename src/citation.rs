// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Validated, reusable citations for provenance-bearing sct artefacts.

use std::collections::HashSet;

use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, Datelike, NaiveDate};
use serde::Serialize;

use crate::provenance::{extract_release_date, CompanionArtifact, Provenance};
use crate::text::single_line;

pub const TERMINOLOGY_ENTRY_ID: &str = "sct-terminology-source";
pub const ARTEFACT_ENTRY_ID: &str = "sct-local-artefact";
pub const SOFTWARE_ENTRY_ID: &str = "sct-software";

const SOFTWARE_NAME: &str = "sct";
const SOFTWARE_REPOSITORY: &str = "https://github.com/pacharanero/sct";
const SOFTWARE_AUTHOR_GIVEN: &str = "Marcus";
const SOFTWARE_AUTHOR_FAMILY: &str = "Baw";
const TERMINOLOGY_EVIDENCE_NOTE: &str = "The recorded edition, release identifier, and release date were inferred during ingestion from RF2 input naming and are not publisher verification.";

/// A validated citation assembled from one artefact's recorded provenance.
///
/// Source paths are deliberately not retained. `composite` records only the
/// evidence limitation needed to interpret a build made from multiple inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Citation {
    pub terminology: TerminologyCitation,
    pub artefact: ArtefactCitation,
    pub software: SoftwareCitation,
    pub data_availability: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminologyCitation {
    pub title: String,
    pub edition_label: Option<String>,
    pub release_id: Option<String>,
    pub release_date: Option<String>,
    pub publisher: Option<String>,
    pub access_url: Option<String>,
    pub evidence_note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtefactCitation {
    pub title: String,
    pub content_fingerprint: Option<String>,
    pub companions: Vec<CompanionCitation>,
    pub created_at: Option<String>,
    pub composite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompanionCitation {
    pub kind: String,
    pub schema_version: u32,
    pub record_count: u64,
    pub content_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SoftwareCitation {
    pub name: String,
    pub author: CitationAuthor,
    pub repository: String,
    pub producer_version: Option<String>,
    pub reader_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CitationAuthor {
    pub family: String,
    pub given: String,
}

impl Citation {
    /// Build a citation using this library version as the reader version.
    pub fn from_provenance(provenance: &Provenance) -> Result<Self> {
        Self::from_provenance_with_reader_version(provenance, env!("CARGO_PKG_VERSION"))
    }

    /// Build a citation while explicitly recording the software version reading
    /// the artefact. This is separate from the producer version in provenance.
    pub fn from_provenance_with_reader_version(
        provenance: &Provenance,
        reader_version: &str,
    ) -> Result<Self> {
        validate_provenance(provenance)?;

        let edition_label = nonempty(&provenance.edition_label);
        let release_id = nonempty(&provenance.release_id);
        let release_date = nonempty(&provenance.release_date);
        let producer_version = nonempty(&provenance.sct_version);
        let reader_version = nonempty(reader_version);
        let created_at = nonempty(&provenance.created_at);
        let (publisher, access_url, access_authority) = publisher_mapping(edition_label.as_deref());

        let terminology_title = edition_label
            .as_deref()
            .filter(|edition| *edition != "unknown")
            .map(|edition| {
                if edition.ends_with("Edition") {
                    format!("SNOMED CT {edition}")
                } else {
                    format!("SNOMED CT {edition} Edition")
                }
            })
            .unwrap_or_else(|| "SNOMED CT terminology source".to_string());
        let artefact_title = match release_id.as_deref() {
            Some(id) => format!("Local sct-derived artefact from {id}"),
            None => "Local sct-derived SNOMED CT artefact".to_string(),
        };

        let mut notes = Vec::new();
        if edition_label
            .as_deref()
            .is_none_or(|edition| edition == "unknown")
        {
            notes.push(
                "The terminology edition is unavailable or unclassified; no publisher mapping is asserted."
                    .to_string(),
            );
        }
        if release_id.is_none() {
            notes.push("The terminology release identifier is unavailable.".to_string());
        }
        if release_date.is_none() {
            notes.push("The terminology release date is unavailable.".to_string());
        }
        if provenance.content_fingerprint.is_none() {
            notes.push(
                "The canonical concept fingerprint is unavailable, so this citation does not exactly identify the local concept content."
                    .to_string(),
            );
        }
        if producer_version.is_none() {
            notes.push("The sct producer version is unavailable.".to_string());
        }
        if created_at.is_none() {
            notes.push("The artefact creation time is unavailable.".to_string());
        }
        let composite = provenance
            .source_paths
            .iter()
            .filter(|path| !path.trim().is_empty())
            .count()
            > 1;
        if composite {
            notes.push(
                "This is a composite build. Existing provenance identifies only the primary terminology release; the constituent source identities were not independently recorded."
                    .to_string(),
            );
        }

        let data_availability = format!(
            "This citation identifies a local derived artefact; it does not publish or redistribute SNOMED CT content. Access to the terminology source is governed by {access_authority}. Stored fingerprints identify recorded canonical content and companion streams but do not independently verify every current SQLite row."
        );

        Ok(Self {
            terminology: TerminologyCitation {
                title: terminology_title,
                edition_label,
                release_id,
                release_date,
                publisher: publisher.map(str::to_string),
                access_url: access_url.map(str::to_string),
                evidence_note: TERMINOLOGY_EVIDENCE_NOTE.to_string(),
            },
            artefact: ArtefactCitation {
                title: artefact_title,
                content_fingerprint: provenance.content_fingerprint.clone(),
                companions: provenance
                    .companions
                    .iter()
                    .map(CompanionCitation::from)
                    .collect(),
                created_at,
                composite,
            },
            software: SoftwareCitation {
                name: SOFTWARE_NAME.to_string(),
                author: CitationAuthor {
                    family: SOFTWARE_AUTHOR_FAMILY.to_string(),
                    given: SOFTWARE_AUTHOR_GIVEN.to_string(),
                },
                repository: SOFTWARE_REPOSITORY.to_string(),
                producer_version,
                reader_version,
            },
            data_availability,
            notes,
        })
    }

    pub fn render_text(&self) -> String {
        render_text(self)
    }

    pub fn render_bibtex(&self) -> String {
        render_bibtex(self)
    }

    pub fn render_csl_json(&self) -> Result<String> {
        render_csl_json(self)
    }
}

impl From<&CompanionArtifact> for CompanionCitation {
    fn from(value: &CompanionArtifact) -> Self {
        Self {
            kind: value.kind.clone(),
            schema_version: value.schema_version,
            record_count: value.record_count,
            content_fingerprint: value.content_fingerprint.clone(),
        }
    }
}

/// Render a terminal-safe plain-text citation with distinct evidence sections.
pub fn render_text(citation: &Citation) -> String {
    let mut output = String::new();
    output.push_str("Terminology source\n");
    text_field(&mut output, "Title", &citation.terminology.title);
    text_optional(
        &mut output,
        "Edition",
        citation.terminology.edition_label.as_deref(),
    );
    text_optional(
        &mut output,
        "Release identifier",
        citation.terminology.release_id.as_deref(),
    );
    text_optional(
        &mut output,
        "Release date",
        citation.terminology.release_date.as_deref(),
    );
    text_optional(
        &mut output,
        "Publisher",
        citation.terminology.publisher.as_deref(),
    );
    text_optional(
        &mut output,
        "Access",
        citation.terminology.access_url.as_deref(),
    );
    text_field(&mut output, "Evidence", &citation.terminology.evidence_note);

    output.push_str("\nLocal derived artefact\n");
    text_field(&mut output, "Title", &citation.artefact.title);
    text_optional(
        &mut output,
        "Canonical concept fingerprint",
        citation.artefact.content_fingerprint.as_deref(),
    );
    for companion in &citation.artefact.companions {
        text_field(
            &mut output,
            "Companion",
            &format!(
                "{}; schema {}; {} records; {}",
                companion.kind,
                companion.schema_version,
                companion.record_count,
                companion.content_fingerprint
            ),
        );
    }
    text_optional(
        &mut output,
        "Created",
        citation.artefact.created_at.as_deref(),
    );

    output.push_str("\nSoftware\n");
    text_field(
        &mut output,
        "Author",
        &format!(
            "{} {}",
            citation.software.author.given, citation.software.author.family
        ),
    );
    text_optional(
        &mut output,
        "Producer",
        citation
            .software
            .producer_version
            .as_deref()
            .map(|version| format!("{} {version}", citation.software.name))
            .as_deref(),
    );
    text_optional(
        &mut output,
        "Reader",
        citation
            .software
            .reader_version
            .as_deref()
            .map(|version| format!("{} {version}", citation.software.name))
            .as_deref(),
    );
    text_field(&mut output, "Repository", &citation.software.repository);

    output.push_str("\nData availability\n");
    output.push_str("  ");
    output.push_str(&single_line(&citation.data_availability));
    output.push('\n');
    if !citation.notes.is_empty() {
        output.push_str("\nNotes\n");
        for note in &citation.notes {
            output.push_str("  - ");
            output.push_str(&single_line(note));
            output.push('\n');
        }
    }
    output
}

/// Render three stable, distinct BibTeX entries from the citation model.
pub fn render_bibtex(citation: &Citation) -> String {
    let mut entries = Vec::new();

    let mut terminology = vec![("title", citation.terminology.title.as_str())];
    push_optional(
        &mut terminology,
        "publisher",
        citation.terminology.publisher.as_deref(),
    );
    push_optional(
        &mut terminology,
        "year",
        citation
            .terminology
            .release_date
            .as_deref()
            .and_then(|date| date.get(..4)),
    );
    push_optional(
        &mut terminology,
        "url",
        citation.terminology.access_url.as_deref(),
    );
    let source_note = join_nonempty([
        citation
            .terminology
            .edition_label
            .as_deref()
            .map(|value| format!("Edition: {value}")),
        citation
            .terminology
            .release_id
            .as_deref()
            .map(|value| format!("Release identifier: {value}")),
        citation
            .terminology
            .release_date
            .as_deref()
            .map(|value| format!("Release date: {value}")),
        Some(citation.terminology.evidence_note.clone()),
    ]);
    if !source_note.is_empty() {
        terminology.push(("note", source_note.as_str()));
    }
    entries.push(bibtex_entry(TERMINOLOGY_ENTRY_ID, &terminology));

    let mut artefact = vec![("title", citation.artefact.title.as_str())];
    let mut artefact_notes = vec!["Local derived artefact; not a published dataset".to_string()];
    if let Some(fingerprint) = &citation.artefact.content_fingerprint {
        artefact_notes.push(format!("Canonical concept fingerprint: {fingerprint}"));
    }
    artefact_notes.extend(citation.artefact.companions.iter().map(|companion| {
        format!(
            "Companion {} (schema {}, {} records) fingerprint: {}",
            companion.kind,
            companion.schema_version,
            companion.record_count,
            companion.content_fingerprint
        )
    }));
    if let Some(created_at) = &citation.artefact.created_at {
        artefact_notes.push(format!("Created: {created_at}"));
    }
    artefact_notes.extend(citation.notes.iter().cloned());
    artefact_notes.push(citation.data_availability.clone());
    let artefact_note = artefact_notes.join("; ");
    artefact.push(("note", artefact_note.as_str()));
    entries.push(bibtex_entry(ARTEFACT_ENTRY_ID, &artefact));

    let software_author = format!(
        "{}, {}",
        citation.software.author.family, citation.software.author.given
    );
    let mut software = vec![
        ("title", citation.software.name.as_str()),
        ("author", software_author.as_str()),
        ("url", citation.software.repository.as_str()),
    ];
    push_optional(
        &mut software,
        "version",
        citation.software.producer_version.as_deref(),
    );
    let software_note = match (
        citation.software.producer_version.as_deref(),
        citation.software.reader_version.as_deref(),
    ) {
        (Some(producer), Some(reader)) => {
            format!("Producer version: {producer}; reader version: {reader}")
        }
        (Some(producer), None) => {
            format!("Producer version: {producer}; reader version unavailable")
        }
        (None, Some(reader)) => format!("Producer version unavailable; reader version: {reader}"),
        (None, None) => "Producer and reader versions unavailable".to_string(),
    };
    software.push(("note", software_note.as_str()));
    entries.push(bibtex_entry(SOFTWARE_ENTRY_ID, &software));

    entries.join("\n\n") + "\n"
}

/// Render a CSL-JSON bibliography array from the citation model.
pub fn render_csl_json(citation: &Citation) -> Result<String> {
    let terminology_note = join_nonempty([
        citation
            .terminology
            .edition_label
            .as_deref()
            .map(|value| format!("Edition: {value}")),
        citation
            .terminology
            .release_id
            .as_deref()
            .map(|value| format!("Release identifier: {value}")),
        Some(citation.terminology.evidence_note.clone()),
    ]);
    let mut terminology = serde_json::json!({
        "id": TERMINOLOGY_ENTRY_ID,
        "type": "dataset",
        "title": citation.terminology.title,
    });
    insert_optional(
        &mut terminology,
        "publisher",
        citation.terminology.publisher.as_deref(),
    );
    insert_optional(
        &mut terminology,
        "URL",
        citation.terminology.access_url.as_deref(),
    );
    insert_optional(
        &mut terminology,
        "version",
        citation.terminology.release_id.as_deref(),
    );
    if let Some(date) = citation.terminology.release_date.as_deref() {
        let parsed = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .context("validated release date could not be rendered")?;
        terminology["issued"] = serde_json::json!({
            "date-parts": [[parsed.year(), parsed.month(), parsed.day()]],
        });
    }
    if !terminology_note.is_empty() {
        terminology["note"] = terminology_note.into();
    }

    let mut artefact_notes = Vec::new();
    if let Some(fingerprint) = &citation.artefact.content_fingerprint {
        artefact_notes.push(format!("Canonical concept fingerprint: {fingerprint}"));
    }
    artefact_notes.extend(citation.artefact.companions.iter().map(|companion| {
        format!(
            "Companion {} (schema {}, {} records) fingerprint: {}",
            companion.kind,
            companion.schema_version,
            companion.record_count,
            companion.content_fingerprint
        )
    }));
    if let Some(created_at) = &citation.artefact.created_at {
        artefact_notes.push(format!("Created: {created_at}"));
    }
    artefact_notes.extend(citation.notes.iter().cloned());
    artefact_notes.push(citation.data_availability.clone());
    let mut artefact = serde_json::json!({
        "id": ARTEFACT_ENTRY_ID,
        "type": "dataset",
        "title": citation.artefact.title,
        "note": artefact_notes.join("; "),
    });
    insert_optional(
        &mut artefact,
        "version",
        citation.artefact.content_fingerprint.as_deref(),
    );

    let software_note = match citation.software.reader_version.as_deref() {
        Some(reader) => format!(
            "Recorded producer version: {}; reader version: {reader}",
            citation
                .software
                .producer_version
                .as_deref()
                .unwrap_or("unavailable")
        ),
        None => format!(
            "Recorded producer version: {}; reader version unavailable",
            citation
                .software
                .producer_version
                .as_deref()
                .unwrap_or("unavailable")
        ),
    };
    let mut software = serde_json::json!({
        "id": SOFTWARE_ENTRY_ID,
        "type": "software",
        "title": citation.software.name,
        "author": [{
            "family": citation.software.author.family,
            "given": citation.software.author.given,
        }],
        "URL": citation.software.repository,
        "note": software_note,
    });
    insert_optional(
        &mut software,
        "version",
        citation.software.producer_version.as_deref(),
    );

    serde_json::to_string_pretty(&vec![terminology, artefact, software])
        .context("serialising CSL-JSON citation")
}

fn validate_provenance(provenance: &Provenance) -> Result<()> {
    let recorded_edition = known_edition(&provenance.edition_label);
    let classified_release = classify_known_edition(&provenance.release_id);
    if let (Some(recorded), Some(classified)) = (recorded_edition, classified_release) {
        ensure!(
            recorded == classified,
            "provenance edition_label {:?} contradicts release_id edition {:?}",
            provenance.edition_label,
            classified
        );
    }

    if let Some(date) = nonempty(&provenance.release_date) {
        NaiveDate::parse_from_str(&date, "%Y-%m-%d").with_context(|| {
            format!("invalid provenance release_date {date:?}; expected YYYY-MM-DD")
        })?;
        ensure!(
            date.len() == 10,
            "invalid provenance release_date {date:?}; expected YYYY-MM-DD"
        );
        if let Some(recorded_date) = extract_release_date(&provenance.release_id) {
            ensure!(
                recorded_date == date,
                "provenance release_date {date:?} contradicts release_id date {recorded_date:?}"
            );
        }
    }

    if let Some(fingerprint) = &provenance.content_fingerprint {
        validate_fingerprint(fingerprint, "content_fingerprint")?;
    }

    if let Some(created_at) = nonempty(&provenance.created_at) {
        DateTime::parse_from_rfc3339(&created_at).with_context(|| {
            format!(
                "invalid provenance created_at {:?}; expected RFC3339",
                provenance.created_at
            )
        })?;
    }

    let mut companion_kinds = HashSet::new();
    for companion in &provenance.companions {
        ensure!(
            !companion.kind.trim().is_empty(),
            "provenance companion kind must not be empty"
        );
        ensure!(
            companion_kinds.insert(companion.kind.as_str()),
            "duplicate provenance companion kind {:?}",
            companion.kind
        );
        validate_fingerprint(
            &companion.content_fingerprint,
            &format!("companion {:?} content_fingerprint", companion.kind),
        )?;
    }

    let has_usable_evidence = nonempty(&provenance.edition_label)
        .is_some_and(|edition| edition != "unknown")
        || nonempty(&provenance.release_date).is_some()
        || nonempty(&provenance.release_id).is_some()
        || provenance.content_fingerprint.is_some()
        || !provenance.companions.is_empty()
        || nonempty(&provenance.sct_version).is_some();
    if !has_usable_evidence {
        bail!(
            "database has no usable citation provenance; rebuild it from known RF2 inputs with `sct ndjson` then `sct sqlite`"
        );
    }
    Ok(())
}

fn validate_fingerprint(fingerprint: &str, field: &str) -> Result<()> {
    let Some(hex) = fingerprint.strip_prefix("sha256:") else {
        bail!("invalid {field}: expected sha256: followed by 64 lowercase hexadecimal characters");
    };
    ensure!(
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "invalid {field}: expected sha256: followed by 64 lowercase hexadecimal characters"
    );
    Ok(())
}

fn publisher_mapping(
    edition: Option<&str>,
) -> (Option<&'static str>, Option<&'static str>, &'static str) {
    match edition {
        Some("International") => (
            Some("SNOMED International"),
            Some("https://www.snomed.org/get-snomed"),
            "SNOMED International licensing and access terms; see https://www.snomed.org/get-snomed",
        ),
        Some("UK Clinical" | "UK Drug" | "UK Monolith" | "UK Edition") => (
            Some("NHS England"),
            Some("https://isd.digital.nhs.uk/trud/"),
            "NHS England TRUD access requirements and the applicable SNOMED CT licence; see https://isd.digital.nhs.uk/trud/",
        ),
        _ => (
            None,
            None,
            "the applicable SNOMED CT licence and source access terms; no publisher or access service is inferred from the recorded edition",
        ),
    }
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

fn known_edition(value: &str) -> Option<&str> {
    match value {
        "International" | "UK Clinical" | "UK Drug" | "UK Monolith" | "UK Edition" => Some(value),
        _ => None,
    }
}

fn classify_known_edition(release_id: &str) -> Option<&'static str> {
    match release_id.trim() {
        "International" => return Some("International"),
        "UK Clinical" => return Some("UK Clinical"),
        "UK Drug" => return Some("UK Drug"),
        "UK Monolith" => return Some("UK Monolith"),
        "UK Edition" => return Some("UK Edition"),
        _ => {}
    }
    let classified = crate::provenance::classify_edition(release_id);
    match classified.as_str() {
        "International" => Some("International"),
        "UK Clinical" => Some("UK Clinical"),
        "UK Drug" => Some("UK Drug"),
        "UK Monolith" => Some("UK Monolith"),
        "UK Edition" => Some("UK Edition"),
        _ => None,
    }
}

fn text_field(output: &mut String, label: &str, value: &str) {
    output.push_str("  ");
    output.push_str(label);
    output.push_str(": ");
    output.push_str(&single_line(value));
    output.push('\n');
}

fn text_optional(output: &mut String, label: &str, value: Option<&str>) {
    if let Some(value) = value {
        text_field(output, label, value);
    }
}

fn bibtex_entry(id: &str, fields: &[(&str, &str)]) -> String {
    let mut output = format!("@misc{{{id},\n");
    for (index, (name, value)) in fields.iter().enumerate() {
        output.push_str("  ");
        output.push_str(name);
        output.push_str(" = {");
        output.push_str(&bibtex_escape(value));
        output.push('}');
        if index + 1 < fields.len() {
            output.push(',');
        }
        output.push('\n');
    }
    output.push('}');
    output
}

fn bibtex_escape(value: &str) -> String {
    let mut output = String::new();
    for ch in single_line(value).chars() {
        match ch {
            '\\' => output.push_str("\\textbackslash{}"),
            '{' => output.push_str("\\{"),
            '}' => output.push_str("\\}"),
            '%' => output.push_str("\\%"),
            '#' => output.push_str("\\#"),
            '&' => output.push_str("\\&"),
            '_' => output.push_str("\\_"),
            '$' => output.push_str("\\$"),
            '~' => output.push_str("\\textasciitilde{}"),
            '^' => output.push_str("\\textasciicircum{}"),
            _ => output.push(ch),
        }
    }
    output
}

fn push_optional<'a>(
    fields: &mut Vec<(&'static str, &'a str)>,
    name: &'static str,
    value: Option<&'a str>,
) {
    if let Some(value) = value {
        fields.push((name, value));
    }
}

fn insert_optional(value: &mut serde_json::Value, field: &str, item: Option<&str>) {
    if let Some(item) = item {
        value[field] = item.into();
    }
}

fn join_nonempty<const N: usize>(values: [Option<String>; N]) -> String {
    values.into_iter().flatten().collect::<Vec<_>>().join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{COMPANION_HISTORY, COMPANION_PAYLOAD_REFSETS, NDJSON_TYPE_TAG};

    const CORE_FINGERPRINT: &str =
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const HISTORY_FINGERPRINT: &str =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111";

    fn provenance() -> Provenance {
        Provenance {
            type_tag: NDJSON_TYPE_TAG.to_string(),
            edition_label: "International".to_string(),
            release_date: "2026-03-01".to_string(),
            release_id: "SnomedCT_InternationalRF2_PRODUCTION_20260301T120000Z".to_string(),
            content_fingerprint: Some(CORE_FINGERPRINT.to_string()),
            companions: vec![CompanionArtifact {
                kind: COMPANION_HISTORY.to_string(),
                schema_version: 1,
                record_count: 17,
                content_fingerprint: HISTORY_FINGERPRINT.to_string(),
            }],
            source_paths: vec!["/private/rf2/release.zip".to_string()],
            sct_version: "0.27.0".to_string(),
            created_at: "2026-03-02T03:04:05+00:00".to_string(),
        }
    }

    #[test]
    fn maps_only_known_international_and_uk_publishers() {
        let international = Citation::from_provenance(&provenance()).unwrap();
        assert_eq!(
            international.terminology.publisher.as_deref(),
            Some("SNOMED International")
        );
        assert_eq!(
            international.terminology.access_url.as_deref(),
            Some("https://www.snomed.org/get-snomed")
        );

        for edition in ["UK Clinical", "UK Drug", "UK Monolith", "UK Edition"] {
            let mut p = provenance();
            p.edition_label = edition.to_string();
            p.release_id.clear();
            let citation = Citation::from_provenance(&p).unwrap();
            assert_eq!(
                citation.terminology.publisher.as_deref(),
                Some("NHS England")
            );
            assert_eq!(
                citation.terminology.access_url.as_deref(),
                Some("https://isd.digital.nhs.uk/trud/")
            );
        }

        let mut extension = provenance();
        extension.edition_label = "Acme International-compatible extension".to_string();
        let citation = Citation::from_provenance(&extension).unwrap();
        assert_eq!(citation.terminology.publisher, None);
        assert_eq!(citation.terminology.access_url, None);
    }

    #[test]
    fn terminology_evidence_note_is_present_and_scoped_to_the_source_entry() {
        let citation = Citation::from_provenance(&provenance()).unwrap();
        assert_eq!(
            citation.terminology.evidence_note,
            TERMINOLOGY_EVIDENCE_NOTE
        );

        let text = citation.render_text();
        assert_eq!(text.matches(TERMINOLOGY_EVIDENCE_NOTE).count(), 1);
        assert!(
            text.find(TERMINOLOGY_EVIDENCE_NOTE).unwrap()
                < text.find("Local derived artefact").unwrap()
        );

        let bibtex = citation.render_bibtex();
        assert_eq!(bibtex.matches(TERMINOLOGY_EVIDENCE_NOTE).count(), 1);
        let artefact_entry = bibtex
            .split(&format!("@misc{{{ARTEFACT_ENTRY_ID},"))
            .nth(1)
            .unwrap();
        assert!(!artefact_entry.contains(TERMINOLOGY_EVIDENCE_NOTE));

        let csl: serde_json::Value =
            serde_json::from_str(&citation.render_csl_json().unwrap()).unwrap();
        assert!(csl[0]["note"]
            .as_str()
            .unwrap()
            .contains(TERMINOLOGY_EVIDENCE_NOTE));
        assert!(!csl[1]["note"]
            .as_str()
            .unwrap()
            .contains(TERMINOLOGY_EVIDENCE_NOTE));
    }

    #[test]
    fn rejects_contradictory_recognised_editions_but_allows_custom_releases() {
        let mut contradictory = provenance();
        contradictory.edition_label = "UK Clinical".to_string();
        let error = Citation::from_provenance(&contradictory)
            .unwrap_err()
            .to_string();
        assert!(error.contains("contradicts release_id edition"), "{error}");

        let mut custom = provenance();
        custom.edition_label = "Local research extension".to_string();
        custom.release_id = "Acme_LocalExtension_PRODUCTION_20260301".to_string();
        assert!(Citation::from_provenance(&custom).is_ok());

        custom.edition_label = "unknown".to_string();
        custom.release_id = "SnomedCT_InternationalRF2_PRODUCTION_20260301T120000Z".to_string();
        assert!(Citation::from_provenance(&custom).is_ok());

        custom.edition_label = "UK Clinical".to_string();
        custom.release_id = "International".to_string();
        assert!(Citation::from_provenance(&custom).is_err());
    }

    #[test]
    fn preserves_producer_reader_and_companion_identity() {
        let citation =
            Citation::from_provenance_with_reader_version(&provenance(), "9.8.7").unwrap();
        assert_eq!(
            citation.software.producer_version.as_deref(),
            Some("0.27.0")
        );
        assert_eq!(citation.software.reader_version.as_deref(), Some("9.8.7"));
        assert_eq!(citation.software.author.family, "Baw");
        assert_eq!(citation.software.author.given, "Marcus");
        assert_eq!(
            citation.artefact.content_fingerprint.as_deref(),
            Some(CORE_FINGERPRINT)
        );
        assert_eq!(
            citation.artefact.companions[0].content_fingerprint,
            HISTORY_FINGERPRINT
        );
    }

    #[test]
    fn redacts_source_paths_from_model_and_every_output() {
        let secret = "/home/alice/secret-patient-project/release.zip";
        let mut p = provenance();
        p.source_paths = vec![secret.to_string(), "/another/private/path".to_string()];
        let citation = Citation::from_provenance(&p).unwrap();
        let outputs = [
            format!("{citation:?}"),
            serde_json::to_string(&citation).unwrap(),
            citation.render_text(),
            citation.render_bibtex(),
            citation.render_csl_json().unwrap(),
        ];
        for output in outputs {
            assert!(!output.contains(secret));
            assert!(!output.contains("/another/private/path"));
        }
        assert!(citation.artefact.composite);
        assert!(citation
            .notes
            .iter()
            .any(|note| note.contains("composite build")));
    }

    #[test]
    fn blank_source_paths_do_not_make_a_build_composite() {
        let mut p = provenance();
        p.source_paths = vec![
            "".to_string(),
            " \t".to_string(),
            "/one/release".to_string(),
        ];
        let citation = Citation::from_provenance(&p).unwrap();
        assert!(!citation.artefact.composite);
        assert!(!citation
            .notes
            .iter()
            .any(|note| note.contains("composite build")));

        p.source_paths.push("/two/release".to_string());
        assert!(Citation::from_provenance(&p).unwrap().artefact.composite);
    }

    #[test]
    fn whitespace_only_values_are_absent_and_other_values_remain_verbatim() {
        let mut p = provenance();
        p.release_date = " \t".to_string();
        p.release_id = "  custom release  ".to_string();
        p.sct_version = "  1.2.3  ".to_string();
        p.created_at = " \n".to_string();
        let citation = Citation::from_provenance(&p).unwrap();
        assert_eq!(citation.terminology.release_date, None);
        assert_eq!(
            citation.terminology.release_id.as_deref(),
            Some("  custom release  ")
        );
        assert_eq!(
            citation.software.producer_version.as_deref(),
            Some("  1.2.3  ")
        );
        assert_eq!(citation.artefact.created_at, None);
        let structured: serde_json::Value =
            serde_json::from_str(&citation.render_csl_json().unwrap()).unwrap();
        assert_eq!(structured[0]["version"], "  custom release  ");
        assert_eq!(structured[2]["version"], "  1.2.3  ");

        p.edition_label = " \t".to_string();
        p.release_id = " \n".to_string();
        p.sct_version = "  ".to_string();
        p.content_fingerprint = None;
        p.companions.clear();
        assert!(Citation::from_provenance(&p)
            .unwrap_err()
            .to_string()
            .contains("no usable citation provenance"));
    }

    #[test]
    fn partial_provenance_discloses_missing_evidence() {
        let mut p = provenance();
        p.release_date.clear();
        p.release_id.clear();
        p.content_fingerprint = None;
        p.sct_version.clear();
        p.created_at.clear();
        let citation = Citation::from_provenance(&p).unwrap();
        let notes = citation.notes.join(" ");
        assert!(notes.contains("release identifier is unavailable"));
        assert!(notes.contains("release date is unavailable"));
        assert!(notes.contains("does not exactly identify"));
        assert!(notes.contains("producer version is unavailable"));
        assert!(notes.contains("creation time is unavailable"));
        assert!(!citation.render_csl_json().unwrap().contains("date-parts"));
    }

    #[test]
    fn rejects_provenance_without_usable_evidence() {
        let mut p = provenance();
        p.edition_label = "unknown".to_string();
        p.release_date.clear();
        p.release_id.clear();
        p.content_fingerprint = None;
        p.companions.clear();
        p.sct_version.clear();
        p.created_at.clear();
        assert!(Citation::from_provenance(&p)
            .unwrap_err()
            .to_string()
            .contains("no usable citation provenance"));
    }

    #[test]
    fn validates_release_date_and_release_id_consistency() {
        let mut p = provenance();
        p.release_date = "2026-02-30".to_string();
        assert!(Citation::from_provenance(&p).is_err());

        p.release_date = "2026-03-02".to_string();
        let error = Citation::from_provenance(&p).unwrap_err().to_string();
        assert!(error.contains("contradicts release_id"), "{error}");
    }

    #[test]
    fn validates_core_and_companion_fingerprints() {
        for invalid in [
            "",
            "sha256:abc",
            "SHA256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeF",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg",
        ] {
            let mut p = provenance();
            p.content_fingerprint = Some(invalid.to_string());
            assert!(
                Citation::from_provenance(&p).is_err(),
                "accepted {invalid:?}"
            );

            let mut p = provenance();
            p.companions[0].content_fingerprint = invalid.to_string();
            assert!(
                Citation::from_provenance(&p).is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn rejects_empty_and_duplicate_companion_kinds() {
        let mut p = provenance();
        p.companions[0].kind = " \t".to_string();
        assert!(Citation::from_provenance(&p).is_err());

        let mut p = provenance();
        p.companions.push(CompanionArtifact {
            kind: COMPANION_HISTORY.to_string(),
            schema_version: 2,
            record_count: 0,
            content_fingerprint: HISTORY_FINGERPRINT.to_string(),
        });
        assert!(Citation::from_provenance(&p).is_err());
    }

    #[test]
    fn validates_nonempty_created_at_as_rfc3339() {
        let mut p = provenance();
        p.created_at.clear();
        assert!(Citation::from_provenance(&p).is_ok());

        p.created_at = "2026-03-02 03:04:05".to_string();
        let error = Citation::from_provenance(&p).unwrap_err().to_string();
        assert!(error.contains("RFC3339"), "{error}");
    }

    #[test]
    fn renders_stable_ids_and_exact_fingerprints_in_all_formats() {
        let citation = Citation::from_provenance(&provenance()).unwrap();
        let text = citation.render_text();
        let bibtex = citation.render_bibtex();
        let csl = citation.render_csl_json().unwrap();
        for output in [&text, &bibtex, &csl] {
            assert!(output.contains(CORE_FINGERPRINT));
            assert!(output.contains(HISTORY_FINGERPRINT));
        }
        for id in [TERMINOLOGY_ENTRY_ID, ARTEFACT_ENTRY_ID, SOFTWARE_ENTRY_ID] {
            assert!(bibtex.contains(&format!("@misc{{{id},")));
        }
        let value: serde_json::Value = serde_json::from_str(&csl).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 3);
        assert_eq!(value[0]["id"], TERMINOLOGY_ENTRY_ID);
        assert_eq!(value[1]["id"], ARTEFACT_ENTRY_ID);
        assert_eq!(value[2]["id"], SOFTWARE_ENTRY_ID);
        assert!(text.contains("Author: Marcus Baw"));
        assert!(bibtex.contains("author = {Baw, Marcus}"));
        assert_eq!(value[2]["author"][0]["family"], "Baw");
        assert_eq!(value[2]["author"][0]["given"], "Marcus");
        assert_eq!(
            value[0]["issued"]["date-parts"][0],
            serde_json::json!([2026, 3, 1])
        );
    }

    #[test]
    fn text_and_bibtex_neutralise_adversarial_metadata() {
        let mut p = provenance();
        p.edition_label = "Evil\n@misc{owned,\n title={x}}\u{1b}[31m%_&#$~^\\".to_string();
        p.release_id = "release-20260301\rforged".to_string();
        p.sct_version = "1.0}\n@misc{injected".to_string();
        let citation = Citation::from_provenance_with_reader_version(&p, "2.0\nforged").unwrap();
        let text = citation.render_text();
        assert!(!text.contains(['\r', '\u{1b}']));
        assert!(!text.lines().any(|line| line.starts_with("@misc")));

        let bibtex = citation.render_bibtex();
        assert_eq!(bibtex.matches("@misc{").count(), 3);
        assert!(!bibtex.contains("\n@misc{owned"));
        assert!(bibtex.contains("\\%\\_\\&\\#\\$\\textasciitilde{}\\textasciicircum{}"));

        let csl = citation.render_csl_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&csl).unwrap();
        assert_eq!(
            value[0]["title"],
            format!("SNOMED CT {} Edition", p.edition_label)
        );
    }

    #[test]
    fn all_companion_kinds_remain_distinct() {
        let mut p = provenance();
        p.companions.push(CompanionArtifact {
            kind: COMPANION_PAYLOAD_REFSETS.to_string(),
            schema_version: 3,
            record_count: 99,
            content_fingerprint:
                "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                    .to_string(),
        });
        let citation = Citation::from_provenance(&p).unwrap();
        assert_eq!(citation.artefact.companions.len(), 2);
        assert_eq!(
            citation.artefact.companions[1].kind,
            COMPANION_PAYLOAD_REFSETS
        );
    }
}
