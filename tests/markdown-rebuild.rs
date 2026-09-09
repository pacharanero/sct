// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

use sct_rs::commands::ndjson;
use sct_rs::schema::ConceptRecord;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MI: &str = "22298006";
const MI_FILE: &str = "clinical-finding/22298006.md";

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("fixture.ndjson");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")],
        locale: "en-GB".into(),
        output: Some(input.clone()),
        include_inactive: false,
        refsets: ndjson::RefsetMode::Simple,
    })
    .unwrap();
    let mi = fs::read_to_string(&input)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<ConceptRecord>(line).ok())
        .find(|record| record.id == MI)
        .unwrap();
    assert_eq!(mi.preferred_term, "Myocardial infarction");
    assert_eq!(mi.hierarchy, "Clinical finding");
    (dir, input)
}

fn export(root: &Path, input: &Path, output: Option<&Path>, mode: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sct"));
    command
        .current_dir(root)
        .env_remove("SCT_CONFIG")
        .env("SCT_CONFIG_HOME", root)
        .env("SCT_DATA_HOME", root)
        .args(["markdown", "--ndjson"])
        .arg(input)
        .args(["--mode", mode]);
    if let Some(output) = output {
        command.arg("--output").arg(output);
    }
    command.output().unwrap()
}

fn success(result: Output) {
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

// Include directories themselves so deleting an empty directory also fails.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut entries = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = PathBuf::from(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            entries.insert(name.clone(), None);
            for (child, bytes) in snapshot(&entry.path()) {
                entries.insert(name.join(child), bytes);
            }
        } else {
            entries.insert(name, Some(fs::read(entry.path()).unwrap()));
        }
    }
    entries
}

fn refused(root: &Path, input: &Path, output: &Path, mode: &str, implicit: bool) {
    let before = snapshot(output);
    let result = export(root, input, (!implicit).then_some(output), mode);
    assert!(
        snapshot(output) == before,
        "refusal must preserve every entry"
    );
    assert!(!result.status.success(), "nonempty output was accepted");
    let error = String::from_utf8_lossy(&result.stderr).to_lowercase();
    assert!(
        error.contains("nonempty") || error.contains("non-empty") || error.contains("not empty"),
        "expected a nonempty-directory diagnostic, got: {error}"
    );
}

fn sentinels(output: &Path) {
    fs::write(output.join("unrelated.txt"), b"keep me\0\xff").unwrap();
    fs::write(output.join(".hidden"), b"hidden contents").unwrap();
    fs::create_dir(output.join("empty-subdir")).unwrap();
}

fn modify(input: &Path, change: &str) {
    let text = fs::read_to_string(input).unwrap();
    let mut modified = String::new();
    for line in text.lines() {
        if let Ok(mut record) = serde_json::from_str::<ConceptRecord>(line) {
            // Default RF2 ingestion omits inactive concepts from canonical NDJSON.
            if (change == "removed" && record.id == MI)
                || (change == "hierarchy-removed" && record.hierarchy == "Clinical finding")
            {
                continue;
            }
            if change == "moved" && record.id == MI {
                record.hierarchy = "Procedure".into();
                record.hierarchy_path = vec!["SNOMED CT Concept".into(), "Procedure".into()];
            }
            modified.push_str(&serde_json::to_string(&record).unwrap());
        } else {
            modified.push_str(line);
        }
        modified.push('\n');
    }
    fs::write(input, modified).unwrap();
}

fn changed_canonical(change: &str, mode: &str) {
    let (dir, input) = fixture();
    let old = dir.path().join("old");
    success(export(dir.path(), &input, Some(&old), mode));
    let initial_file = if mode == "concept" {
        MI_FILE
    } else {
        "clinical-finding.md"
    };
    assert!(fs::read_to_string(old.join(initial_file))
        .unwrap()
        .contains(MI));
    sentinels(&old);
    modify(&input, change);

    // Verify the changed artefact independently, before exercising refusal.
    let fresh = dir.path().join("fresh");
    fs::create_dir(&fresh).unwrap();
    success(export(dir.path(), &input, Some(&fresh), mode));
    assert!(!fresh.join(initial_file).exists());
    if change == "moved" {
        assert!(fs::read_to_string(fresh.join("procedure/22298006.md"))
            .unwrap()
            .contains("Myocardial infarction"));
    } else if mode == "concept" {
        assert!(fresh.join("clinical-finding/46635009.md").exists());
    } else {
        assert!(fresh.join("procedure.md").exists());
    }
    refused(dir.path(), &input, &old, mode, false);
}

#[test]
fn removed_concept_requires_fresh_output() {
    changed_canonical("removed", "concept");
}

#[test]
fn moved_concept_requires_fresh_output() {
    changed_canonical("moved", "concept");
}

#[test]
fn vanished_hierarchy_requires_fresh_output() {
    changed_canonical("hierarchy-removed", "hierarchy");
}

#[test]
fn repeated_exports_and_mode_changes_preserve_existing_output() {
    let (dir, input) = fixture();
    for initial in ["concept", "hierarchy"] {
        for next in ["concept", "hierarchy"] {
            let output = dir.path().join(format!("{initial}-{next}"));
            success(export(dir.path(), &input, Some(&output), initial));
            sentinels(&output);
            refused(dir.path(), &input, &output, next, false);
        }
    }
}

#[test]
fn any_entry_makes_output_nonempty() {
    let (dir, input) = fixture();
    for mode in ["concept", "hierarchy"] {
        for kind in ["sentinel", "hidden", "empty-directory"] {
            let output = dir.path().join(format!("{mode}-{kind}"));
            fs::create_dir(&output).unwrap();
            match kind {
                "empty-directory" => fs::create_dir(output.join("empty")).unwrap(),
                "hidden" => fs::write(output.join(".hidden"), b"keep").unwrap(),
                _ => fs::write(output.join("unrelated.txt"), b"keep").unwrap(),
            }
            refused(dir.path(), &input, &output, mode, false);
        }
    }
}

#[test]
fn derived_default_output_is_guarded_in_both_modes() {
    for mode in ["concept", "hierarchy"] {
        let (dir, input) = fixture();
        let output = dir.path().join("fixture-concepts");
        success(export(dir.path(), &input, None, mode));
        assert!(output.is_dir());
        sentinels(&output);
        refused(dir.path(), &input, &output, mode, true);
    }
}

#[test]
fn nonempty_output_is_rejected_before_opening_input() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("missing.ndjson");
    assert!(!input.exists());
    for mode in ["concept", "hierarchy"] {
        let output = dir.path().join(mode);
        fs::create_dir(&output).unwrap();
        sentinels(&output);
        // Missing input establishes preflight precedence without a blocking stdin test.
        refused(dir.path(), &input, &output, mode, false);
    }
}
