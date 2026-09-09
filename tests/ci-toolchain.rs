// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

use serde_yaml_ng::Value;
use std::{collections::BTreeSet, fs, path::Path};

fn workflow(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".github/workflows")
        .join(name);
    serde_yaml_ng::from_str(&fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn run_lines(step: &Value) -> Vec<&str> {
    step["run"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

#[test]
fn repository_toolchain_is_pinned_with_required_components() {
    let toolchain: toml::Value = toml::from_str(include_str!("../rust-toolchain.toml")).unwrap();
    let toolchain = &toolchain["toolchain"];
    let channel = toolchain["channel"].as_str().unwrap();
    let parts: Vec<_> = channel.split('.').collect();
    assert!(
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
        "pin an explicit Rust release in rust-toolchain.toml, not a floating channel: {channel}"
    );
    assert_eq!(toolchain["profile"].as_str(), Some("minimal"));
    let components: BTreeSet<_> = toolchain["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|component| component.as_str().unwrap())
        .collect();
    for required in ["clippy", "rustfmt"] {
        assert!(
            components.contains(required),
            "missing required component: {required}"
        );
    }
}

#[test]
fn every_native_rust_job_sets_up_and_reports_the_file_selected_toolchain() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    let mut native_jobs = BTreeSet::new();
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if !matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap();
        let workflow = workflow(name);
        for (id, job) in workflow["jobs"].as_mapping().unwrap() {
            let context = format!("{name}/{}", id.as_str().unwrap());
            let Some(steps) = job["steps"].as_sequence() else {
                continue; // Reusable workflows are checked in their own files.
            };
            // Source policy for the repo's direct, line-leading run commands, not
            // a shell interpreter. Action internals and Nix/Docker are out of scope.
            let first_native = steps.iter().position(|step| {
                run_lines(step).iter().any(|line| {
                    matches!(
                        line.split_whitespace().next(),
                        Some("cargo" | "rustup" | "rustc")
                    )
                })
            });
            let Some(first_native) = first_native else {
                continue;
            };
            native_jobs.insert(context.clone());
            let checkout = steps
                .iter()
                .position(|step| {
                    step["uses"]
                        .as_str()
                        .is_some_and(|action| action.starts_with("actions/checkout@"))
                })
                .unwrap_or_else(|| panic!("{context}: missing checkout"));
            let setup = steps
                .iter()
                .position(|step| step["name"].as_str() == Some("Set up pinned Rust toolchain"))
                .unwrap_or_else(|| panic!("{context}: missing pinned setup"));
            assert!(
                checkout < setup && setup == first_native,
                "{context}: checkout must precede setup, and setup must be the first native Rust step"
            );
            let step = &steps[setup];
            // Explicit `bash` gives Actions' -e -o pipefail on Windows as well.
            assert_eq!(
                step["shell"].as_str(),
                Some("bash"),
                "{context}: setup shell"
            );
            assert!(
                step["if"].is_null() && step["continue-on-error"].is_null(),
                "{context}: setup must run and fail the job on error"
            );
            let mut expected = vec!["rustup toolchain install --no-self-update"];
            if name == "ci.yml" && id.as_str() == Some("coverage") {
                expected.push("rustup component add llvm-tools-preview");
            }
            expected.extend([
                "rustup show active-toolchain",
                "rustc --version",
                "cargo --version",
            ]);
            assert_eq!(
                run_lines(step),
                expected,
                "{context}: install without a channel, then report active toolchain and versions"
            );

            for scope in std::iter::once(&workflow)
                .chain(std::iter::once(job))
                .chain(steps)
            {
                assert!(
                    scope["env"]
                        .as_mapping()
                        .is_none_or(|env| { !env.contains_key(Value::from("RUSTUP_TOOLCHAIN")) }),
                    "{context}: env must not override rust-toolchain.toml"
                );
            }
            for step in steps {
                for line in run_lines(step) {
                    assert!(
                        !line.contains("RUSTUP_TOOLCHAIN"),
                        "{context}: run must not override the file-selected toolchain: {line}"
                    );
                    let words: Vec<_> = line.split_whitespace().collect();
                    assert!(
                        !words.starts_with(&["rustup", "default"])
                            && !words.starts_with(&["rustup", "override"]),
                        "{context}: do not change toolchain selection: {line}"
                    );
                    if words.starts_with(&["rustup", "toolchain", "install"]) {
                        assert_eq!(
                            line, "rustup toolchain install --no-self-update",
                            "{context}: toolchain install must not name stable or any other channel"
                        );
                    }
                    if matches!(words.first(), Some(&"cargo" | &"rustc" | &"rustup")) {
                        assert!(
                            !words.iter().any(
                                |word| word.starts_with('+') || word.starts_with("--toolchain")
                            ),
                            "{context}: native commands must use the active pin: {line}"
                        );
                    }
                }
            }
        }
    }
    // Named coverage prevents deleted jobs from making the traversal vacuous;
    // newly added native jobs are still checked without updating an allowlist.
    for job in [
        "ci.yml/test",
        "ci.yml/windows-check",
        "ci.yml/windows-test",
        "ci.yml/python",
        "ci.yml/coverage",
        "release.yml/build",
        "release.yml/publish-crates",
    ] {
        assert!(native_jobs.contains(job), "missing native Rust job: {job}");
    }
}

#[test]
fn coverage_and_release_add_components_and_targets_before_use() {
    let ci = workflow("ci.yml");
    let coverage = ci["jobs"]["coverage"]["steps"].as_sequence().unwrap();
    let component = coverage
        .iter()
        .position(|step| run_lines(step).contains(&"rustup component add llvm-tools-preview"))
        .expect("coverage must add LLVM tools to the active pin, without --toolchain stable");
    let instrument = coverage
        .iter()
        .position(|step| {
            run_lines(step)
                .iter()
                .any(|line| line.starts_with("cargo llvm-cov "))
        })
        .expect("coverage must run cargo llvm-cov");
    assert!(
        component < instrument,
        "LLVM tools must be installed before coverage runs"
    );

    let release = workflow("release.yml");
    let build = release["jobs"]["build"]["steps"].as_sequence().unwrap();
    let setup = build
        .iter()
        .position(|step| step["name"].as_str() == Some("Set up pinned Rust toolchain"))
        .unwrap();
    let target = build
        .iter()
        .position(|step| run_lines(step).contains(&"rustup target add ${{ matrix.target }}"))
        .expect("release must add the matrix target to the active pin");
    let compile = build
        .iter()
        .position(|step| {
            run_lines(step)
                .iter()
                .any(|line| line.starts_with("cargo build "))
        })
        .expect("release must build the binary");
    assert!(
        setup < target && target < compile,
        "release target installation must follow setup and precede compilation"
    );
    assert!(
        build[target]["if"].is_null() && build[target]["continue-on-error"].is_null(),
        "target installation must run successfully for every release matrix entry"
    );
    assert!(
        run_lines(&build[compile]).contains(&"cargo build --release --target ${{ matrix.target }}"),
        "release must build for the installed matrix target"
    );
}
