// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "cli")]

use assert_cmd::cargo::cargo_bin;
use rmcp::model::{
    CacheScope, CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation, JsonObject,
    ProtocolVersion,
};
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt, ServiceExt};
use sct_rs::commands::ndjson::{self, RefsetMode};
use sct_rs::commands::{sqlite, tct};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z")
}

fn build_db() -> (tempfile::TempDir, PathBuf) {
    build_db_with_tct(true)
}

fn build_db_with_tct(transitive_closure: bool) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let ndjson_path = directory.path().join("synthetic.ndjson");
    let db = directory.path().join("synthetic.db");
    ndjson::run(ndjson::Args {
        rf2_dirs: vec![fixture_dir()],
        locale: "en-GB".to_string(),
        output: Some(ndjson_path.clone()),
        include_inactive: false,
        refsets: RefsetMode::Simple,
    })
    .unwrap();
    sqlite::run(sqlite::Args {
        input: ndjson_path,
        output: Some(db.clone()),
        transitive_closure,
        include_self: false,
    })
    .unwrap();
    (directory, db)
}

fn tokio_mcp_command(db: &Path, codelist_root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(cargo_bin("sct"));
    command
        .arg("mcp")
        .arg("--db")
        .arg(db)
        .arg("--codelist-root")
        .arg(codelist_root);
    command
}

fn spawn_tokio_mcp(
    db: &Path,
    codelist_root: &Path,
) -> (
    tokio::process::Child,
    tokio::process::ChildStdout,
    tokio::process::ChildStdin,
) {
    let mut command = tokio_mcp_command(db, codelist_root);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    (child, stdout, stdin)
}

fn arguments(value: Value) -> JsonObject {
    value.as_object().cloned().expect("arguments are an object")
}

