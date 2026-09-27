// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

fn entrypoint_args(env: &[(&str, &str)]) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let fake_sct = dir.path().join("sct");
    fs::write(&fake_sct, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    let mut permissions = fs::metadata(&fake_sct).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_sct, permissions).unwrap();

    let mut paths = vec![dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths).unwrap();
    let entrypoint = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docker/entrypoint.sh");
    let output = Command::new("sh")
        .arg(entrypoint)
        .env_clear()
        .env("PATH", path)
        .env("SCT_DB", "/test/snomed.db")
        .env("SCT_BOOTSTRAP", "false")
        .envs(env.iter().copied())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "entrypoint failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn public_url(args: &[String]) -> Option<&str> {
    args.windows(2)
        .find(|pair| pair[0] == "--public-url")
        .map(|pair| pair[1].as_str())
}

#[test]
fn bundled_caddy_public_url_defaults_and_precedence_are_explicit() {
    let local = entrypoint_args(&[("SCT_PUBLIC_URL_FALLBACK", "http://localhost/fhir")]);
    assert_eq!(public_url(&local), Some("http://localhost/fhir"));

    let domain = entrypoint_args(&[
        ("DOMAIN", "fhir.example.org"),
        ("SCT_FHIR_BASE", "fhir/"),
        ("SCT_PUBLIC_URL_FALLBACK", "http://localhost/fhir"),
    ]);
    assert_eq!(public_url(&domain), Some("https://fhir.example.org/fhir"));

    let explicit = entrypoint_args(&[
        ("DOMAIN", "fhir.example.org"),
        ("SCT_PUBLIC_URL", "https://proxy.example.net/terminology"),
        ("SCT_PUBLIC_URL_FALLBACK", "http://localhost/fhir"),
    ]);
    assert_eq!(
        public_url(&explicit),
        Some("https://proxy.example.net/terminology")
    );
}

#[test]
fn compose_files_configure_the_bundled_caddy_fallback() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for name in ["compose.yaml", "compose.hub.yaml"] {
        let compose = fs::read_to_string(root.join(name)).unwrap();
        assert!(
            compose.contains("SCT_PUBLIC_URL_FALLBACK: http://localhost/fhir"),
            "{name} does not configure the local advertised URL"
        );
    }
}
