// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! `sct transcode` over the synthetic fixture built with `--refsets all`
//! (so ICD-10/OPCS-4 crossmaps + concept history are present).

use rusqlite::Connection;
use sct_rs::commands::crosswalk::equivalents;
use sct_rs::commands::ndjson::{self, RefsetMode};
use sct_rs::commands::read2;
use sct_rs::commands::sqlite;
use sct_rs::commands::transcode::transcode_one;
use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn build() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let ndjson = dir.path().join("syn.ndjson");
    let db = dir.path().join("syn.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![fixture_dir()],
        locale: "en-GB".into(),
        output: Some(ndjson.clone()),
        include_inactive: false,
        refsets: RefsetMode::All,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input: ndjson,
        output: Some(db.clone()),
        transitive_closure: false,
        include_self: false,
    })
    .unwrap();
    (dir, Connection::open(&db).unwrap())
}

fn targets(c: &Connection, from: &str, code: &str, to: &str, fwd: bool) -> Vec<String> {
    let mut v: Vec<String> = transcode_one(c, from, code, to, fwd)
        .unwrap()
        .into_iter()
        .map(|m| m.target)
        .collect();
    v.sort();
    v
}

fn item9_zip() -> (tempfile::TempDir, PathBuf) {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("nhs_datamigration_29.0.0_20200401000001.zip");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file(
        "Mapping Tables/Updated/Clinically Assured/rcsctmap2_uk_20200401000001.txt",
        opts,
    )
    .unwrap();
    write!(
        zip,
        "MapId\tReadCode\tTermCode\tConceptId\tDescriptionId\tIS_ASSURED\tEffectiveDate\tMapStatus\r\n\
         rm1\t0111.\t00\t22298006\t1001\t1\t20200401\t1\r\n\
         rm2\tH33..\t11\t195967001\t1002\t0\t20200401\t1\r\n"
    )
    .unwrap();
    zip.finish().unwrap();
    (dir, path)
}

#[test]
fn snomed_to_icd10_and_reverse() {
    let (_d, c) = build();
    assert_eq!(targets(&c, "snomed", "22298006", "icd10", false), ["I219"]);
    // Reverse: ICD-10 -> SNOMED.
    assert_eq!(targets(&c, "icd10", "I219", "snomed", false), ["22298006"]);
}

#[test]
fn snomed_to_opcs4() {
    let (_d, c) = build();
    assert_eq!(targets(&c, "snomed", "80146002", "opcs4", false), ["H011"]);
}

#[test]
fn ctv3_to_icd10_two_hop() {
    let (_d, c) = build();
    // CTV3 X200 -> SNOMED 22298006 -> ICD-10 I219.
    assert_eq!(targets(&c, "ctv3", "X200", "icd10", false), ["I219"]);
}

#[test]
fn read2_item9_import_feeds_transcode() {
    let (_d, mut c) = build();
    let (_zdir, archive) = item9_zip();
    read2::import_archive_conn(&mut c, &archive).unwrap();

    // Read v2 -> SNOMED -> ICD-10 uses the imported item 9 map plus RF2
    // ExtendedMap rows from the SNOMED pipeline.
    assert_eq!(targets(&c, "read2", "0111.00", "icd10", false), ["I219"]);

    // Reverse through SNOMED also exposes the Read v2 source key.
    assert_eq!(targets(&c, "icd10", "I219", "read2", false), ["0111.00"]);
}

#[test]
fn history_forwarding_of_inactive_pivot() {
    let (_d, c) = build();
    // 9468002 is inactive; without forwarding it maps to nothing useful.
    assert!(targets(&c, "snomed", "9468002", "snomed", false) == ["9468002"]);
    // With forwarding it resolves to its same_as / replaced_by targets.
    assert_eq!(
        targets(&c, "snomed", "9468002", "snomed", true),
        ["195967001", "22298006"]
    );
}

#[test]
fn unmapped_code_yields_nothing() {
    let (_d, c) = build();
    assert!(targets(&c, "icd10", "Z999", "snomed", false).is_empty());
}

#[test]
fn codelist_include_maps_spans_concept_maps_and_crossmaps() {
    use sct_rs::commands::codelist::lookup_crosswalks;
    let (_d, c) = build();
    let maps = lookup_crosswalks(
        &c,
        &["22298006", "73211009"],
        &["icd10".to_string(), "ctv3".to_string()],
    )
    .unwrap();
    // ICD-10 and CTV3 both come through the general crossmaps model.
    assert_eq!(maps.codes_for("22298006", "icd10"), "I219");
    assert_eq!(maps.codes_for("73211009", "icd10"), "E149");
    assert_eq!(maps.codes_for("22298006", "ctv3"), "X200");
}

