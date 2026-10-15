// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Panic-risk audit of the RF2 ingestion and NDJSON -> SQLite build paths.
//!
//! A malformed release (truncated row, missing field, non-UTF8 bytes, empty
//! file, garbage NDJSON) must follow its documented tolerance or error path
//! without panicking or aborting.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn find(root: &Path, needle: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if let Some(found) = find(&path, needle) {
                return Some(found);
            }
        } else if path.file_name().unwrap().to_string_lossy().contains(needle) {
            return Some(path);
        }
    }
    None
}

fn assert_exit(args: &[&str], what: &str, expected: i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_sct"))
        .args(args)
        .output()
        .unwrap();
    let code = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked at"),
        "{what}: process panicked, stderr:\n{stderr}"
    );
    assert_eq!(
        code,
        Some(expected),
        "{what}: expected exit {expected}, stderr:\n{stderr}"
    );
    if expected != 0 {
        assert!(
            stderr.contains("Error:"),
            "{what}: failure did not include error context:\n{stderr}"
        );
    }
}

struct Corruption {
    label: &'static str,
    apply: fn(&str) -> Vec<u8>,
    expected_exit: i32,
}

fn truncate_last_row(s: &str) -> Vec<u8> {
    let trimmed = s.trim_end_matches(['\r', '\n']);
    let row_start = trimmed.rfind('\n').map_or(0, |index| index + 1);
    let final_tab = trimmed[row_start..]
        .rfind('\t')
        .expect("fixture data row has at least one tab");
    trimmed.as_bytes()[..row_start + final_tab].to_vec()
}

fn replace_first_identifier(s: &str) -> Vec<u8> {
    let header_end = s.find('\n').expect("fixture has a header line") + 1;
    let id_end = s[header_end..]
        .find('\t')
        .expect("fixture data row has an identifier")
        + header_end;
    let mut out = s.as_bytes().to_vec();
    out.splice(header_end..id_end, b"abc".iter().copied());
    out
}

fn corruptions() -> Vec<Corruption> {
    vec![
        Corruption {
            label: "empty file",
            apply: |_| Vec::new(),
            expected_exit: 0,
        },
        Corruption {
            label: "header only",
            apply: |s| format!("{}\n", s.lines().next().unwrap()).into_bytes(),
            expected_exit: 0,
        },
        Corruption {
            label: "truncated last row",
            apply: truncate_last_row,
            expected_exit: 1,
        },
        Corruption {
            label: "missing tab fields",
            apply: |s| {
                let mut out = s.lines().next().unwrap().to_string();
                out.push_str("\n123\t2026\n");
                out.into_bytes()
            },
            expected_exit: 1,
        },
        Corruption {
            label: "non-numeric identifier",
            apply: replace_first_identifier,
            expected_exit: 0,
        },
        Corruption {
            label: "non-utf8 bytes",
            apply: |s| {
                let mut out = s.as_bytes().to_vec();
                out.extend_from_slice(b"\xff\xfe\t\xc3\x28\n");
                out
            },
            expected_exit: 1,
        },
        Corruption {
            label: "no trailing newline, lone tab",
            apply: |s| {
                let mut out = s.to_string();
                out.push('\t');
                out.into_bytes()
            },
            expected_exit: 1,
        },
    ]
}

fn expected_rf2_exit(needle: &str, corruption: &Corruption) -> i32 {
    let checked_parser = needle.contains("ComplexMap")
        || needle.contains("ExtendedMap")
        || needle.contains("AttributeValue");
    if corruption.label == "empty file" && checked_parser {
        1
    } else {
        corruption.expected_exit
    }
}

