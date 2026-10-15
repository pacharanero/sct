// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Panic-risk audit of the RF2 ingestion and NDJSON -> SQLite build paths.
//!
//! A malformed release (truncated row, missing field, non-UTF8 bytes, empty
//! file, garbage NDJSON) may be rejected or tolerated, but must never panic or
//! abort: the process exit code must be 0 or 1 (an `anyhow` error), never 101
//! (Rust panic) or a signal.

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

fn find(root: &Path, needle: &str) -> PathBuf {
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if let Some(found) = try_find(&path, needle) {
                return found;
            }
        } else if path.file_name().unwrap().to_string_lossy().contains(needle) {
            return path;
        }
    }
    panic!("no file containing {needle}");
}

fn try_find(root: &Path, needle: &str) -> Option<PathBuf> {
    std::panic::catch_unwind(|| find(root, needle)).ok()
}

fn assert_no_panic(args: &[&str], what: &str) -> i32 {
    let output = Command::new(env!("CARGO_BIN_EXE_sct"))
        .args(args)
        .output()
        .unwrap();
    let code = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        matches!(code, Some(0) | Some(1)) && !stderr.contains("panicked at"),
        "{what}: exit {code:?}, stderr:\n{stderr}"
    );
    code.unwrap()
}

type Corruption = (&'static str, fn(&str) -> Vec<u8>);

fn corruptions() -> Vec<Corruption> {
    vec![
        ("empty file", |_| Vec::new()),
        ("header only", |s| {
            format!("{}\n", s.lines().next().unwrap()).into_bytes()
        }),
        ("truncated last line", |s| {
            let cut = s.trim_end().len() - 12;
            s[..cut].as_bytes().to_vec()
        }),
        ("missing tab fields", |s| {
            let mut out = s.lines().next().unwrap().to_string();
            out.push_str("\n123\t2026\n");
            out.into_bytes()
        }),
        ("non-numeric ids", |s| {
            let mut out = s.to_string();
            out.push_str("abc\tdef\t1\tx\ty\tz\tw\tv\tu\tt\tq\n");
            out.into_bytes()
        }),
        ("non-utf8 bytes", |s| {
            let mut out = s.as_bytes().to_vec();
            out.extend_from_slice(b"\xff\xfe\t\xc3\x28\n");
            out
        }),
        ("no trailing newline, lone tab", |s| {
            let mut out = s.to_string();
            out.push('\t');
            out.into_bytes()
        }),
    ]
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
        for (label, corrupt) in corruptions() {
            let rf2 = work.path().join("rf2");
            let _ = std::fs::remove_dir_all(&rf2);
            copy_dir(&fixture_dir(), &rf2);
            let target = find(&rf2, needle);
            let original = std::fs::read_to_string(&target).unwrap();
            std::fs::write(&target, corrupt(&original)).unwrap();
            let out = work.path().join("out.ndjson");
            assert_no_panic(
                &[
                    "ndjson",
                    "--rf2",
                    rf2.to_str().unwrap(),
                    "--refsets",
                    "all",
                    "--output",
                    out.to_str().unwrap(),
                ],
                &format!("{needle} / {label}"),
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

    let variants: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("garbage", b"not json\n{{{\n".to_vec()),
        ("non-utf8", b"\xff\xfe\n".to_vec()),
        ("json null line", b"null\n[]\n42\n\"x\"\n".to_vec()),
        ("empty object", b"{}\n".to_vec()),
        ("truncated", text.as_bytes()[..text.len() / 2].to_vec()),
        (
            "missing first line (provenance)",
            text.lines()
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes(),
        ),
    ];
    for (label, bytes) in variants {
        let input = work.path().join("bad.ndjson");
        let db = work.path().join("bad.db");
        let _ = std::fs::remove_file(&db);
        std::fs::write(&input, bytes).unwrap();
        assert_no_panic(
            &[
                "sqlite",
                "--ndjson",
                input.to_str().unwrap(),
                "--output",
                db.to_str().unwrap(),
            ],
            label,
        );
    }
}