#[test]
fn crosswalk_shows_all_equivalents() {
    let (_d, c) = build();
    // From a SNOMED concept: its CTV3 + ICD-10 equivalents, all at once.
    let cw = equivalents(&c, "snomed", "22298006").unwrap();
    assert_eq!(cw.snomed, "22298006");
    assert_eq!(cw.display, "Myocardial infarction");
    let by: std::collections::HashMap<_, _> = cw.equivalents.iter().cloned().collect();
    assert_eq!(by["ctv3"], vec!["X200".to_string()]);
    assert_eq!(by["icd10"], vec!["I219".to_string()]);
    assert!(by["opcs4"].is_empty());

    // From a legacy CTV3 code: resolves to SNOMED and shows ICD-10.
    let cw = equivalents(&c, "ctv3", "X200").unwrap();
    assert_eq!(cw.snomed, "22298006");
    let by: std::collections::HashMap<_, _> = cw.equivalents.iter().cloned().collect();
    assert_eq!(by["icd10"], vec!["I219".to_string()]);
    assert!(by.contains_key("snomed")); // snomed is included when from != snomed
}

const CLASSIFICATION_CASES: [(&str, &str, &str, &str); 2] = [
    ("22298006", "icd10", "I219", "Myocardial infarction"),
    ("80146002", "opcs4", "H011", "Appendicectomy"),
];

