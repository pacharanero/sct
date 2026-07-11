// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FHIR R4 response builders and the error type, hand-rolled with `serde_json`
//! (no FHIR model crate). See `spec/commands/serve.md`.

use serde_json::{json, Value};

/// SNOMED CT code system URI.
pub const SNOMED_SYSTEM: &str = "http://snomed.info/sct";

/// Map a FHIR code-system URI (or a bare `sct` internal name) to the internal
/// terminology key used by the crossmap engine. `None` for unsupported systems.
pub fn system_to_internal(system: &str) -> Option<&'static str> {
    match system {
        SNOMED_SYSTEM | "snomed" => Some("snomed"),
        "http://hl7.org/fhir/sid/icd-10" | "http://hl7.org/fhir/sid/icd-10-uk" | "icd10" => {
            Some("icd10")
        }
        "https://fhir.hl7.org.uk/Id/opcs-4" | "http://hl7.org/fhir/sid/ex-opcs4" | "opcs4" => {
            Some("opcs4")
        }
        "http://read.info/ctv3" | "ctv3" => Some("ctv3"),
        "http://read.info/readv2" | "read2" => Some("read2"),
        _ => None,
    }
}

/// The canonical FHIR code-system URI for an internal terminology key.
pub fn internal_to_system(internal: &str) -> &str {
    match internal {
        "snomed" => SNOMED_SYSTEM,
        "icd10" => "http://hl7.org/fhir/sid/icd-10",
        "opcs4" => "https://fhir.hl7.org.uk/Id/opcs-4",
        "ctv3" => "http://read.info/ctv3",
        "read2" => "http://read.info/readv2",
        other => other,
    }
}

/// An error that maps to an HTTP status plus a FHIR `OperationOutcome`.
#[derive(Debug)]
pub struct FhirError {
    pub status: u16,
    pub code: &'static str,
    pub diagnostics: String,
}

impl FhirError {
    pub fn not_found(d: impl Into<String>) -> Self {
        Self {
            status: 404,
            code: "not-found",
            diagnostics: d.into(),
        }
    }
    pub fn invalid(d: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: "invalid",
            diagnostics: d.into(),
        }
    }
    pub fn exception(d: impl Into<String>) -> Self {
        Self {
            status: 500,
            code: "exception",
            diagnostics: d.into(),
        }
    }
    /// The `OperationOutcome` body for this error.
    pub fn outcome(&self) -> Value {
        operation_outcome("error", self.code, &self.diagnostics)
    }
}

/// A FHIR `OperationOutcome` with a single issue.
pub fn operation_outcome(severity: &str, code: &str, diagnostics: &str) -> Value {
    json!({
        "resourceType": "OperationOutcome",
        "issue": [{ "severity": severity, "code": code, "diagnostics": diagnostics }],
    })
}

/// Wrap a list of `parameter` entries in a FHIR `Parameters` resource.
pub fn parameters(parameter: Vec<Value>) -> Value {
    json!({ "resourceType": "Parameters", "parameter": parameter })
}

/// A `$lookup` `property` entry whose value is a coded concept (parent / child /
/// ancestor), with a human-readable description part.
pub fn property_concept(code: &str, sctid: &str, display: &str) -> Value {
    json!({
        "name": "property",
        "part": [
            { "name": "code", "valueCode": code },
            { "name": "value", "valueCode": sctid },
            { "name": "description", "valueString": display },
        ],
    })
}

/// A `$lookup` `designation` entry (FSN or synonym).
pub fn designation(type_id: &str, type_label: &str, term: &str) -> Value {
    json!({
        "name": "designation",
        "part": [
            { "name": "use", "valueCoding": { "system": SNOMED_SYSTEM, "code": type_id, "display": type_label } },
            { "name": "value", "valueString": term },
        ],
    })
}

