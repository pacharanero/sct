// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! #133: an empty enumerated ValueSet must not mean the whole SNOMED system.

use sct_rs::{
    commands::{ndjson, sqlite},
    sdk,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const SYSTEM: &str = "http://snomed.info/sct";
const MEMBERS: &str = "22298006 Myocardial infarction\n46635009 Type 1 diabetes mellitus\n";
const CASES: &[(&str, &str)] = &[
    ("control", MEMBERS),
    ("plain", ""),
    ("exclusions-only", "# 22298006 Myocardial infarction\n"),
    (
        "composed",
        "# 22298006 Myocardial infarction\n# 46635009 Type 1 diabetes mellitus\n",
    ),
    ("pending-only", "# ? 22298006 Myocardial infarction\n"),
];

fn fixture() -> (tempfile::TempDir, PathBuf, BTreeSet<String>) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("fixture.ndjson");
    let db = dir.path().join("fixture.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")],
        locale: "en-GB".into(),
        output: Some(input.clone()),
        include_inactive: false,
        refsets: ndjson::RefsetMode::Simple,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input,
        output: Some(db.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    let snomed = sdk::Snomed::open(&db).unwrap();
    for line in MEMBERS.lines() {
        let (id, expected) = line.split_once(' ').unwrap();
        assert_eq!(
            snomed.concept(id).unwrap().unwrap().preferred_term,
            expected
        );
    }
    let universe = snomed.expand("*").unwrap().into_iter().collect();
    for (id, body) in CASES {
        let includes = if *id == "composed" {
            "includes: ['./control.codelist']\n"
        } else {
            ""
        };
        let list = sdk::parse_codelist(&format!(
            "---\nid: {id}\ntitle: Synthetic {id}\ndescription: Empty ValueSet regression\nterminology: SNOMED CT\ncreated: '2026-01-01'\nupdated: '2026-01-01'\nversion: 1\nstatus: draft\nlicence: CC-BY-4.0\ncopyright: Synthetic test\nappropriate_use: Testing\nmisuse: None\n{includes}---\n{body}"
        )).unwrap();
        sdk::write_codelist(&list, dir.path().join(format!("{id}.codelist"))).unwrap();
    }
    (dir, db, universe)
}

fn cli(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sct"));
    command
        .current_dir(dir)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("SCT_DATA_HOME", dir.join("data"))
        .env_remove("SCT_DB")
        .env_remove("SCT_CODELISTS")
        .arg("codelist");
    command
}

fn assert_definition(resource: &Value, universe: &BTreeSet<String>, empty: bool) {
    assert_eq!(resource["resourceType"], "ValueSet");
    let compose = &resource["compose"];
    if !empty {
        assert_eq!(
            compose,
            &json!({"include": [{"system": SYSTEM, "concept": [
                {"code": "22298006", "display": "Myocardial infarction"},
                {"code": "46635009", "display": "Type 1 diabetes mellitus"}
            ]}]})
        );
        return;
    }
    // Independent FHIR set semantics: a system-only group denotes its universe.
    // Reject concept: [], filters, version restrictions, and mismatched scopes.
    let scope = |side: &str| {
        let groups = compose[side].as_array().expect("nonempty compose groups");
        assert!(!groups.is_empty(), "{resource}");
        assert_eq!(groups.len(), 1, "canonical form: {resource}");
        let mut selected = BTreeSet::new();
        for group in groups {
            assert_eq!(group, &json!({"system": SYSTEM}), "{resource}");
            selected.extend(universe.iter().cloned());
        }
        selected
    };
    let included = scope("include");
    let excluded = scope("exclude");
    assert!(!included.is_empty());
    assert_eq!(included, excluded);
    assert_eq!(included.difference(&excluded).count(), 0);
    assert_eq!(compose.as_object().unwrap().len(), 2);
}

fn import_and_check(dir: &Path, source: &Path, id: &str, empty: bool) {
    let destination = dir.join(format!("imported-{id}.codelist"));
    let output = cli(dir)
        .arg("import")
        .arg(&destination)
        .args(["--from", "fhir-json"])
        .arg(source)
        .output()
        .unwrap();
    assert!(output.status.success(), "{id}: {output:?}");
    let imported = sdk::read_codelist(&destination).unwrap();
    let members = sdk::effective_members_of(&imported, &destination, dir).unwrap();
    let mut actual: Vec<_> = members.iter().map(|m| m.id.as_str()).collect();
    actual.sort_unstable();
    let expected = if empty {
        vec![]
    } else {
        vec!["22298006", "46635009"]
    };
    assert_eq!(actual, expected, "{id}");
    assert_eq!(imported.front_matter.title, format!("Synthetic {id}"));
    assert_eq!(
        imported.front_matter.description,
        "Empty ValueSet regression"
    );
    assert_eq!(imported.front_matter.copyright, "Synthetic test");
    assert_eq!(imported.front_matter.status, "draft");
}