#[tokio::test]
async fn current_discovery_lists_typed_tools_and_enforces_codelist_root() {
    let (_database_directory, db) = build_db();
    let codelists = tempfile::tempdir().unwrap();
    let (mut child, stdout, stdin) = spawn_tokio_mcp(&db, codelists.path());
    let mut client = ()
        .serve_with_lifecycle(
            (stdout, stdin),
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap();

    let peer_info = client.peer_info().expect("server discovery information");
    assert_eq!(peer_info.protocol_version, ProtocolVersion::V_2026_07_28);

    let listed = client.list_tools(None).await.unwrap();
    assert!(listed.result_type.is_some());
    assert_eq!(listed.ttl_ms, Some(60_000));
    assert_eq!(listed.cache_scope, Some(CacheScope::Public));
    let search = listed
        .tools
        .iter()
        .find(|tool| tool.name == "snomed_search")
        .expect("search tool");
    assert!(search.output_schema.is_some());
    assert_eq!(
        search.output_schema.as_ref().unwrap()["properties"]["data"]["type"],
        "array"
    );
    assert!(
        search.output_schema.as_ref().unwrap()["properties"]["data"]["items"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("id"))
    );
    assert_eq!(
        search
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.read_only_hint),
        Some(true)
    );

    let search_result = client
        .call_tool(
            CallToolRequestParams::new("snomed_search")
                .with_arguments(arguments(json!({ "query": "heart" }))),
        )
        .await
        .unwrap();
    assert_eq!(search_result.is_error, Some(false));
    assert!(search_result.result_type.is_some());
    assert_eq!(
        search_result.structured_content.as_ref().unwrap()["data"][0]["id"],
        "22298006"
    );

    let ancestors = client
        .call_tool(
            CallToolRequestParams::new("snomed_ancestors")
                .with_arguments(arguments(json!({ "id": "22298006" }))),
        )
        .await
        .unwrap();
    assert!(ancestors.meta.is_none());

    let invalid_arguments = client
        .call_tool(
            CallToolRequestParams::new("snomed_search")
                .with_arguments(arguments(json!({ "query": 42 }))),
        )
        .await
        .unwrap();
    assert_eq!(invalid_arguments.is_error, Some(true));
    assert!(
        invalid_arguments.structured_content.as_ref().unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("invalid arguments")
    );

    let outside = codelists.path().parent().unwrap().join("outside.codelist");
    let escaped = client
        .call_tool(
            CallToolRequestParams::new("codelist_new").with_arguments(arguments(json!({
                "file": "../outside.codelist",
                "title": "Outside"
            }))),
        )
        .await
        .unwrap();
    assert_eq!(escaped.is_error, Some(true));
    assert!(!outside.exists());

    let created = client
        .call_tool(
            CallToolRequestParams::new("codelist_new").with_arguments(arguments(json!({
                "file": "nested/diabetes.codelist",
                "title": "Diabetes"
            }))),
        )
        .await
        .unwrap();
    assert_eq!(created.is_error, Some(false));
    assert!(codelists.path().join("nested/diabetes.codelist").is_file());

    let added = client
        .call_tool(
            CallToolRequestParams::new("codelist_add").with_arguments(arguments(json!({
                "file": "nested/diabetes.codelist",
                "sctids": ["46635009", "46635009"]
            }))),
        )
        .await
        .unwrap();
    assert_eq!(
        added.structured_content.as_ref().unwrap()["data"]["added"],
        1
    );

    assert!(client
        .call_tool(CallToolRequestParams::new("no_such_tool"))
        .await
        .is_err());
    client.close().await.unwrap();
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test]
async fn tct_fallback_diagnostic_tracks_live_database_status() {
    let (_database_directory, db) = build_db_with_tct(false);
    let codelists = tempfile::tempdir().unwrap();
    let (mut child, stdout, stdin) = spawn_tokio_mcp(&db, codelists.path());
    let mut client = ()
        .serve_with_lifecycle(
            (stdout, stdin),
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap();

    let ancestors = client
        .call_tool(
            CallToolRequestParams::new("snomed_ancestors")
                .with_arguments(arguments(json!({ "id": "22298006" }))),
        )
        .await
        .unwrap();
    assert_eq!(ancestors.is_error, Some(false));
    let diagnostics = &ancestors.meta.as_ref().unwrap().0["org.sct/diagnostics"];
    assert_eq!(diagnostics[0]["code"], "unusable-transitive-closure");
    assert!(diagnostics[0]["message"]
        .as_str()
        .unwrap()
        .contains("sct tct --db <db>"));

    tct::run(tct::Args {
        db: db.clone(),
        include_self: false,
    })
    .unwrap();
    let indexed = client
        .call_tool(
            CallToolRequestParams::new("snomed_ancestors")
                .with_arguments(arguments(json!({ "id": "22298006" }))),
        )
        .await
        .unwrap();
    assert!(indexed.meta.is_none());

    rusqlite::Connection::open(&db)
        .unwrap()
        .execute("DELETE FROM concept_ancestors_meta", [])
        .unwrap();
    let invalidated = client
        .call_tool(
            CallToolRequestParams::new("snomed_ancestors")
                .with_arguments(arguments(json!({ "id": "22298006" }))),
        )
        .await
        .unwrap();
    assert_eq!(
        invalidated.meta.as_ref().unwrap().0["org.sct/diagnostics"][0]["code"],
        "unusable-transitive-closure"
    );

    let children = client
        .call_tool(
            CallToolRequestParams::new("snomed_children")
                .with_arguments(arguments(json!({ "id": "73211009" }))),
        )
        .await
        .unwrap();
    assert!(children.meta.is_none());

    client.close().await.unwrap();
    assert!(child.wait().await.unwrap().success());
}

#[derive(Clone, Copy)]
struct LegacyClient;

impl ClientHandler for LegacyClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("sct-legacy-test", "1.0.0"),
        )
        .with_protocol_version(ProtocolVersion::V_2024_11_05)
    }
}

