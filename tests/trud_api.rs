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
async fn command_line_api_key_emits_security_warning() {
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
    cmd.args(["trud", "list", "--item", ITEM, "--api-key", KEY]);
    run(cmd)
        .await
        .success()
        .stderr(predicate::str::contains(
            "warning: --api-key exposes the key in process listings and shell history",
        ))
        .stderr(predicate::str::contains(KEY).not());
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

/// Lead #1 from the roadmap's bug-audit backlog: "is a stale partial ever
/// picked up as complete?" Simulate the worst case - a truncated/corrupt file
/// already sitting at the final destination path, as a crash mid-download
/// might leave behind - and confirm a subsequent `sct trud download` verifies
/// it against the real checksum rather than trusting mere presence-on-disk,
/// and replaces it with the genuine, fully-verified content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_replaces_a_stale_local_file_that_fails_checksum() {
    let body = b"synthetic snomed release archive - full and correct";
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
    // Pre-seed the destination with a short, truncated stand-in for a file a
    // crashed prior download might have left behind - it must never be
    // mistaken for the real, complete release.
    std::fs::write(out.path().join("rel.zip"), b"truncated leftover").unwrap();

    let health = format!("{}/health", server.uri());
    let mut cmd = sct_trud(&server.uri(), &health);
    cmd.args(["trud", "download", "--item", ITEM, "--output-dir"])
        .arg(out.path());
    run(cmd)
        .await
        .success()
        .stderr(predicate::str::contains("unexpected SHA-256"));

    let saved = std::fs::read(out.path().join("rel.zip")).unwrap();
    assert_eq!(
        saved,
        body.to_vec(),
        "the stale/truncated file must be replaced by the fully verified download, \
         never left in place or trusted by presence alone"
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

// ---------------------------------------------------------------------------
// sct trud auth
// ---------------------------------------------------------------------------

/// An `sct trud auth` command with a private config home and no ambient key.
///
/// `TRUD_API_KEY` is deliberately removed: `sct_trud` sets it for the read
/// subcommands, but for `auth` it would only trigger the "env var shadows the
/// config" warning.
fn sct_auth(base: &str, config_home: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("sct").expect("sct binary builds");
    c.env("SCT_TRUD_API_BASE", base)
        .env("SCT_CONFIG_HOME", config_home)
        .env_remove("TRUD_API_KEY")
        .env_remove("SCT_CONFIG");
    c
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_verifies_key_then_writes_config() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/keys/{KEY}/items/{ITEM}/releases")))
        .respond_with(ResponseTemplate::new(200).set_body_json(releases_json(
            "rel.zip",
            "https://example.test/rel.zip",
            "ab",
        )))
        .mount(&server)
        .await;

    let config_home = tempfile::tempdir().unwrap();
    let mut cmd = sct_auth(&server.uri(), config_home.path());
    cmd.args(["trud", "auth", KEY]);
    run(cmd)
        .await
        .success()
        .stderr(predicate::str::contains("key verified against TRUD"))
        // Only the last four characters may ever be echoed.
        .stderr(predicate::str::contains("****-key"))
        .stderr(predicate::str::contains(KEY).not());

    let written = std::fs::read_to_string(config_home.path().join("config.toml")).unwrap();
    assert!(
        written.contains(&format!("api_key = \"{KEY}\"")),
        "key must be stored, got:\n{written}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(config_home.path().join("config.toml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "a file holding a credential must be 0600");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_refuses_to_store_a_key_trud_rejects() {
    let server = MockServer::start().await;
    // TRUD answers HTTP 400 for a key it does not recognise.
    Mock::given(method("GET"))
        .and(path(format!("/keys/bogus-key/items/{ITEM}/releases")))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;

    let config_home = tempfile::tempdir().unwrap();
    let mut cmd = sct_auth(&server.uri(), config_home.path());
    cmd.args(["trud", "auth", "bogus-key"]);
    run(cmd)
        .await
        .failure()
        .stderr(predicate::str::contains("TRUD API key invalid"));

    assert!(
        !config_home.path().join("config.toml").exists(),
        "a rejected key must not be written to the config"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_stores_key_offline_with_no_verify() {
    // No mock server mounted: --no-verify must not make any request at all.
    let config_home = tempfile::tempdir().unwrap();
    let mut cmd = sct_auth("http://127.0.0.1:1/unreachable", config_home.path());
    cmd.args(["trud", "auth", "OFFLINEKEY", "--no-verify"]);
    run(cmd).await.success();

    let written = std::fs::read_to_string(config_home.path().join("config.toml")).unwrap();
    assert!(written.contains("api_key = \"OFFLINEKEY\""));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_dry_run_writes_nothing() {
    let config_home = tempfile::tempdir().unwrap();
    let mut cmd = sct_auth("http://127.0.0.1:1/unreachable", config_home.path());
    cmd.args(["trud", "auth", "SOMEKEY", "--no-verify", "--dry-run"]);
    run(cmd)
        .await
        .success()
        .stdout(predicate::str::contains("api_key = \"SOMEKEY\""));

    assert!(
        !config_home.path().join("config.toml").exists(),
        "--dry-run must not create the config file"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_then_list_uses_the_stored_key() {
    // End to end: the key auth writes is the key the next command resolves.
    let server = MockServer::start().await;
    mount_health(&server).await;
    mount_releases(
        &server,
        releases_json("stored.zip", "https://example.test/stored.zip", "ab"),
    )
    .await;

    let config_home = tempfile::tempdir().unwrap();
    let mut auth = sct_auth(&server.uri(), config_home.path());
    auth.args(["trud", "auth", KEY, "--no-verify"]);
    run(auth).await.success();

    let health = format!("{}/health", server.uri());
    let mut list = sct_auth(&server.uri(), config_home.path());
    list.env("SCT_TRUD_HEALTH_URL", &health)
        .args(["trud", "list", "--item", ITEM]);
    run(list)
        .await
        .success()
        .stdout(predicate::str::contains("stored.zip"));
}