#[test]
fn cli_export_import_preserves_empty_membership_and_metadata() {
    let (dir, _db, universe) = fixture();
    for (id, _) in CASES {
        let file = dir.path().join(format!("{id}.codelist"));
        let list = sdk::read_codelist(&file).unwrap();
        let empty = *id != "control";
        let members = sdk::effective_members_of(&list, &file, dir.path()).unwrap();
        assert_eq!(members.len(), if empty { 0 } else { 2 });
        let output = cli(dir.path())
            .arg("export")
            .arg(file)
            .args(["--format", "fhir-json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{id}: {output:?}");
        let resource: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_definition(&resource, &universe, empty);
        let source = dir.path().join("export.json");
        fs::write(&source, output.stdout).unwrap();
        import_and_check(dir.path(), &source, id, empty);
    }
}

#[test]
fn canonical_empty_import_is_accepted_independently_of_exporter() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("empty.json");
    fs::write(
        &source,
        json!({
            "resourceType": "ValueSet", "title": "Synthetic plain", "status": "draft",
            "description": "Empty ValueSet regression", "copyright": "Synthetic test",
            "compose": {"include": [{"system": SYSTEM}], "exclude": [{"system": SYSTEM}]}
        })
        .to_string(),
    )
    .unwrap();
    import_and_check(dir.path(), &source, "plain", true);
}

#[test]
fn ambiguous_empty_concept_arrays_are_rejected_before_creating_a_list() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("ambiguous.json");
    let destination = dir.path().join("must-not-exist.codelist");
    for side in ["include", "exclude"] {
        let mut compose =
            json!({"include": [{"system": SYSTEM, "concept": [{"code": "22298006"}]}]});
        compose[side] = json!([{"system": SYSTEM, "concept": []}]);
        fs::write(
            &source,
            json!({"resourceType": "ValueSet", "compose": compose}).to_string(),
        )
        .unwrap();
        let output = cli(dir.path())
            .arg("import")
            .arg(&destination)
            .args(["--from", "fhir-json"])
            .arg(&source)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{side}");
        assert!(!destination.exists());
        assert!(String::from_utf8_lossy(&output.stderr).contains("entire code system"));
    }
}

#[cfg(feature = "serve")]
#[test]
fn stored_and_http_empty_definitions_agree_with_expansions() {
    use sct_rs::commands::serve::{serve_listener, valuesets};
    let (dir, db, universe) = fixture();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let registry = valuesets::load_registry(dir.path(), &base);
    assert_eq!(registry.len(), CASES.len());
    let codelists = dir.path().to_path_buf();
    std::thread::spawn(move || {
        serve_listener(db, "/", Some(codelists), None, 2, listener).unwrap()
    });
    let get = |url: &str| -> Value {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(2)))
            .build()
            .new_agent();
        for _ in 0..50 {
            if let Ok(response) = agent.get(url).call() {
                return serde_json::from_str(&response.into_body().read_to_string().unwrap())
                    .unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        panic!("server did not respond successfully: {url}");
    };
    for (id, _) in CASES {
        let empty = *id != "control";
        let stored = registry.get(id).unwrap();
        assert_eq!(stored.members.len(), if empty { 0 } else { 2 });
        let summary = stored.summary_resource();
        assert!(summary.get("compose").is_none());
        assert!(summary.get("expansion").is_none());
        assert_eq!(summary["title"], format!("Synthetic {id}"));
        let definition = stored.to_resource();
        assert_definition(&definition, &universe, empty);
        let read = get(&format!("{base}/ValueSet/{id}"));
        assert_definition(&read, &universe, empty);
        assert_eq!(read, definition);
        for route in [
            format!("ValueSet/{id}/$expand?includeDefinition=true"),
            format!("ValueSet/$expand?url={base}/ValueSet/{id}&includeDefinition=true"),
        ] {
            let expanded = get(&format!("{base}/{route}"));
            assert_definition(&expanded, &universe, empty);
            assert_eq!(expanded["compose"], definition["compose"]);
            assert_eq!(expanded["expansion"]["total"], if empty { 0 } else { 2 });
            let mut codes: Vec<_> = expanded["expansion"]
                .get("contains")
                .map(|v| {
                    v.as_array()
                        .unwrap()
                        .iter()
                        .map(|c| c["code"].as_str().unwrap())
                        .collect()
                })
                .unwrap_or_default();
            codes.sort_unstable();
            let expected = if empty {
                vec![]
            } else {
                vec!["22298006", "46635009"]
            };
            assert_eq!(codes, expected);
        }
    }
}
