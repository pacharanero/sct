// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Network-layer tests (R19) for `sct trud` against a mocked TRUD API served by
//! `wiremock`. The `sct` binary is driven via `assert_cmd` and pointed at the
//! mock through per-subprocess env overrides (`SCT_TRUD_API_BASE`,
//! `SCT_TRUD_HEALTH_URL`), so no real network or TRUD key is involved and there
//! is no global-env contention between tests. Covers the paths the in-crate unit
//! tests never exercised: `fetch_releases` (list), the `check` exit-2 signal,
//! and download with SHA-256 verification (match and mismatch).

use assert_cmd::Command;
use predicates::prelude::*;
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "test-key";
const ITEM: &str = "1799";

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// One-release TRUD list payload (camelCase, matching the real API fields).
fn releases_json(name: &str, url: &str, sha: &str) -> serde_json::Value {
    serde_json::json!({
        "releases": [{
            "archiveFileUrl": url,
            "archiveFileName": name,
            "archiveFileSizeBytes": 1024,
            "archiveFileSha256": sha,
            "releaseDate": "2026-01-01"
        }]
    })
}

async fn mount_health(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;
}

async fn mount_releases(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(format!("/keys/{KEY}/items/{ITEM}/releases")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// An `sct` command wired to the mock TRUD API via per-process env overrides.
fn sct_trud(base: &str, health: &str) -> Command {
    let mut c = Command::cargo_bin("sct").expect("sct binary builds");
    c.env("SCT_TRUD_API_BASE", base)
        .env("SCT_TRUD_HEALTH_URL", health)
        .env("TRUD_API_KEY", KEY);
    c
}

/// Run a blocking `assert_cmd` command off the async reactor so the wiremock
/// server keeps its worker threads free to answer the subprocess.
async fn run(cmd: Command) -> assert_cmd::assert::Assert {
    let mut cmd = cmd;
    tokio::task::spawn_blocking(move || cmd.assert())
        .await
        .expect("assert task joins")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_shows_available_release() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    mount_releases(
        &server,
        releases_json(
            "uk_release_20260101.zip",
            &format!("{}/download/uk_release_20260101.zip", server.uri()),
            "deadbeef",
        ),
    )
    .await;

    let health = format!("{}/health", server.uri());
    let mut cmd = sct_trud(&server.uri(), &health);
    cmd.args(["trud", "list", "--item", ITEM]);
    run(cmd)
        .await
        .success()
        .stdout(predicate::str::contains("uk_release_20260101.zip"))
        .stdout(predicate::str::contains("1.0 KB"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn check_reports_new_release_with_exit_2() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    mount_releases(
        &server,
        releases_json(
            "rel.zip",
            &format!("{}/download/rel.zip", server.uri()),
            "deadbeef",
        ),
    )
    .await;

    // Empty data home => the release is "not present locally" => exit 2.
    let data_home = tempfile::tempdir().unwrap();
    let health = format!("{}/health", server.uri());
    let mut cmd = sct_trud(&server.uri(), &health);
    cmd.env("SCT_DATA_HOME", data_home.path())
        .args(["trud", "check", "--item", ITEM]);
    run(cmd)
        .await
        .failure()
        .code(2)
        .stdout(predicate::str::contains("New release available"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_verifies_sha256() {
    let body = b"synthetic snomed release archive";
    let sha = sha256_hex(body);

    let server = MockServer::start().await;
    mount_health(&server).await;
    mount_releases(
        &server,
        releases_json(
            "rel.zip",
            &format!("{}/download/rel.zip", server.uri()),
            &sha,
        ),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/download/rel.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
        .mount(&server)
        .await;

    let out = tempfile::tempdir().unwrap();
    let health = format!("{}/health", server.uri());
    let mut cmd = sct_trud(&server.uri(), &health);
    cmd.args(["trud", "download", "--item", ITEM, "--output-dir"])
        .arg(out.path());
    run(cmd).await.success();

    assert!(
        out.path().join("rel.zip").exists(),
        "verified archive should be saved to the output dir"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_rejects_sha256_mismatch() {
    let body = b"synthetic snomed release archive";
    let wrong_sha = "0".repeat(64);

    let server = MockServer::start().await;
    mount_health(&server).await;
    mount_releases(
        &server,
        releases_json(
            "rel.zip",
            &format!("{}/download/rel.zip", server.uri()),
            &wrong_sha,
        ),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/download/rel.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
        .mount(&server)
        .await;

    let out = tempfile::tempdir().unwrap();
    let health = format!("{}/health", server.uri());
    let mut cmd = sct_trud(&server.uri(), &health);
    cmd.args(["trud", "download", "--item", ITEM, "--output-dir"])
        .arg(out.path());
    run(cmd)
        .await
        .failure()
        .stderr(predicate::str::contains("checksum mismatch"));

    assert!(
        !out.path().join("rel.zip").exists(),
        "a corrupt download must not be committed to the final path"
    );
    assert!(
        std::fs::read_dir(out.path()).unwrap().next().is_none(),
        "a failed download must not leave a temporary file behind"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_rejects_traversal_in_archive_filename() {
    let server = MockServer::start().await;
    mount_health(&server).await;
    mount_releases(
        &server,
        releases_json(
            "../../escape.zip",
            &format!("{}/download/escape.zip", server.uri()),
            "deadbeef",
        ),
    )
    .await;

    let parent = tempfile::tempdir().unwrap();
    let out = parent.path().join("releases");
    let health = format!("{}/health", server.uri());
    let mut cmd = sct_trud(&server.uri(), &health);
    cmd.args(["trud", "download", "--item", ITEM, "--output-dir"])
        .arg(&out);
    run(cmd)
        .await
        .failure()
        .stderr(predicate::str::contains("unsafe TRUD archiveFileName"));

    assert!(!parent.path().join("escape.zip").exists());
}