#[tokio::test]
async fn legacy_initialize_lifecycle_remains_supported() {
    let (_database_directory, db) = build_db();
    let codelists = tempfile::tempdir().unwrap();
    let (mut child, stdout, stdin) = spawn_tokio_mcp(&db, codelists.path());
    let mut client = LegacyClient.serve((stdout, stdin)).await.unwrap();

    assert_eq!(
        client.peer_info().unwrap().protocol_version,
        ProtocolVersion::V_2024_11_05
    );
    let listed = client.list_tools(None).await.unwrap();
    assert!(listed.result_type.is_none());
    assert!(listed
        .tools
        .iter()
        .any(|tool| tool.name == "snomed_concept"));

    let result = client
        .call_tool(
            CallToolRequestParams::new("snomed_concept")
                .with_arguments(arguments(json!({ "id": "22298006" }))),
        )
        .await
        .unwrap();
    assert!(result.result_type.is_none());
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result.structured_content.as_ref().unwrap()["data"]["preferred_term"],
        "Myocardial infarction"
    );
    client.close().await.unwrap();
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_codelist_mutations_do_not_lose_updates() {
    const SCTIDS: [&str; 8] = [
        "73211009",
        "46635009",
        "44054006",
        "22298006",
        "195967001",
        "74281007",
        "55641003",
        "80146002",
    ];

    let (_database_directory, db) = build_db();
    let codelists = tempfile::tempdir().unwrap();
    let (mut child, stdout, stdin) = spawn_tokio_mcp(&db, codelists.path());
    let mut client = ()
        .serve_with_lifecycle(
            (stdout, stdin),
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap();

    client
        .call_tool(
            CallToolRequestParams::new("codelist_new").with_arguments(arguments(json!({
                "file": "concurrent.codelist",
                "title": "Concurrent mutations"
            }))),
        )
        .await
        .unwrap();
    let added = client
        .call_tool(
            CallToolRequestParams::new("codelist_add").with_arguments(arguments(json!({
                "file": "concurrent.codelist",
                "sctids": SCTIDS
            }))),
        )
        .await
        .unwrap();
    assert_eq!(
        added.structured_content.as_ref().unwrap()["data"]["added"],
        SCTIDS.len()
    );

    let peer = client.peer().clone();
    let mut removals = tokio::task::JoinSet::new();
    for sctid in SCTIDS {
        let peer = peer.clone();
        removals.spawn(async move {
            peer.call_tool(
                CallToolRequestParams::new("codelist_remove").with_arguments(arguments(json!({
                    "file": "concurrent.codelist",
                    "sctid": sctid
                }))),
            )
            .await
        });
    }
    while let Some(result) = removals.join_next().await {
        let result = result.unwrap().unwrap();
        assert_eq!(result.is_error, Some(false));
    }

    let read = client
        .call_tool(
            CallToolRequestParams::new("codelist_read").with_arguments(arguments(json!({
                "file": "concurrent.codelist"
            }))),
        )
        .await
        .unwrap();
    let data = &read.structured_content.as_ref().unwrap()["data"];
    assert_eq!(data["active_concepts"].as_array().unwrap().len(), 0);
    assert_eq!(
        data["excluded_concepts"].as_array().unwrap().len(),
        SCTIDS.len()
    );
    assert_eq!(data["version"], 2 + SCTIDS.len());

    client.close().await.unwrap();
    assert!(child.wait().await.unwrap().success());
}

struct RawServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl RawServer {
    fn spawn(db: &Path, codelist_root: &Path) -> Self {
        let mut child = Command::new(cargo_bin("sct"))
            .arg("mcp")
            .arg("--db")
            .arg(db)
            .arg("--codelist-root")
            .arg(codelist_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send_raw(&mut self, message: &str) {
        self.stdin.write_all(message.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn send(&mut self, message: Value) {
        self.send_raw(&serde_json::to_string(&message).unwrap());
    }

    fn receive(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        if line.is_empty() {
            let status = self.child.wait().unwrap();
            let mut stderr = String::new();
            self.child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            panic!("server closed before returning a response ({status}): {stderr}");
        }
        serde_json::from_str(&line).unwrap()
    }

    fn finish(mut self) -> (std::process::ExitStatus, String) {
        drop(self.stdin);
        drop(self.stdout);
        let status = self.child.wait().unwrap();
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        (status, stderr)
    }
}

#[test]
fn malformed_envelopes_notifications_and_eof_have_jsonrpc_semantics() {
    let (_database_directory, db) = build_db();
    let codelists = tempfile::tempdir().unwrap();
    let mut server = RawServer::spawn(&db, codelists.path());

    server.send_raw("{not-json}");
    assert_eq!(server.receive()["error"]["code"], -32700);

    server.send(json!({ "jsonrpc": "2.0", "id": 7 }));
    let malformed = server.receive();
    assert_eq!(malformed["id"], 7);
    assert_eq!(malformed["error"]["code"], -32600);

    server.send(json!({ "jsonrpc": "2.0", "id": {}, "method": "ping" }));
    assert_eq!(server.receive()["error"]["code"], -32600);

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "raw-test", "version": "1.0.0" }
        }
    }));
    assert_eq!(server.receive()["result"]["serverInfo"]["name"], "sct-mcp");

    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    server.send(json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": "missing", "reason": "test" }
    }));
    server.send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let list = server.receive();
    assert_eq!(list["id"], 2);
    assert!(list["result"]["tools"].as_array().unwrap().len() >= 18);

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "no_such_tool", "arguments": {} }
    }));
    let unknown_tool = server.receive();
    assert_eq!(unknown_tool["id"], 3);
    assert_eq!(unknown_tool["error"]["code"], -32602);

    for (id, method) in [
        (4, "prompts/list"),
        (5, "resources/list"),
        (6, "resources/templates/list"),
    ] {
        server.send(json!({ "jsonrpc": "2.0", "id": id, "method": method }));
        let unsupported = server.receive();
        assert_eq!(unsupported["id"], id);
        assert_eq!(unsupported["error"]["code"], -32601);
    }
    server.send(json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "completion/complete",
        "params": {
            "ref": { "type": "ref/prompt", "name": "unused" },
            "argument": { "name": "unused", "value": "" }
        }
    }));
    let unsupported = server.receive();
    assert_eq!(unsupported["id"], 7);
    assert_eq!(unsupported["error"]["code"], -32601);

    let (status, stderr) = server.finish();
    assert!(status.success());
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unread_stdout_applies_end_to_end_backpressure() {
    let (_database_directory, db) = build_db();
    let codelists = tempfile::tempdir().unwrap();
    let (mut child, stdout, mut stdin) = spawn_tokio_mcp(&db, codelists.path());
    let mut stdout = tokio::io::BufReader::new(stdout);

    let initialize = format!(
        "{}\n",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "backpressure-test", "version": "1.0.0" }
            }
        })
    );
    stdin.write_all(initialize.as_bytes()).await.unwrap();
    stdin.flush().await.unwrap();
    let mut initialized = String::new();
    stdout.read_line(&mut initialized).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&initialized).unwrap()["id"],
        1
    );

    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let mut flood = tokio::spawn(async move {
        for id in 1_000..6_000 {
            let request =
                format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/list\"}}\n");
            stdin.write_all(request.as_bytes()).await?;
        }
        stdin.flush().await
    });

    if let Ok(result) = tokio::time::timeout(Duration::from_secs(1), &mut flood).await {
        let result = result.unwrap();
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        panic!("stdin flood completed without output backpressure: {result:?}");
    }

    flood.abort();
    let _ = flood.await;
    assert!(child.try_wait().unwrap().is_none());
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

fn run_rejected_input(db: &Path, codelist_root: &Path, input: &[u8]) -> String {
    let mut child = Command::new(cargo_bin("sct"))
        .arg("mcp")
        .arg("--db")
        .arg(db)
        .arg("--codelist-root")
        .arg(codelist_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn oversized_and_unterminated_messages_are_rejected_without_unbounded_buffering() {
    let (_database_directory, db) = build_db();
    let codelists = tempfile::tempdir().unwrap();

    let stderr = run_rejected_input(&db, codelists.path(), b"{\"jsonrpc\":\"2.0\"");
    assert!(stderr.contains("newline message delimiter"), "{stderr}");

    let oversized = vec![b'x'; 16 * 1024 * 1024 + 1];
    let stderr = run_rejected_input(&db, codelists.path(), &oversized);
    assert!(stderr.contains("exceeds maximum accepted size"), "{stderr}");
}