/// The `/metadata` CapabilityStatement.
pub fn capability_statement(
    software_version: &str,
    impl_url: &str,
    translate_available: bool,
) -> Value {
    let mut resources = vec![
        json!({
            "type": "CodeSystem",
            "operation": [
                { "name": "lookup", "definition": "http://hl7.org/fhir/OperationDefinition/CodeSystem-lookup" },
                { "name": "validate-code", "definition": "http://hl7.org/fhir/OperationDefinition/CodeSystem-validate-code" },
                { "name": "subsumes", "definition": "http://hl7.org/fhir/OperationDefinition/CodeSystem-subsumes" },
            ],
        }),
        json!({
            "type": "ValueSet",
            "operation": [
                { "name": "expand", "definition": "http://hl7.org/fhir/OperationDefinition/ValueSet-expand" },
                { "name": "validate-code", "definition": "http://hl7.org/fhir/OperationDefinition/ValueSet-validate-code" },
            ],
        }),
    ];
    if translate_available {
        resources.push(json!({
            "type": "ConceptMap",
            "operation": [
                { "name": "translate", "definition": "http://hl7.org/fhir/OperationDefinition/ConceptMap-translate" },
            ],
        }));
    }

    json!({
        "resourceType": "CapabilityStatement",
        "status": "active",
        "fhirVersion": "4.0.1",
        "kind": "instance",
        "format": ["application/fhir+json", "json"],
        "software": { "name": "sct", "version": software_version },
        "implementation": {
            "description": "SNOMED CT FHIR R4 terminology server backed by SQLite",
            "url": impl_url,
        },
        "rest": [{
            "mode": "server",
            "interaction": [{ "code": "batch" }],
            "resource": resources,
        }],
    })
}

/// The `/metadata?mode=terminology` TerminologyCapabilities statement - the
/// terminology-specific counterpart to the CapabilityStatement, advertising the
/// code systems served and the expansion / validation / translation features.
pub fn terminology_capabilities(
    software_version: &str,
    impl_url: &str,
    translate_available: bool,
) -> Value {
    let mut tc = json!({
        "resourceType": "TerminologyCapabilities",
        "status": "active",
        // `date` is required in R4; use the day the statement is served.
        "date": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "kind": "instance",
        "software": { "name": "sct", "version": software_version },
        "implementation": {
            "description": "SNOMED CT FHIR R4 terminology server backed by SQLite",
            "url": impl_url,
        },
        "codeSystem": [{ "uri": SNOMED_SYSTEM }],
        "expansion": { "hierarchical": false, "paging": true },
        "codeSearch": "all",
        "validateCode": { "translations": translate_available },
    });
    if translate_available {
        tc["translation"] = json!({ "needsMap": true });
    }
    tc
}

/// A FHIR `ValueSet` with an `expansion`. `contains` entries are pre-built.
pub fn value_set_expansion(
    total: usize,
    offset: usize,
    count: usize,
    contains: Vec<Value>,
) -> Value {
    json!({
        "resourceType": "ValueSet",
        "status": "active",
        "expansion": {
            "total": total,
            "offset": offset,
            "parameter": [{ "name": "count", "valueInteger": count }],
            "contains": contains,
        },
    })
}

/// A FHIR `Bundle` of type `searchset` wrapping pre-built resources.
pub fn bundle_searchset(resources: Vec<Value>) -> Value {
    let entry: Vec<Value> = resources
        .into_iter()
        .map(|r| json!({ "resource": r }))
        .collect();
    json!({
        "resourceType": "Bundle",
        "type": "searchset",
        "total": entry.len(),
        "entry": entry,
    })
}

/// A `batch-response` Bundle: one entry per request entry, in order, each with a
/// `response.status` and the operation's result `resource` (or an
/// OperationOutcome). Pre-built entries are passed in.
pub fn bundle_batch_response(entries: Vec<Value>) -> Value {
    json!({
        "resourceType": "Bundle",
        "type": "batch-response",
        "entry": entries,
    })
}
