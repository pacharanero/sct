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
    pub fn timeout(d: impl Into<String>) -> Self {
        Self {
            status: 408,
            code: "timeout",
            diagnostics: d.into(),
        }
    }
    /// FHIR's own vocabulary for "this would be too expensive to compute"
    /// (`OperationOutcome.issue.code` `too-costly`), distinct from `timeout`:
    /// the server refused up front rather than starting and running out of
    /// time. Used when a compound ECL/filter expansion would materialise more
    /// results than the server keeps in memory at once (see `EvalLimits` in
    /// `crate::ecl::eval`, roadmap `R53`).
    pub fn too_costly(d: impl Into<String>) -> Self {
        Self {
            status: 403,
            code: "too-costly",
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

/// A `ValueSet.expansion.contains.designation` object (FSN or synonym).
///
/// This deliberately differs from [`designation`], which wraps the same data
/// in a `Parameters.parameter` entry for `CodeSystem/$lookup`.
pub fn expansion_designation(type_id: &str, type_label: &str, term: &str) -> Value {
    json!({
        "use": { "system": SNOMED_SYSTEM, "code": type_id, "display": type_label },
        "value": term,
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

/// The definition (`compose` plus identifying metadata) of the *implicit*
/// SNOMED CT value set an `$expand` request named, for `includeDefinition=true`.
/// Follows the templates in the R4 SNOMED CT code-system page: an ECL-defined
/// set is a `constraint`/`=` filter, and the bare `?fhir_vs` form is the whole
/// code system with no filter.
///
/// `version` is deliberately omitted. SNOMED's URI specification requires a
/// version to be the full `http://snomed.info/sct/[sctid]/version/[YYYYMMDD]`
/// form, and explicitly says a bare release date is not safe to publish as
/// one. `sct` records the release date but not the edition's module SCTID, so
/// it cannot construct a conformant version URI - and emitting a
/// non-conformant one would be worse than omitting it. The loaded release is
/// still discoverable via `$lookup` and `/metadata`.
pub fn implicit_valueset_definition(ecl: Option<&str>) -> Value {
    const COPYRIGHT: &str = "This value set includes content from SNOMED CT, which is copyright \u{a9} 2002+ International Health Terminology Standards Development Organisation (SNOMED International), and distributed by agreement between SNOMED International and HL7. Implementer use of SNOMED CT is not covered by this agreement";
    match ecl {
        Some(ecl) => json!({
            "url": format!("{SNOMED_SYSTEM}?fhir_vs=ecl/{ecl}"),
            "name": format!("SNOMED CT Concepts matching {ecl}"),
            "description": format!("All SNOMED CT concepts that match the expression constraint {ecl}"),
            "copyright": COPYRIGHT,
            "compose": {
                "include": [{
                    "system": SNOMED_SYSTEM,
                    "filter": [{ "property": "constraint", "op": "=", "value": ecl }],
                }],
            },
        }),
        None => json!({
            "url": format!("{SNOMED_SYSTEM}?fhir_vs"),
            "name": "SNOMED CT Concepts",
            "description": "All SNOMED CT concepts",
            "copyright": COPYRIGHT,
            "compose": { "include": [{ "system": SNOMED_SYSTEM }] },
        }),
    }
}

/// Merge a value set's definition into an expansion resource, in place. Keys
/// already present on the expansion win, so the expansion's own `status` and
/// `resourceType` are never overwritten by the definition's.
pub fn attach_definition(expansion: &mut Value, definition: Value) {
    let (Some(target), Some(source)) = (expansion.as_object_mut(), definition.as_object()) else {
        return;
    };
    for (key, value) in source {
        if key == "resourceType" || key == "expansion" {
            continue;
        }
        target.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

/// A FHIR `ValueSet` with an `expansion`. `contains` entries are pre-built.
/// `display_language`, when `Some`, is the resolved language actually used
/// for `display`/designation strings (see
/// [`resolve_display_language`](super::ops::resolve_display_language)) and is
/// echoed back as an `expansion.parameter` entry per the `$expand` operation
/// definition, so a client can tell whether its requested `displayLanguage`
/// was honoured or fell back to `sct`'s single loaded locale.
pub fn value_set_expansion(
    total: usize,
    offset: usize,
    count: usize,
    contains: Vec<Value>,
    display_language: Option<&str>,
) -> Value {
    let mut parameter = vec![json!({ "name": "count", "valueInteger": count })];
    if let Some(lang) = display_language {
        parameter.push(json!({ "name": "displayLanguage", "valueCode": lang }));
    }
    json!({
        "resourceType": "ValueSet",
        "status": "active",
        "expansion": {
            "total": total,
            "offset": offset,
            "parameter": parameter,
            "contains": contains,
        },
    })
}

/// The stable id of the single `CodeSystem` resource this server serves
/// (`GET /CodeSystem/{id}`).
pub const CODE_SYSTEM_ID: &str = "sct";

/// The `CodeSystem` resource describing the loaded SNOMED CT release.
///
/// `content` is always `"not-present"`: unlike a small local code system,
/// `sct` never embeds the concept list in this resource - the concepts
/// themselves are reached through `CodeSystem/$lookup`, `$validate-code`,
/// `$subsumes`, and `ValueSet/$expand`. `version`, when the database records
/// a release date or id (see [`super::ops::release_version`]), is that value.
/// This is safe to publish, unlike the versioned *ValueSet* URI discussed in
/// [`implicit_valueset_definition`]'s doc comment: `CodeSystem.version` is a
/// plain business-version string, not a URI construction SNOMED's own
/// specification constrains.
pub fn code_system(version: Option<&str>, count: i64) -> Value {
    let mut cs = json!({
        "resourceType": "CodeSystem",
        "id": CODE_SYSTEM_ID,
        "url": SNOMED_SYSTEM,
        "name": "SNOMEDCT",
        "title": "SNOMED CT",
        "status": "active",
        "content": "not-present",
        "count": count,
    });
    if let Some(v) = version {
        cs["version"] = json!(v);
    }
    cs
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