fn replace_claims(c: &Connection, pivot: &str, system: &str, code: &str, claims: &[Option<&str>]) {
    c.execute(
        "DELETE FROM crossmaps WHERE source_code = ?1 AND target_system = ?2",
        rusqlite::params![pivot, system],
    )
    .unwrap();
    let refset = if system == "icd10" {
        "447562003"
    } else {
        "1126441000000105"
    };
    for correlation in claims {
        c.execute(
            "INSERT INTO crossmaps
             (source_system, source_code, target_system, target_code, map_refset, correlation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["snomed", pivot, system, code, refset, correlation],
        )
        .unwrap();
    }
}

#[test]
fn raw_claims_preserve_correlations_and_sort_independently_of_insertion() {
    let (_d, c) = build();
    for (pivot, system, code, display) in CLASSIFICATION_CASES {
        let mut claims = vec![
            None,
            Some(""),
            Some("447557004"),
            Some("447559001"),
            Some("999999999"),
        ];
        let expected: Vec<_> = claims
            .iter()
            .map(|correlation| {
                (
                    pivot.to_owned(),
                    code.to_owned(),
                    correlation.map(str::to_owned),
                    Some(display.to_owned()),
                )
            })
            .collect();
        // Repeat every raw row, then reinsert in the opposite order.
        claims.extend(claims.clone());
        for _ in 0..2 {
            replace_claims(&c, pivot, system, code, &claims);
            let actual: Vec<_> = transcode_one(&c, "snomed", pivot, system, false)
                .unwrap()
                .into_iter()
                .map(|m| (m.snomed, m.target, m.correlation, m.display))
                .collect();
            assert_eq!(actual, expected, "{system}: {claims:?}");
            claims.reverse();
        }
        // The target is also part of identity, even when the correlation matches.
        let second_code = if system == "icd10" { "I210" } else { "H010" };
        c.execute(
            "INSERT INTO crossmaps (source_system, source_code, target_system, target_code, map_refset, correlation)
             SELECT source_system, source_code, target_system, ?3, map_refset, ?4
             FROM crossmaps WHERE source_code = ?1 AND target_system = ?2 LIMIT 1",
            rusqlite::params![pivot, system, second_code, "447557004"],
        ).unwrap();
        let mut expected = expected;
        expected.insert(
            0,
            (
                pivot.into(),
                second_code.into(),
                Some("447557004".into()),
                Some(display.into()),
            ),
        );
        let actual: Vec<_> = transcode_one(&c, "snomed", pivot, system, false)
            .unwrap()
            .into_iter()
            .map(|m| (m.snomed, m.target, m.correlation, m.display))
            .collect();
        assert_eq!(actual, expected);
    }
}

#[cfg(feature = "serve")]
#[test]
fn translate_keeps_competing_equivalences_in_order() {
    let (_d, c) = build();
    for (pivot, system, code, _) in CLASSIFICATION_CASES {
        for claims in [
            [Some("447559001"), Some("447557004"), Some("447559001")],
            [Some("447557004"), Some("447559001"), Some("447557004")],
        ] {
            replace_claims(&c, pivot, system, code, &claims);
            let uri = if system == "icd10" {
                "http://hl7.org/fhir/sid/icd-10"
            } else {
                "https://fhir.hl7.org.uk/Id/opcs-4"
            };
            let result =
                sct_rs::commands::serve::ops::translate(&c, "http://snomed.info/sct", pivot, uri)
                    .unwrap();
            let matches: Vec<_> = result["parameter"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| p["name"] == "match")
                .cloned()
                .collect();
            let expected: Vec<_> = ["equivalent", "narrower"]
                .into_iter()
                .map(|equivalence| {
                    serde_json::json!({"name": "match", "part": [
                        {"name": "equivalence", "valueCode": equivalence},
                        {"name": "concept", "valueCoding": {"system": uri, "code": code}}
                    ]})
                })
                .collect();
            assert_eq!(matches, expected, "{system}");
        }
    }
}

#[test]
fn code_only_projections_collapse_claims_without_changing_fields() {
    use sct_rs::sdk::{Snomed, Terminology};
    let (d, c) = build();
    for (pivot, system, code, display) in CLASSIFICATION_CASES {
        replace_claims(
            &c,
            pivot,
            system,
            code,
            &[
                Some("447559001"),
                None,
                Some(""),
                Some("999999999"),
                Some("447557004"),
                Some("447557004"),
            ],
        );
        let sdk = Snomed::open(d.path().join("syn.db")).unwrap();
        let expected = serde_json::json!([
            {"target": code, "snomed": pivot, "display": display}
        ]);
        for rows in [
            sdk.map(Terminology::Snomed, pivot, system.parse().unwrap())
                .unwrap(),
            sdk.map_forwarding_history(Terminology::Snomed, pivot, system.parse().unwrap())
                .unwrap(),
        ] {
            assert_eq!(serde_json::to_value(rows).unwrap(), expected);
        }
        let cw = equivalents(&c, "snomed", pivot).unwrap();
        assert_eq!(
            cw.equivalents.iter().find(|(s, _)| *s == system).unwrap().1,
            [code]
        );
        let maps =
            sct_rs::commands::codelist::lookup_crosswalks(&c, &[pivot], &[system.to_owned()])
                .unwrap();
        assert_eq!(maps.codes_for(pivot, system), code);

        for format in ["text", "csv", "tsv", "json"] {
            let output = std::process::Command::new(env!("CARGO_BIN_EXE_sct"))
                .args(["map", pivot, "--to", system, "--format", format, "--db"])
                .arg(d.path().join("syn.db"))
                .env("SCT_DATA_HOME", d.path())
                .current_dir(d.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            if format == "json" {
                let records: Vec<serde_json::Value> = stdout
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                assert_eq!(
                    records,
                    [serde_json::json!({
                        "input": pivot, "from": "snomed", "to": system,
                        "target": code, "snomed": pivot, "display": display,
                    })]
                );
            } else if format == "text" {
                assert_eq!(stdout.trim(), format!("{pivot}  \u{2192}  {code}"));
            } else {
                let sep = if format == "csv" { ',' } else { '\t' };
                assert_eq!(stdout, format!(
                    "input{sep}target{sep}snomed{sep}display\n{pivot}{sep}{code}{sep}{pivot}{sep}{display}\n"
                ));
            }
        }
    }
}

#[test]
fn forwarding_keeps_distinct_pivots_but_collapses_duplicate_paths() {
    use sct_rs::sdk::{Snomed, Terminology};
    let (d, c) = build();
    // A second association to the same replacement is a distinct valid history row.
    c.execute(
        "INSERT INTO concept_history (source_id, association, target_id) VALUES (?1, ?2, ?3)",
        rusqlite::params!["9468002", "possibly_equivalent_to", "22298006"],
    )
    .unwrap();
    for pivot in ["22298006", "195967001"] {
        replace_claims(
            &c,
            pivot,
            "icd10",
            "I219",
            &[Some("447559001"), Some("447557004"), Some("447557004")],
        );
    }
    let sdk = Snomed::open(d.path().join("syn.db")).unwrap();
    let rows = sdk
        .map_forwarding_history(Terminology::Snomed, "9468002", Terminology::Icd10)
        .unwrap();
    assert_eq!(
        serde_json::to_value(rows).unwrap(),
        serde_json::json!([
            {"target": "I219", "snomed": "195967001", "display": "Asthma"},
            {"target": "I219", "snomed": "22298006", "display": "Myocardial infarction"},
        ])
    );
    let actual: Vec<_> = transcode_one(&c, "snomed", "9468002", "icd10", true)
        .unwrap()
        .into_iter()
        .map(|m| (m.snomed, m.target, m.correlation, m.display))
        .collect();
    let expected: Vec<_> = [
        ("195967001", "Asthma"),
        ("22298006", "Myocardial infarction"),
    ]
    .into_iter()
    .flat_map(|(pivot, display)| {
        ["447557004", "447559001"]
            .into_iter()
            .map(move |correlation| {
                (
                    pivot.to_owned(),
                    "I219".to_owned(),
                    Some(correlation.to_owned()),
                    Some(display.to_owned()),
                )
            })
    })
    .collect();
    assert_eq!(actual, expected);
}