#[test]
fn corrupted_rf2_files_never_panic_in_ndjson() {
    let work = tempfile::tempdir().unwrap();
    for needle in [
        "sct2_Concept_Snapshot",
        "sct2_Description_Snapshot",
        "sct2_Relationship_Snapshot",
        "der2_cRefset_LanguageSnapshot",
        "der2_Refset_SimpleSnapshot",
        "der2_sRefset_SimpleMapSnapshot",
        "der2_cRefset_AssociationSnapshot",
        "der2_cRefset_AttributeValueSnapshot",
        "der2_iissscRefset_ComplexMapSnapshot",
        "der2_iisssciRefset_ExtendedMapSnapshot",
        "der2_iisssccRefset_ExtendedMapSnapshot",
    ] {
        for corruption in corruptions() {
            let rf2 = work.path().join("rf2");
            let _ = std::fs::remove_dir_all(&rf2);
            copy_dir(&fixture_dir(), &rf2);
            let target =
                find(&rf2, needle).unwrap_or_else(|| panic!("no file containing {needle}"));
            let original = std::fs::read_to_string(&target).unwrap();
            std::fs::write(&target, (corruption.apply)(&original)).unwrap();
            let out = work.path().join("out.ndjson");
            assert_exit(
                &[
                    "ndjson",
                    "--rf2",
                    rf2.to_str().unwrap(),
                    "--refsets",
                    "all",
                    "--output",
                    out.to_str().unwrap(),
                ],
                &format!("{needle} / {}", corruption.label),
                expected_rf2_exit(needle, &corruption),
            );
        }
    }
}

#[test]
fn corrupted_ndjson_never_panics_in_sqlite() {
    let work = tempfile::tempdir().unwrap();
    let good = work.path().join("good.ndjson");
    let status = Command::new(env!("CARGO_BIN_EXE_sct"))
        .args([
            "ndjson",
            "--rf2",
            fixture_dir().to_str().unwrap(),
            "--output",
            good.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let text = std::fs::read_to_string(&good).unwrap();

    let variants: Vec<(&str, Vec<u8>, i32)> = vec![
        ("empty", Vec::new(), 0),
        ("garbage", b"not json\n{{{\n".to_vec(), 1),
        ("non-utf8", b"\xff\xfe\n".to_vec(), 1),
        ("json null line", b"null\n[]\n42\n\"x\"\n".to_vec(), 1),
        ("empty object", b"{}\n".to_vec(), 1),
        ("truncated", text.as_bytes()[..text.len() / 2].to_vec(), 1),
        (
            "missing first line (provenance)",
            text.lines()
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes(),
            0,
        ),
    ];
    for (label, bytes, expected_exit) in variants {
        let input = work.path().join("bad.ndjson");
        let db = work.path().join("bad.db");
        let _ = std::fs::remove_file(&db);
        std::fs::write(&input, bytes).unwrap();
        assert_exit(
            &[
                "sqlite",
                "--ndjson",
                input.to_str().unwrap(),
                "--output",
                db.to_str().unwrap(),
            ],
            label,
            expected_exit,
        );
    }
}

#[test]
fn corrupted_ndjson_companions_fail_cleanly_in_sqlite() {
    let work = tempfile::tempdir().unwrap();
    let good = work.path().join("good.ndjson");
    let status = Command::new(env!("CARGO_BIN_EXE_sct"))
        .args([
            "ndjson",
            "--rf2",
            fixture_dir().to_str().unwrap(),
            "--refsets",
            "all",
            "--output",
            good.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());

    let good_refsets = companion_path(&good, "refsets");
    let good_history = companion_path(&good, "history");
    assert!(good_refsets.is_file());
    assert!(good_history.is_file());

    for (companion, source) in [("refsets", &good_refsets), ("history", &good_history)] {
        let original = std::fs::read(source).unwrap();
        for (label, suffix) in [
            ("garbage JSON", b"not json\n".as_slice()),
            ("non-UTF8", b"\xff\xfe\n".as_slice()),
        ] {
            let input = work.path().join(format!("bad-{companion}-{label}.ndjson"));
            std::fs::copy(&good, &input).unwrap();
            std::fs::copy(&good_refsets, companion_path(&input, "refsets")).unwrap();
            std::fs::copy(&good_history, companion_path(&input, "history")).unwrap();

            let mut corrupted = original.clone();
            corrupted.extend_from_slice(suffix);
            std::fs::write(companion_path(&input, companion), corrupted).unwrap();

            let db = work.path().join(format!("bad-{companion}-{label}.db"));
            assert_exit(
                &[
                    "sqlite",
                    "--ndjson",
                    input.to_str().unwrap(),
                    "--output",
                    db.to_str().unwrap(),
                ],
                &format!("{companion} companion / {label}"),
                1,
            );
        }
    }
}

fn companion_path(input: &Path, kind: &str) -> PathBuf {
    let input = input.to_string_lossy();
    let base = input.strip_suffix(".ndjson").unwrap_or(&input);
    PathBuf::from(format!("{base}.{kind}.ndjson"))
}
