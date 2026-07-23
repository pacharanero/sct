// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Snapshot tests (R21) for human-readable formatted output, using `insta`.
//! Freeze the shape of `sct info`, `sct diff`, and `sct trud list` so accidental
//! format regressions surface as snapshot diffs rather than needing hand-written
//! `contains` assertions. All inputs are the committed synthetic RF2 fixture (or
//! a fully-mocked TRUD response), so the output is deterministic; the few
//! volatile fields (file paths, the building `sct` version) are redacted with
//! insta filters.

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn rf2_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn build_ndjson(dir: &Path, name: &str) -> PathBuf {
    let out = dir.join(name);
    Command::cargo_bin("sct")
        .unwrap()
        .args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--locale", "en-GB", "--output"])
        .arg(&out)
        .assert()
        .success();
    out
}

fn build_db(dir: &Path, ndjson: &Path, name: &str) -> PathBuf {
    let out = dir.join(name);
    Command::cargo_bin("sct")
        .unwrap()
        .args(["sqlite", "--ndjson"])
        .arg(ndjson)
        .arg("--output")
        .arg(&out)
        .assert()
        .success();
    out
}

/// Snapshot `value` under `name`, redacting the volatile bits that legitimately
/// change run-to-run or release-to-release (file paths, the building sct
/// version) so the snapshot captures layout + stable content only.
fn snapshot_filtered(name: &str, value: String) {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"(?m)^File:.*$", "File:           [PATH]");
    settings.add_filter(r"(?m)^Built by:.*$", "Built by:       sct [VERSION]");
    settings.add_filter(r#"[^\s"]*\.ndjson"#, "[NDJSON]");
    settings.bind(|| insta::assert_snapshot!(name, value));
}

#[test]
fn info_ndjson_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let ndjson = build_ndjson(tmp.path(), "syn.ndjson");

    let output = Command::cargo_bin("sct")
        .unwrap()
        .arg("info")
        .arg(&ndjson)
        .output()
        .unwrap();
    assert!(output.status.success(), "sct info should succeed");
    let stdout = String::from_utf8(output.stdout).unwrap();

    snapshot_filtered("info_ndjson", stdout);
}

#[test]
fn diff_summary_one_inactivated() {
    let tmp = tempfile::tempdir().unwrap();
    let old = build_ndjson(tmp.path(), "old.ndjson");

    // New release = old minus Myocardial infarction (22298006): exactly one
    // concept disappears, so the diff reports a single Inactivated entry.
    let new = tmp.path().join("new.ndjson");
    let text = std::fs::read_to_string(&old).unwrap();
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !l.contains("\"id\":\"22298006\""))
        .collect();
    std::fs::write(&new, format!("{}\n", kept.join("\n"))).unwrap();

    let output = Command::cargo_bin("sct")
        .unwrap()
        .args(["diff", "--old"])
        .arg(&old)
        .arg("--new")
        .arg(&new)
        .output()
        .unwrap();
    assert!(output.status.success(), "sct diff should succeed");
    let stdout = String::from_utf8(output.stdout).unwrap();

    snapshot_filtered("diff_one_inactivated", stdout);
}

#[test]
fn refset_compare_and_profile_text() {
    const REFSET: &str = "991381000000107";

    let tmp = tempfile::tempdir().unwrap();
    let ndjson = build_ndjson(tmp.path(), "syn.ndjson");
    let db = build_db(tmp.path(), &ndjson, "syn.db");

    let compare = Command::cargo_bin("sct")
        .unwrap()
        .args(["refset", "compare", REFSET, REFSET, "--show", "all", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        compare.status.success(),
        "sct refset compare should succeed"
    );
    insta::assert_snapshot!(
        "refset_compare_text",
        String::from_utf8(compare.stdout).unwrap()
    );

    let profile = Command::cargo_bin("sct")
        .unwrap()
        .args(["refset", "profile", REFSET, "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        profile.status.success(),
        "sct refset profile should succeed"
    );
    insta::assert_snapshot!(
        "refset_profile_text",
        String::from_utf8(profile.stdout).unwrap()
    );

    let empty_profile = Command::cargo_bin("sct")
        .unwrap()
        .args(["refset", "profile", "900000000000508004", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        empty_profile.status.success(),
        "sct refset profile should handle an empty refset"
    );
    insta::assert_snapshot!(
        "refset_empty_profile_text",
        String::from_utf8(empty_profile.stdout).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trud_list_table() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(wm_path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let body = serde_json::json!({
        "releases": [{
            "archiveFileUrl": "https://example.test/uk_monolith_20260101.zip",
            "archiveFileName": "uk_monolith_20260101000001Z.zip",
            "archiveFileSizeBytes": 123_456_789u64,
            "archiveFileSha256":
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            "releaseDate": "2026-01-01"
        }]
    });
    Mock::given(method("GET"))
        .and(wm_path("/keys/test-key/items/1799/releases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let base = server.uri();
    let health = format!("{}/health", server.uri());
    let output = tokio::task::spawn_blocking(move || {
        Command::cargo_bin("sct")
            .unwrap()
            .env("SCT_TRUD_API_BASE", &base)
            .env("SCT_TRUD_HEALTH_URL", &health)
            .env("TRUD_API_KEY", "test-key")
            .args(["trud", "list", "--item", "1799"])
            .output()
            .unwrap()
    })
    .await
    .unwrap();

    assert!(output.status.success(), "sct trud list should succeed");
    let stdout = String::from_utf8(output.stdout).unwrap();
    insta::assert_snapshot!("trud_list", stdout);
}
