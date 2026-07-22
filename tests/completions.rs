// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

use std::process::Command;

#[test]
fn completions_generate_and_install() {
    let bin = env!("CARGO_BIN_EXE_sct");
    let out_dir = std::env::temp_dir().join(format!("sct-completions-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out_dir);

    let output = Command::new(bin)
        .args(["completions", "bash"])
        .output()
        .expect("run sct completions bash");
    assert!(output.status.success());
    let bash = String::from_utf8(output.stdout).expect("bash completions are utf8");
    assert!(bash.contains("complete"));
    assert!(bash.contains("completions"));

    let output = Command::new(bin)
        .args([
            "completions",
            "install",
            "--shell",
            "zsh",
            "--dir",
            out_dir.to_str().expect("temp path"),
        ])
        .output()
        .expect("run sct completions install");
    assert!(output.status.success());
    assert!(out_dir.join("_sct").exists());
}
