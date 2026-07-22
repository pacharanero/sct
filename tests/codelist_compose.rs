// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Composable `.codelist` files: `includes:` resolution, exclusion override,
//! cycle detection, bare-id vs path references, and `resolve` flattening.

use sct_rs::commands::codelist::{effective_members_of, read_codelist, MemberSource};
use std::fs;
use std::path::{Path, PathBuf};

/// Write a minimal valid `.codelist` with the given id, optional `includes:`,
/// and concept body lines (each `"<id> <term>"`, or `"# <id> <term>"` to exclude).
fn write_list(dir: &Path, id: &str, includes: &[&str], concepts: &[&str]) -> PathBuf {
    let inc = if includes.is_empty() {
        String::new()
    } else {
        let mut s = String::from("includes:\n");
        for i in includes {
            s.push_str(&format!("  - {i}\n"));
        }
        s
    };
    let body: String = concepts
        .iter()
        .map(|c| format!("{c}\n"))
        .collect::<String>();
    let text = format!(
        "---\n\
         id: {id}\n\
         title: {id}\n\
         description: test\n\
         terminology: SNOMED CT\n\
         created: 2026-01-01\n\
         updated: 2026-01-01\n\
         version: 1\n\
         status: active\n\
         licence: CC-BY-4.0\n\
         copyright: x\n\
         appropriate_use: x\n\
         misuse: x\n\
         {inc}\
         ---\n\n# concepts\n{body}"
    );
    let path = dir.join(format!("{id}.codelist"));
    fs::write(&path, text).unwrap();
    path
}

fn member_ids(dir: &Path, file: &Path) -> Vec<String> {
    let cl = read_codelist(file).unwrap();
    effective_members_of(&cl, file, dir, false)
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect()
}

#[test]
fn union_of_includes_plus_own() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    write_list(dir, "child-a", &[], &["111 One", "222 Two"]);
    write_list(dir, "child-b", &[], &["333 Three"]);
    let parent = write_list(dir, "parent", &["child-a", "child-b"], &["999 Own"]);

    let mut ids = member_ids(dir, &parent);
    ids.sort();
    assert_eq!(ids, ["111", "222", "333", "999"]);
}

#[test]
fn parent_exclusion_overrides_included_member() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    write_list(dir, "child-a", &[], &["111 One", "222 Two"]);
    // Parent excludes 222 that child-a contributes.
    let parent = write_list(dir, "parent", &["child-a"], &["999 Own", "# 222 Two"]);

    let mut ids = member_ids(dir, &parent);
    ids.sort();
    assert_eq!(ids, ["111", "999"]);
}

#[test]
fn direct_member_wins_provenance_over_inherited() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    write_list(dir, "child-a", &[], &["111 Inherited term"]);
    // Parent also declares 111 directly with its own term.
    let parent = write_list(dir, "parent", &["child-a"], &["111 Direct term"]);

    let cl = read_codelist(&parent).unwrap();
    let members = effective_members_of(&cl, &parent, dir, false).unwrap();
    let m = members.iter().find(|m| m.id == "111").unwrap();
    assert_eq!(m.source, MemberSource::Direct);
    assert_eq!(m.term, "Direct term");
    assert_eq!(members.len(), 1); // deduped
}

#[test]
fn transitive_includes_resolve() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    write_list(dir, "leaf", &[], &["111 One"]);
    write_list(dir, "mid", &["leaf"], &["222 Two"]);
    let top = write_list(dir, "top", &["mid"], &["333 Three"]);

    let mut ids = member_ids(dir, &top);
    ids.sort();
    assert_eq!(ids, ["111", "222", "333"]);
}

#[test]
fn cycle_is_detected() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    write_list(dir, "a", &["b"], &["111 One"]);
    let b = write_list(dir, "b", &["a"], &["222 Two"]);

    let cl = read_codelist(&b).unwrap();
    let err = effective_members_of(&cl, &b, dir, false).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("cycle")
            || format!("{err:#}").to_lowercase().contains("cycle"),
        "expected a cycle error, got: {err:#}"
    );
}

#[test]
fn relative_path_reference_resolves() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    let sub = dir.join("shared");
    fs::create_dir_all(&sub).unwrap();
    write_list(&sub, "renal", &[], &["111 Renal"]);
    // Parent in dir references the child by relative path, not bare id.
    let parent = write_list(dir, "parent", &["shared/renal.codelist"], &["999 Own"]);

    let mut ids = member_ids(dir, &parent);
    ids.sort();
    assert_eq!(ids, ["111", "999"]);
}

#[test]
fn missing_include_errors() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    let parent = write_list(dir, "parent", &["does-not-exist"], &["999 Own"]);
    let cl = read_codelist(&parent).unwrap();
    assert!(effective_members_of(&cl, &parent, dir, false).is_err());
}
