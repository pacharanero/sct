// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

//! Embed + semantic-search smoke test (R17, Option A): drive `sct embed` and
//! `sct semantic` through the real binary against a mocked Ollama `/api/embed`
//! endpoint. A deterministic token-hash embedding stands in for
//! nomic-embed-text, so the test is hermetic (no Ollama, no model download) and
//! validates the *plumbing* - request batching, the Arrow round-trip with its
//! model/dimension metadata, and cosine top-k ranking - rather than the real
//! model's semantic quality. Because `sct embed`/`sct semantic` both accept
//! `--ollama-url`, no source seam is needed.
//!
//! The mock embeds a bag-of-hashed-tokens per input text (L2-normalised), so
//! texts that share words score high under cosine. The query "heart attack"
//! therefore surfaces Myocardial infarction (22298006), whose document text
//! carries the synonym "Heart attack".

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const DIM: usize = 64;

fn rf2_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

/// Deterministic stand-in for a real embedding: hash each content token into a
/// fixed-dimension bag-of-words, then L2-normalise. Shared tokens -> high
/// cosine. The `search_document:` / `search_query:` prefixes `sct` adds are
/// stripped so only content words drive similarity.
fn embed_text(text: &str) -> Vec<f32> {
    let content = text
        .strip_prefix("search_document: ")
        .or_else(|| text.strip_prefix("search_query: "))
        .unwrap_or(text);

    let mut v = vec![0f32; DIM];
    for token in content
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        // FNV-1a over the lowercased token.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in token.to_ascii_lowercase().bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        v[(h as usize) % DIM] += 1.0;
    }

    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// Answers Ollama `/api/embed`: `{model, input:[texts]}` -> `{embeddings:[vecs]}`,
/// computing one deterministic vector per input text.
struct EmbedResponder;

impl Respond for EmbedResponder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let embeddings: Vec<Vec<f32>> = body["input"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|t| embed_text(t.as_str().unwrap_or("")))
                    .collect()
            })
            .unwrap_or_default();
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "embeddings": embeddings }))
    }
}

/// Run a blocking `assert_cmd` command off the async reactor.
async fn run(cmd: Command) -> assert_cmd::assert::Assert {
    let mut cmd = cmd;
    tokio::task::spawn_blocking(move || cmd.assert())
        .await
        .expect("assert task joins")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embed_then_semantic_surfaces_myocardial_infarction() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(EmbedResponder)
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let ndjson = tmp.path().join("syn.ndjson");
    let arrow = tmp.path().join("syn.arrow");

    // 1. Build NDJSON from the committed RF2 fixture (no network).
    let mut c = Command::cargo_bin("sct").unwrap();
    c.args(["ndjson", "--rf2"])
        .arg(rf2_fixture())
        .args(["--locale", "en-GB", "--output"])
        .arg(&ndjson);
    run(c).await.success();

    // 2. Embed via the mocked Ollama endpoint -> Arrow file.
    let mut c = Command::cargo_bin("sct").unwrap();
    c.args(["embed", "--input"])
        .arg(&ndjson)
        .arg("--ollama-url")
        .arg(server.uri())
        .arg("--output")
        .arg(&arrow);
    run(c).await.success();
    assert!(arrow.exists(), "embeddings Arrow file should be written");

    // 3. Semantic search for a synonym-phrase; MI (22298006) must surface.
    let mut c = Command::cargo_bin("sct").unwrap();
    c.args(["semantic", "--embeddings"])
        .arg(&arrow)
        .arg("--ollama-url")
        .arg(server.uri())
        .args(["--ids", "--limit", "5", "heart attack"]);
    run(c)
        .await
        .success()
        .stdout(predicate::str::contains("22298006"));

    // 4. Batch stdin sends both queries in one Ollama request and emits one
    // structured document whose item order matches stdin.
    let mut c = Command::cargo_bin("sct").unwrap();
    c.args(["semantic", "--embeddings"])
        .arg(&arrow)
        .arg("--ollama-url")
        .arg(server.uri())
        .args(["--format", "json", "--limit", "3", "-"])
        .write_stdin("heart attack\ndiabetes\n");
    let output = tokio::task::spawn_blocking(move || c.output().unwrap())
        .await
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let items = value["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["input"], "heart attack");
    assert_eq!(items[0]["result"][0]["id"], "22298006");
    assert_eq!(items[1]["input"], "diabetes");

    let requests = server.received_requests().await.unwrap();
    let last: serde_json::Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    assert_eq!(last["input"].as_array().unwrap().len(), 2);
    assert_eq!(last["input"][0], "search_query: heart attack");
    assert_eq!(last["input"][1], "search_query: diabetes");

    // 5. The CLI rejects oversized batches before contacting Ollama.
    let request_count = requests.len();
    let mut c = Command::cargo_bin("sct").unwrap();
    c.args(["semantic", "--embeddings"])
        .arg(&arrow)
        .arg("--ollama-url")
        .arg(server.uri())
        .args(["--format", "json", "-"])
        .write_stdin(
            (0..=100)
                .map(|index| format!("query {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    run(c)
        .await
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "queries batch cannot exceed 100 entries",
        ));
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        request_count
    );

    // 6. A malformed multi-query Ollama response fails before structured
    // stdout is written.
    let malformed_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "embeddings": [vec![0.0f32; DIM]]
        })))
        .mount(&malformed_server)
        .await;
    let mut c = Command::cargo_bin("sct").unwrap();
    c.args(["semantic", "--embeddings"])
        .arg(&arrow)
        .arg("--ollama-url")
        .arg(malformed_server.uri())
        .args(["--format", "json", "-"])
        .write_stdin("heart attack\ndiabetes\n");
    run(c)
        .await
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "returned 1 embeddings for 2 queries",
        ));
}
