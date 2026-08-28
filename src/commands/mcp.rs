// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct mcp` - Local MCP server over stdio backed by a SNOMED CT SQLite database.
//!
//! Transport: newline-delimited JSON-RPC 2.0 over stdio, with a 16 MiB message limit.
//! Protocol versions: legacy initialization through the current stateless protocol.
//!
//! Tools exposed:
//!   snomed_search          - FTS5 free-text search
//!   snomed_concept         - Full concept detail by SCTID
//!   snomed_children        - Immediate children of a concept
//!   snomed_ancestors       - Full ancestor chain to root
//!   snomed_hierarchy       - All concepts in a named top-level hierarchy
//!   snomed_map             - Cross-map between SNOMED CT and legacy UK terminologies
//!   snomed_refsets         - List loaded refsets with member counts
//!   snomed_refset_members  - List concepts in a refset
//!   snomed_refset_compare  - Membership diff between two refsets
//!   snomed_refset_profile  - Breakdown of a refset's members by top-level hierarchy
//!   snomed_semantic_search - Nearest-neighbour semantic search (optional; requires --embeddings)
//!
//! Claude Desktop config:
//!   {
//!     "mcpServers": {
//!       "snomed": {
//!         "command": "sct",
//!         "args": ["mcp", "--db", "/path/to/snomed.db"]
//!       }
//!     }
//!   }

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    DiscoverResult, ErrorCode, GetExtensions, Implementation, JsonObject, JsonRpcMessage,
    ListToolsResult, MetaObject, ProtocolVersion, RequestId, ServerCapabilities, ServerInfo, Tool,
    ToolAnnotations,
};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::{
    MaybeSendFuture, RequestContext, RoleServer, RxJsonRpcMessage, TxJsonRpcMessage,
};
use rmcp::transport::Transport;
use rmcp::{ErrorData, ServerHandler};
use rusqlite::{params, Connection};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::future::Future;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

use crate::codelist::write_new_codelist;
use crate::commands::codelist::{
    export_csv, export_markdown, export_opencodelists_csv, lookup_concept_row,
    lookup_hierarchy_and_children, read_codelist, today, write_codelist, CodelistFile, ConceptLine,
    FrontMatter, Warning,
};
use crate::commands::semantic;
use crate::provenance::{self, Provenance};

#[derive(Parser, Debug)]
pub struct Args {
    /// Path to the SNOMED CT SQLite database produced by `sct sqlite`.
    /// See `docs/path-resolution.md` for the discovery order when omitted.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// Arrow IPC embeddings file produced by `sct embed`. Only registers the
    /// `snomed_semantic_search` tool when supplied explicitly - it is not
    /// auto-discovered, since semantic search needs Ollama running.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub embeddings: Option<PathBuf>,

    /// Supported Ollama embedding model used by `snomed_semantic_search`:
    /// nomic-embed-text, nomic-embed-text:v1.5,
    /// nomic-embed-text-v2-moe, qwen3-embedding:0.6b, or embeddinggemma.
    #[arg(long, default_value = "nomic-embed-text")]
    pub model: String,

    /// Ollama API base URL (used by `snomed_semantic_search`).
    #[arg(long, default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// Directory that bounds every codelist path exposed through MCP. Relative
    /// tool paths are resolved beneath this root; traversal and symlinks are rejected.
    #[arg(long, default_value = ".", value_parser = crate::paths::tilde_pathbuf)]
    pub codelist_root: PathBuf,
}

/// Configuration for the optional semantic search tool.
#[derive(Debug)]
struct SemanticConfig {
    embeddings: PathBuf,
    model: String,
    ollama_url: String,
}

pub fn run(args: Args) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    let conn = crate::commands::open_db_readonly(&db, Some(32768))?;

    // Validate the database schema_version before serving.
    validate_schema_version(&conn)?;
    crate::ecl::eval::has_tct(&conn)?;

    // For embeddings: only consult the resolution chain when the user passed
    // --embeddings explicitly. We do not silently auto-discover an embeddings
    // file here - registering `snomed_semantic_search` requires Ollama, so
    // implicit activation could surprise users who haven't set that up.
    let semantic_cfg = if let Some(p) = args.embeddings {
        crate::commands::embedding_profile::resolve(&args.model)?;
        let path = crate::paths::resolve_embeddings(Some(&p))?.path;
        Some(SemanticConfig {
            embeddings: path,
            model: args.model,
            ollama_url: args.ollama_url,
        })
    } else {
        None
    };

    let codelist_root = CodelistRoot::new(&args.codelist_root)?;

    // Read provenance once at startup so we can advertise it on every
    // initialize handshake and inject it into per-concept tool responses.
    let prov = provenance::read_sqlite(&conn).unwrap_or(None);
    let server = SctMcp::new(conn, semantic_cfg, prov, codelist_root);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("creating MCP runtime")?;

    runtime.block_on(async move {
        let service = rmcp::serve_server(server, BoundedStdioTransport::stdio())
            .await
            .context("starting MCP server")?;
        service.waiting().await.context("running MCP server")?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Schema version validation
// ---------------------------------------------------------------------------

fn validate_schema_version(conn: &Connection) -> Result<()> {
    if let crate::sdk::SchemaCompatibility::Older {
        database,
        supported,
    } = crate::sdk::query_schema_compatibility(conn)?
    {
        eprintln!(
            "sct mcp: database schema_version {} is older than this binary expects ({}).\n\
             Consider regenerating with `sct ndjson` + `sct sqlite`.",
            database, supported
        );
    }
    Ok(())
}

const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;
const MAX_IN_FLIGHT_REQUESTS: usize = 8;
const CATALOG_TTL_MS: u64 = 60_000;

#[derive(Clone)]
struct InFlightRequestGuard {
    _inner: Arc<InFlightRequestGuardInner>,
}

#[derive(Clone)]
struct InFlightNotificationGuard {
    _permit: Arc<OwnedSemaphorePermit>,
}

struct InFlightRequestGuardInner {
    registry: Arc<InFlightRegistry>,
    id: RequestId,
}

impl Drop for InFlightRequestGuardInner {
    fn drop(&mut self) {
        self.registry.handler_finished(&self.id);
    }
}

struct InFlightRequest {
    _permit: OwnedSemaphorePermit,
    handler_finished: bool,
    cancelled: bool,
    response_started: bool,
}

struct InFlightRegistry {
    slots: Arc<Semaphore>,
    requests: Mutex<std::collections::HashMap<RequestId, InFlightRequest>>,
}

#[derive(Debug)]
enum StartRequestError {
    Closed,
    Duplicate,
    Full,
}

impl InFlightRegistry {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
            requests: Mutex::new(std::collections::HashMap::new()),
        })
    }

    fn start(
        self: &Arc<Self>,
        id: RequestId,
    ) -> std::result::Result<InFlightRequestGuard, StartRequestError> {
        let permit = match self.slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(TryAcquireError::Closed) => return Err(StartRequestError::Closed),
            Err(TryAcquireError::NoPermits) => return Err(StartRequestError::Full),
        };
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(|lock| lock.into_inner());
        if requests.contains_key(&id) {
            return Err(StartRequestError::Duplicate);
        }
        requests.insert(
            id.clone(),
            InFlightRequest {
                _permit: permit,
                handler_finished: false,
                cancelled: false,
                response_started: false,
            },
        );
        Ok(InFlightRequestGuard {
            _inner: Arc::new(InFlightRequestGuardInner {
                registry: self.clone(),
                id,
            }),
        })
    }

    fn handler_finished(&self, id: &RequestId) {
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(|lock| lock.into_inner());
        if let Some(request) = requests.get_mut(id) {
            request.handler_finished = true;
            if request.cancelled && !request.response_started {
                requests.remove(id);
            }
        }
    }

    fn cancel(&self, id: &RequestId) {
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(|lock| lock.into_inner());
        if let Some(request) = requests.get_mut(id) {
            request.cancelled = true;
            if request.handler_finished && !request.response_started {
                requests.remove(id);
            }
        }
    }

    fn response_started(&self, id: &RequestId) {
        let mut requests = self
            .requests
            .lock()
            .unwrap_or_else(|lock| lock.into_inner());
        if let Some(request) = requests.get_mut(id) {
            request.response_started = true;
        }
    }

    fn response_finished(&self, id: &RequestId) {
        self.requests
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .remove(id);
    }
}

struct BoundedStdioTransport<R, W> {
    reader: BufReader<R>,
    line: Vec<u8>,
    writer: Arc<tokio::sync::Mutex<Option<W>>>,
    in_flight: Arc<InFlightRegistry>,
    notification_slots: Arc<Semaphore>,
}

impl BoundedStdioTransport<tokio::io::Stdin, tokio::io::Stdout> {
    fn stdio() -> Self {
        Self::new(tokio::io::stdin(), tokio::io::stdout())
    }
}

impl<R, W> BoundedStdioTransport<R, W>
where
    R: AsyncRead + Unpin,
{
    fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            line: Vec::new(),
            writer: Arc::new(tokio::sync::Mutex::new(Some(writer))),
            in_flight: InFlightRegistry::new(),
            notification_slots: Arc::new(Semaphore::new(MAX_IN_FLIGHT_REQUESTS)),
        }
    }
}

impl<R, W> Transport<RoleServer> for BoundedStdioTransport<R, W>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let writer = self.writer.clone();
        let in_flight = self.in_flight.clone();
        let response_id = match &item {
            JsonRpcMessage::Response(response) => Some(response.id.clone()),
            JsonRpcMessage::Error(error) => error.id.clone(),
            JsonRpcMessage::Request(_) | JsonRpcMessage::Notification(_) => None,
        };
        if let Some(id) = response_id.as_ref() {
            in_flight.response_started(id);
        }
        async move {
            let result = write_json_line(&writer, item).await;
            if let Some(id) = response_id.as_ref() {
                in_flight.response_finished(id);
            }
            result
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            match read_bounded_line(&mut self.reader, &mut self.line).await {
                Ok(false) => return None,
                Ok(true) => {}
                Err(error) => {
                    eprintln!("sct mcp: {error}");
                    return None;
                }
            }

            let value = {
                let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
                if line.is_empty() {
                    self.line.clear();
                    continue;
                }
                serde_json::from_slice::<Value>(line)
            };
            self.line.clear();

            let value = match value {
                Ok(value) => value,
                Err(error) => {
                    let writer = self.writer.clone();
                    let message = TxJsonRpcMessage::<RoleServer>::error(
                        ErrorData::parse_error(format!("Parse error: {error}"), None),
                        None,
                    );
                    if write_json_line(&writer, message).await.is_err() {
                        return None;
                    }
                    continue;
                }
            };
            let request_id = jsonrpc_request_id(&value);
            let cancelled_request_id = jsonrpc_cancelled_request_id(&value);

            if let Err(message) = validate_jsonrpc_envelope(&value) {
                let writer = self.writer.clone();
                let message = TxJsonRpcMessage::<RoleServer>::error(
                    ErrorData::invalid_request(message, None),
                    request_id,
                );
                if write_json_line(&writer, message).await.is_err() {
                    return None;
                }
                continue;
            }

            match serde_json::from_value::<RxJsonRpcMessage<RoleServer>>(value) {
                Ok(mut message) => {
                    if let JsonRpcMessage::Request(request) = &mut message {
                        match self.in_flight.start(request.id.clone()) {
                            Ok(guard) => {
                                request.request.extensions_mut().insert(guard);
                            }
                            Err(StartRequestError::Duplicate) => {
                                let writer = self.writer.clone();
                                let message = TxJsonRpcMessage::<RoleServer>::error(
                                    ErrorData::invalid_request(
                                        "Duplicate in-flight request id",
                                        None,
                                    ),
                                    Some(request.id.clone()),
                                );
                                if write_json_line(&writer, message).await.is_err() {
                                    return None;
                                }
                                continue;
                            }
                            Err(StartRequestError::Full) => {
                                let writer = self.writer.clone();
                                let message = TxJsonRpcMessage::<RoleServer>::error(
                                    ErrorData::new(
                                        ErrorCode(-32000),
                                        "Too many in-flight requests",
                                        None,
                                    ),
                                    Some(request.id.clone()),
                                );
                                if write_json_line(&writer, message).await.is_err() {
                                    return None;
                                }
                                continue;
                            }
                            Err(StartRequestError::Closed) => return None,
                        }
                    } else if let JsonRpcMessage::Notification(notification) = &mut message {
                        let Ok(permit) = self.notification_slots.clone().try_acquire_owned() else {
                            continue;
                        };
                        notification.notification.extensions_mut().insert(
                            InFlightNotificationGuard {
                                _permit: Arc::new(permit),
                            },
                        );
                        if let Some(id) = cancelled_request_id.as_ref() {
                            self.in_flight.cancel(id);
                        }
                    }
                    return Some(message);
                }
                Err(_) => {
                    let writer = self.writer.clone();
                    let message = TxJsonRpcMessage::<RoleServer>::error(
                        ErrorData::invalid_request("Invalid request", None),
                        request_id,
                    );
                    if write_json_line(&writer, message).await.is_err() {
                        return None;
                    }
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.writer.lock().await.take();
        Ok(())
    }
}

fn jsonrpc_request_id(value: &Value) -> Option<RequestId> {
    jsonrpc_id_value(value.get("id")?)
}

fn jsonrpc_cancelled_request_id(value: &Value) -> Option<RequestId> {
    if value.get("method").and_then(Value::as_str) != Some("notifications/cancelled") {
        return None;
    }
    jsonrpc_id_value(value.get("params")?.get("requestId")?)
}

fn jsonrpc_id_value(value: &Value) -> Option<RequestId> {
    match value {
        Value::String(id) => Some(RequestId::String(Arc::from(id.as_str()))),
        Value::Number(id) => id.as_i64().map(RequestId::Number),
        _ => None,
    }
}

fn validate_jsonrpc_envelope(value: &Value) -> std::result::Result<(), &'static str> {
    let object = value
        .as_object()
        .ok_or("Invalid request: JSON-RPC message must be an object")?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("Invalid request: jsonrpc must be \"2.0\"");
    }
    if let Some(id) = object.get("id") {
        let valid_id = id.is_string() || id.as_i64().is_some();
        if !valid_id {
            return Err("Invalid request: id must be a string or integer");
        }
    }
    if let Some(method) = object.get("method") {
        if !method.is_string() {
            return Err("Invalid request: method must be a string");
        }
    } else if !object.contains_key("result") && !object.contains_key("error") {
        return Err("Invalid request: message has no method, result, or error");
    }
    Ok(())
}

async fn read_bounded_line<R>(reader: &mut R, line: &mut Vec<u8>) -> io::Result<bool>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        let (consumed, complete) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                if line.is_empty() {
                    return Ok(false);
                }
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "input ended before the newline message delimiter",
                ));
            }

            let newline = available.iter().position(|byte| *byte == b'\n');
            let content_len = newline.unwrap_or(available.len());
            if line.len().saturating_add(content_len) > MAX_MESSAGE_SIZE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("input line exceeds maximum accepted size ({MAX_MESSAGE_SIZE} bytes)"),
                ));
            }
            line.extend_from_slice(&available[..content_len]);
            (
                content_len + usize::from(newline.is_some()),
                newline.is_some(),
            )
        };
        reader.consume(consumed);
        if complete {
            return Ok(true);
        }
    }
}

async fn write_json_line<W>(
    writer: &Arc<tokio::sync::Mutex<Option<W>>>,
    message: TxJsonRpcMessage<RoleServer>,
) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut writer = writer.lock().await;
    let writer = writer
        .as_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "transport is closed"))?;
    let bytes = serde_json::to_vec(&message).map_err(io::Error::other)?;
    writer.write_all(&bytes).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

#[derive(Debug)]
struct CodelistRoot {
    root: PathBuf,
}

impl CodelistRoot {
    fn new(root: &Path) -> Result<Self> {
        let root = std::fs::canonicalize(root)
            .with_context(|| format!("resolving codelist root {}", root.display()))?;
        anyhow::ensure!(
            root.is_dir(),
            "codelist root is not a directory: {}",
            root.display()
        );
        Ok(Self { root })
    }

    fn resolve_existing_file(&self, raw: &str) -> Result<PathBuf> {
        let path = self.resolve_existing(raw)?;
        anyhow::ensure!(path.is_file(), "codelist path is not a file: {raw}");
        anyhow::ensure!(
            path.extension().and_then(|extension| extension.to_str()) == Some("codelist"),
            "codelist file must use the .codelist extension: {raw}"
        );
        Ok(path)
    }

    fn resolve_existing_directory(&self, raw: &str) -> Result<PathBuf> {
        let path = self.resolve_existing(raw)?;
        anyhow::ensure!(path.is_dir(), "codelist directory not found: {raw}");
        Ok(path)
    }

    fn resolve_existing(&self, raw: &str) -> Result<PathBuf> {
        let candidate = self.candidate(raw)?;
        self.ensure_no_symlinks(&candidate)?;
        let canonical = std::fs::canonicalize(&candidate)
            .with_context(|| format!("resolving codelist path {raw}"))?;
        anyhow::ensure!(
            canonical.starts_with(&self.root),
            "codelist path escapes configured root: {raw}"
        );
        Ok(canonical)
    }

    fn resolve_new_file(&self, raw: &str) -> Result<PathBuf> {
        let candidate = self.candidate(raw)?;
        anyhow::ensure!(
            candidate
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("codelist"),
            "codelist file must use the .codelist extension: {raw}"
        );
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => anyhow::bail!("{} already exists", self.display(&candidate)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("checking {}", candidate.display()))
            }
        }
        self.ensure_no_symlinks(&candidate)?;

        let parent = candidate
            .parent()
            .context("codelist path has no parent directory")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating codelist directory {}", parent.display()))?;
        self.ensure_no_symlinks(parent)?;
        let canonical_parent = std::fs::canonicalize(parent)
            .with_context(|| format!("resolving codelist directory {}", parent.display()))?;
        anyhow::ensure!(
            canonical_parent.starts_with(&self.root),
            "codelist path escapes configured root: {raw}"
        );
        Ok(canonical_parent.join(
            candidate
                .file_name()
                .context("codelist path must include a file name")?,
        ))
    }

    fn candidate(&self, raw: &str) -> Result<PathBuf> {
        anyhow::ensure!(!raw.trim().is_empty(), "codelist path must not be empty");
        let raw_path = Path::new(raw);
        let relative = if raw_path.is_absolute() {
            raw_path.strip_prefix(&self.root).with_context(|| {
                format!("absolute codelist path is outside configured root: {raw}")
            })?
        } else {
            raw_path
        };

        let mut clean = PathBuf::new();
        for component in relative.components() {
            match component {
                Component::Normal(part) => clean.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    anyhow::ensure!(clean.pop(), "codelist path escapes configured root: {raw}");
                }
                Component::RootDir | Component::Prefix(_) => {
                    anyhow::bail!("invalid codelist path: {raw}")
                }
            }
        }
        Ok(self.root.join(clean))
    }

    fn ensure_no_symlinks(&self, path: &Path) -> Result<()> {
        let relative = path
            .strip_prefix(&self.root)
            .context("codelist path is outside configured root")?;
        let mut current = self.root.clone();
        for component in relative.components() {
            current.push(component.as_os_str());
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) => anyhow::ensure!(
                    !metadata.file_type().is_symlink(),
                    "codelist paths may not traverse symlinks: {}",
                    self.display(&current)
                ),
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(error).with_context(|| format!("checking {}", current.display()))
                }
            }
        }
        Ok(())
    }

    fn display(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .display()
            .to_string()
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    /// Search terms (words or phrases).
    query: String,
    /// Maximum number of results (default 10, max 100).
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConceptInput {
    /// SNOMED CT concept identifier (SCTID).
    id: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ChildrenInput {
    /// SNOMED CT concept identifier (SCTID).
    id: String,
    /// Maximum number of children (default 50, max 500).
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HierarchyInput {
    /// Top-level hierarchy name.
    hierarchy: String,
    /// Maximum number of results (default 100, max 1000).
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LimitInput {
    /// Maximum number of results.
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefsetInput {
    /// SCTID of the reference set.
    refset_id: String,
    /// Maximum number of members (default 200, max 5000).
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefsetProfileInput {
    /// SCTID of the reference set.
    refset_id: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefsetCompareInput {
    /// SCTID of the first reference set.
    refset_id_a: String,
    /// SCTID of the second reference set.
    refset_id_b: String,
    /// Maximum members returned per set (default 200, max 5000).
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MapInput {
    /// Code to map.
    code: String,
    /// Source terminology: snomed, ctv3, read2, icd10, or opcs4.
    terminology: TerminologyInput,
    /// Optional target terminology.
    to: Option<TerminologyInput>,
    /// Forward inactive SNOMED concepts through replacement associations.
    #[serde(default)]
    forward_history: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum TerminologyInput {
    Snomed,
    Ctv3,
    Read2,
    Icd10,
    Opcs4,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CodelistListInput {
    /// Directory beneath the configured codelist root (default `.`).
    directory: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CodelistFileInput {
    /// `.codelist` path beneath the configured codelist root.
    file: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CodelistNewInput {
    /// Path for the new `.codelist` file.
    file: String,
    /// Human-readable title.
    title: String,
    /// What this codelist is for.
    description: Option<String>,
    /// Terminology name (default SNOMED CT).
    terminology: Option<String>,
    /// Author name.
    author: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CodelistAddInput {
    /// `.codelist` path beneath the configured root.
    file: String,
    /// SCTIDs to add.
    #[schemars(length(min = 1))]
    sctids: Vec<String>,
    /// Optional annotation for added lines.
    comment: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CodelistRemoveInput {
    /// `.codelist` path beneath the configured root.
    file: String,
    /// SCTID to exclude.
    sctid: String,
    /// Reason for exclusion.
    comment: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CodelistExportInput {
    /// `.codelist` path beneath the configured root.
    file: String,
    /// Export format: csv, opencodelists-csv, or markdown.
    format: Option<CodelistExportFormat>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum CodelistExportFormat {
    Csv,
    OpencodelistsCsv,
    Markdown,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SemanticSearchInput {
    /// Natural-language search query.
    query: String,
    /// Maximum number of results (default 10, max 100).
    limit: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct StructuredToolResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone)]
struct SctMcp {
    conn: Arc<Mutex<Connection>>,
    codelist_mutations: Arc<Mutex<()>>,
    semantic_cfg: Option<Arc<SemanticConfig>>,
    provenance: Option<Arc<Provenance>>,
    codelist_root: Arc<CodelistRoot>,
    tools: Arc<Vec<Tool>>,
}

impl SctMcp {
    fn new(
        conn: Connection,
        semantic_cfg: Option<SemanticConfig>,
        provenance: Option<Provenance>,
        codelist_root: CodelistRoot,
    ) -> Self {
        let tools = tool_definitions(semantic_cfg.is_some());
        Self {
            conn: Arc::new(Mutex::new(conn)),
            codelist_mutations: Arc::new(Mutex::new(())),
            semantic_cfg: semantic_cfg.map(Arc::new),
            provenance: provenance.map(Arc::new),
            codelist_root: Arc::new(codelist_root),
            tools: Arc::new(tools),
        }
    }

    fn call_tool_sync(
        &self,
        request: CallToolRequestParams,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.as_ref();
        if !self.tools.iter().any(|tool| tool.name == name) {
            return Err(ErrorData::invalid_params(
                format!("Unknown tool: {name}"),
                None,
            ));
        }
        if is_cancelled() {
            return Ok(failed_tool_result(anyhow::anyhow!("Tool call cancelled")).into());
        }
        let arguments = request.arguments.unwrap_or_default();
        let mut used_tct_fallback = false;
        let result = match name {
            "snomed_semantic_search" => {
                run_typed::<SemanticSearchInput>(&arguments, name, |args| {
                    tool_semantic_search(args, self.semantic_cfg.as_deref())
                })
            }
            "codelist_list" => run_typed::<CodelistListInput>(&arguments, name, |args| {
                tool_codelist_list(args, &self.codelist_root)
            }),
            "codelist_read" => run_typed::<CodelistFileInput>(&arguments, name, |args| {
                tool_codelist_read(args, &self.codelist_root)
            }),
            "codelist_new" => {
                let _guard = self.codelist_mutations.lock().map_err(|_| {
                    ErrorData::internal_error("Codelist mutation lock is unavailable", None)
                })?;
                run_unless_cancelled(&is_cancelled, || {
                    run_typed::<CodelistNewInput>(&arguments, name, |args| {
                        tool_codelist_new(args, &self.codelist_root)
                    })
                })
            }
            "codelist_add" => {
                let _guard = self.codelist_mutations.lock().map_err(|_| {
                    ErrorData::internal_error("Codelist mutation lock is unavailable", None)
                })?;
                let conn = self.conn.lock().map_err(|_| {
                    ErrorData::internal_error("SNOMED database lock is unavailable", None)
                })?;
                run_unless_cancelled(&is_cancelled, || {
                    run_typed::<CodelistAddInput>(&arguments, name, |args| {
                        tool_codelist_add(&conn, args, &self.codelist_root)
                    })
                })
            }
            "codelist_remove" => {
                let _guard = self.codelist_mutations.lock().map_err(|_| {
                    ErrorData::internal_error("Codelist mutation lock is unavailable", None)
                })?;
                run_unless_cancelled(&is_cancelled, || {
                    run_typed::<CodelistRemoveInput>(&arguments, name, |args| {
                        tool_codelist_remove(args, &self.codelist_root)
                    })
                })
            }
            "codelist_export" => run_typed::<CodelistExportInput>(&arguments, name, |args| {
                tool_codelist_export(args, &self.codelist_root)
            }),
            "snomed_ancestors" => {
                let conn = self.conn.lock().map_err(|_| {
                    ErrorData::internal_error("SNOMED database lock is unavailable", None)
                })?;
                run_unless_cancelled(&is_cancelled, || {
                    run_typed_with_tct_status::<ConceptInput>(&arguments, name, |args| {
                        tool_ancestors_with_tct_status(&conn, args)
                    })
                    .map(|(text, tct)| {
                        used_tct_fallback = !tct;
                        text
                    })
                })
            }
            _ => {
                let conn = self.conn.lock().map_err(|_| {
                    ErrorData::internal_error("SNOMED database lock is unavailable", None)
                })?;
                run_unless_cancelled(&is_cancelled, || {
                    call_database_tool(
                        &conn,
                        name,
                        &arguments,
                        self.provenance.as_deref(),
                        &self.codelist_root,
                    )
                })
            }
        };

        Ok(match result {
            Ok(text) => {
                let mut result = successful_tool_result(text);
                if used_tct_fallback {
                    add_unusable_tct_diagnostic(&mut result);
                }
                result.into()
            }
            Err(error) => failed_tool_result(error).into(),
        })
    }
}

fn call_database_tool(
    conn: &Connection,
    name: &str,
    arguments: &JsonObject,
    provenance: Option<&Provenance>,
    codelist_root: &CodelistRoot,
) -> Result<String> {
    match name {
        "snomed_search" => {
            run_typed::<SearchInput>(arguments, name, |args| tool_search(conn, args))
        }
        "snomed_concept" => {
            run_typed::<ConceptInput>(arguments, name, |args| tool_concept(conn, args, provenance))
        }
        "snomed_children" => {
            run_typed::<ChildrenInput>(arguments, name, |args| tool_children(conn, args))
        }
        "snomed_ancestors" => {
            run_typed::<ConceptInput>(arguments, name, |args| tool_ancestors(conn, args))
        }
        "snomed_hierarchy" => {
            run_typed::<HierarchyInput>(arguments, name, |args| tool_hierarchy(conn, args))
        }
        "snomed_map" => run_typed::<MapInput>(arguments, name, |args| tool_map(conn, args)),
        "snomed_refsets" => {
            run_typed::<LimitInput>(arguments, name, |args| tool_refsets(conn, args))
        }
        "snomed_refset_members" => {
            run_typed::<RefsetInput>(arguments, name, |args| tool_refset_members(conn, args))
        }
        "snomed_refset_compare" => {
            run_typed::<RefsetCompareInput>(arguments, name, |args| tool_refset_compare(conn, args))
        }
        "snomed_refset_profile" => {
            run_typed::<RefsetProfileInput>(arguments, name, |args| tool_refset_profile(conn, args))
        }
        "codelist_validate" => run_typed::<CodelistFileInput>(arguments, name, |args| {
            tool_codelist_validate(conn, args, codelist_root)
        }),
        "codelist_stats" => run_typed::<CodelistFileInput>(arguments, name, |args| {
            tool_codelist_stats(conn, args, codelist_root)
        }),
        _ => anyhow::bail!("tool registry and database dispatch do not match: {name}"),
    }
}

impl ServerHandler for SctMcp {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
    }

    fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<DiscoverResult, ErrorData>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        )
        .with_ttl_ms(CATALOG_TTL_MS)
        .with_cache_scope(CacheScope::Public)))
    }

    fn complete(
        &self,
        _request: rmcp::model::CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::CompleteResult, ErrorData>> + MaybeSendFuture + '_
    {
        std::future::ready(Err(ErrorData::method_not_found::<
            rmcp::model::CompleteRequestMethod,
        >()))
    }

    fn list_prompts(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListPromptsResult, ErrorData>> + MaybeSendFuture + '_
    {
        std::future::ready(Err(ErrorData::method_not_found::<
            rmcp::model::ListPromptsRequestMethod,
        >()))
    }

    fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListResourcesResult, ErrorData>> + MaybeSendFuture + '_
    {
        std::future::ready(Err(ErrorData::method_not_found::<
            rmcp::model::ListResourcesRequestMethod,
        >()))
    }

    fn list_resource_templates(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListResourceTemplatesResult, ErrorData>>
           + MaybeSendFuture
           + '_ {
        std::future::ready(Err(ErrorData::method_not_found::<
            rmcp::model::ListResourceTemplatesRequestMethod,
        >()))
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + MaybeSendFuture + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(
            self.tools.as_ref().clone(),
        )
        .with_ttl_ms(CATALOG_TTL_MS)
        .with_cache_scope(CacheScope::Public)))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, ErrorData>> + MaybeSendFuture + '_ {
        let server = self.clone();
        async move {
            if context.ct.is_cancelled() {
                return Ok(failed_tool_result(anyhow::anyhow!("Tool call cancelled")).into());
            }
            let cancellation = context.ct.clone();
            let task = tokio::task::spawn_blocking(move || {
                server.call_tool_sync(request, || cancellation.is_cancelled())
            });
            // Blocking work cannot be aborted safely. Keep this handler alive until it
            // finishes; rmcp suppresses the response if cancellation arrived meanwhile.
            let result = task.await.map_err(|error| {
                ErrorData::internal_error(format!("MCP tool worker failed: {error}"), None)
            })?;
            drop(context);
            result
        }
    }

    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder().enable_tools().build();
        let implementation = Implementation::new("sct-mcp", env!("CARGO_PKG_VERSION"))
            .with_title("sct SNOMED CT MCP server")
            .with_description("Local-first SNOMED CT terminology and codelist tools")
            .with_website_url("https://github.com/pacharanero/sct");
        let mut info = ServerInfo::new(capabilities)
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
            .with_server_info(implementation)
            .with_instructions(format!(
                "SNOMED CT database access is read-only. Codelist paths are restricted to {}.",
                self.codelist_root.root.display()
            ));
        if let Some(provenance) = self.provenance.as_ref() {
            let mut meta = MetaObject::new();
            meta.0
                .insert("org.sct/provenance".to_string(), provenance.to_json_value());
            info.meta = Some(meta);
        }
        info
    }
}

fn run_unless_cancelled<T>(
    is_cancelled: &impl Fn() -> bool,
    run: impl FnOnce() -> Result<T>,
) -> Result<T> {
    anyhow::ensure!(!is_cancelled(), "Tool call cancelled");
    run()
}

fn run_typed<T>(
    arguments: &JsonObject,
    tool_name: &str,
    run: impl FnOnce(&Value) -> Result<String>,
) -> Result<String>
where
    T: DeserializeOwned + Serialize,
{
    let input: T = serde_json::from_value(Value::Object(arguments.clone()))
        .with_context(|| format!("invalid arguments for {tool_name}"))?;
    run(&serde_json::to_value(input)?)
}

fn run_typed_with_tct_status<T>(
    arguments: &JsonObject,
    tool_name: &str,
    run: impl FnOnce(&Value) -> Result<(String, bool)>,
) -> Result<(String, bool)>
where
    T: DeserializeOwned + Serialize,
{
    let input: T = serde_json::from_value(Value::Object(arguments.clone()))
        .with_context(|| format!("invalid arguments for {tool_name}"))?;
    run(&serde_json::to_value(input)?)
}

fn successful_tool_result(text: String) -> CallToolResult {
    let structured = match serde_json::from_str(&text) {
        Ok(data) => StructuredToolResult {
            data: Some(data),
            message: None,
            error: None,
        },
        Err(_) => StructuredToolResult {
            data: None,
            message: Some(text.clone()),
            error: None,
        },
    };
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = serde_json::to_value(structured).ok();
    result
}

fn add_unusable_tct_diagnostic(result: &mut CallToolResult) {
    let mut meta = result.meta.take().unwrap_or_default();
    meta.0.insert(
        "org.sct/diagnostics".to_string(),
        json!([{
            "code": "unusable-transitive-closure",
            "level": "warning",
            "message": crate::ecl::tct_fallback_guidance("this ancestor query"),
        }]),
    );
    result.meta = Some(meta);
}

fn failed_tool_result(error: anyhow::Error) -> CallToolResult {
    let message = format!("{error:#}");
    let structured = StructuredToolResult {
        data: None,
        message: None,
        error: Some(message.clone()),
    };
    let mut result = CallToolResult::error(vec![ContentBlock::text(message)]);
    result.structured_content = serde_json::to_value(structured).ok();
    result
}

fn read_only_annotations(title: &str) -> ToolAnnotations {
    ToolAnnotations::with_title(title)
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(false)
}

fn open_world_read_only_annotations(title: &str) -> ToolAnnotations {
    read_only_annotations(title).open_world(true)
}

fn mutating_annotations(title: &str, destructive: bool, idempotent: bool) -> ToolAnnotations {
    ToolAnnotations::with_title(title)
        .read_only(false)
        .destructive(destructive)
        .idempotent(idempotent)
        .open_world(false)
}

#[derive(Clone, Copy)]
enum OutputShape {
    SearchResults,
    Concept,
    ConceptSummaries,
    Mapping,
    Refsets,
    RefsetMembers,
    RefsetComparison,
    HierarchyCounts,
    SemanticResults,
    CodelistList,
    Codelist,
    CodelistAdd,
    CodelistValidation,
    CodelistStats,
    Message,
}

fn structured_output_schema(shape: OutputShape) -> Arc<JsonObject> {
    let schema = json!({
        "type": "object",
        "properties": {
            "data": data_schema(shape),
            "message": { "type": "string" },
            "error": { "type": "string" }
        },
        "additionalProperties": false
    });
    Arc::new(
        schema
            .as_object()
            .expect("output schema is an object")
            .clone(),
    )
}

fn data_schema(shape: OutputShape) -> Value {
    let concept_summary = || {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "preferred_term": { "type": "string" },
                "fsn": { "type": "string" },
                "active": { "type": "boolean" }
            },
            "required": ["id", "preferred_term", "fsn", "active"],
            "additionalProperties": false
        })
    };
    let refset_summary = || {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "preferred_term": { "type": "string" },
                "fsn": { "type": "string" },
                "module": { "type": "string" },
                "member_count": { "type": "integer" }
            },
            "required": ["id", "preferred_term", "fsn", "module", "member_count"],
            "additionalProperties": false
        })
    };
    let refset_member = || {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "preferred_term": { "type": "string" },
                "fsn": { "type": "string" },
                "hierarchy": { "type": "string" },
                "effective_time": { "type": "string" },
                "active": { "type": "boolean" }
            },
            "required": ["id", "preferred_term", "fsn", "hierarchy", "effective_time", "active"],
            "additionalProperties": false
        })
    };
    let codelist_concept = || {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "term": { "type": "string" },
                "comment": { "type": ["string", "null"] }
            },
            "required": ["id", "term", "comment"],
            "additionalProperties": false
        })
    };

    match shape {
        OutputShape::SearchResults => json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "preferred_term": { "type": "string" },
                    "fsn": { "type": "string" },
                    "hierarchy": { "type": "string" },
                    "active": { "type": "boolean" }
                },
                "required": ["id", "preferred_term", "fsn", "hierarchy", "active"],
                "additionalProperties": false
            }
        }),
        OutputShape::Concept => json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "fsn": { "type": "string" },
                "preferred_term": { "type": "string" },
                "synonyms": { "type": "array", "items": { "type": "string" } },
                "hierarchy": { "type": "string" },
                "hierarchy_path": { "type": "array", "items": { "type": "string" } },
                "parents": { "type": "array", "items": { "type": "object" } },
                "children_count": { "type": "integer", "minimum": 0 },
                "attributes": { "type": "object" },
                "active": { "type": "boolean" },
                "definition_status": { "type": "string" },
                "module": { "type": "string" },
                "effective_time": { "type": "string" },
                "ctv3_codes": { "type": "array", "items": { "type": "string" } },
                "read2_codes": { "type": "array", "items": { "type": "string" } },
                "member_of": { "type": "array", "items": { "type": "object" } },
                // Null for an active concept, and for an inactive one whose
                // release does not record a reason (or a database built before
                // payload refsets were ingested).
                "inactivation_reason": { "type": ["object", "null"] },
                // What to use instead of an inactive concept. Empty for an
                // active one.
                "historical_associations": { "type": "array", "items": { "type": "object" } },
                "_provenance": { "type": ["object", "null"] }
            },
            "required": ["id", "fsn", "preferred_term", "synonyms", "hierarchy", "hierarchy_path", "parents", "children_count", "attributes", "active", "definition_status", "module", "effective_time", "ctv3_codes", "read2_codes", "member_of", "inactivation_reason", "historical_associations"],
            "additionalProperties": false
        }),
        OutputShape::ConceptSummaries => {
            json!({ "type": "array", "items": concept_summary() })
        }
        OutputShape::Mapping => json!({
            "type": "object",
            "properties": {
                "code": { "type": "string" },
                "terminology": { "type": "string" },
                "from": { "type": "string" },
                "to": { "type": "string" },
                "mapped": { "type": "array", "items": { "type": "object" } },
                "snomed_id": { "type": "string" },
                "snomed_concepts": { "type": "array", "items": { "type": "object" } },
                "ctv3_codes": { "type": "array", "items": { "type": "string" } },
                "read2_codes": { "type": "array", "items": { "type": "string" } },
                "icd10_codes": { "type": "array", "items": { "type": "string" } },
                "opcs4_codes": { "type": "array", "items": { "type": "string" } }
            },
            "additionalProperties": false
        }),
        OutputShape::Refsets => json!({ "type": "array", "items": refset_summary() }),
        OutputShape::RefsetMembers => json!({ "type": "array", "items": refset_member() }),
        OutputShape::RefsetComparison => json!({
            "type": "object",
            "properties": {
                "refset_a": refset_summary(),
                "refset_b": refset_summary(),
                "only_in_a": { "type": "object" },
                "only_in_b": { "type": "object" },
                "in_both": { "type": "object" }
            },
            "required": ["refset_a", "refset_b", "only_in_a", "only_in_b", "in_both"],
            "additionalProperties": false
        }),
        OutputShape::HierarchyCounts => json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "hierarchy": { "type": "string" },
                    "count": { "type": "integer" }
                },
                "required": ["hierarchy", "count"],
                "additionalProperties": false
            }
        }),
        OutputShape::SemanticResults => json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "preferred_term": { "type": "string" },
                    "similarity": { "type": "number" }
                },
                "required": ["id", "preferred_term", "similarity"],
                "additionalProperties": false
            }
        }),
        OutputShape::CodelistList => json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "status": { "type": "string" },
                    "version": { "type": "integer" },
                    "active_concepts": { "type": "integer" },
                    "updated": { "type": "string" },
                    "error": { "type": "string" }
                },
                "required": ["file"],
                "additionalProperties": false
            }
        }),
        OutputShape::Codelist => json!({
            "type": "object",
            "properties": {
                "file": { "type": "string" },
                "id": { "type": "string" },
                "title": { "type": "string" },
                "description": { "type": "string" },
                "terminology": { "type": "string" },
                "status": { "type": "string" },
                "version": { "type": "integer" },
                "updated": { "type": "string" },
                "snomed_release": { "type": ["string", "null"] },
                "active_concepts": { "type": "array", "items": codelist_concept() },
                "excluded_concepts": { "type": "array", "items": codelist_concept() },
                "pending_review": { "type": "array", "items": { "type": "object" } }
            },
            "required": ["file", "id", "title", "description", "terminology", "status", "version", "updated", "active_concepts", "excluded_concepts", "pending_review"],
            "additionalProperties": false
        }),
        OutputShape::CodelistAdd => json!({
            "type": "object",
            "properties": {
                "file": { "type": "string" },
                "added": { "type": "integer", "minimum": 0 },
                "not_found": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["file", "added"],
            "additionalProperties": false
        }),
        OutputShape::CodelistValidation => json!({
            "type": "object",
            "properties": {
                "file": { "type": "string" },
                "active_concepts": { "type": "integer" },
                "warnings": { "type": "array", "items": { "type": "string" } },
                "errors": { "type": "array", "items": { "type": "string" } },
                "valid": { "type": "boolean" }
            },
            "required": ["file", "active_concepts", "warnings", "errors", "valid"],
            "additionalProperties": false
        }),
        OutputShape::CodelistStats => json!({
            "type": "object",
            "properties": {
                "file": { "type": "string" },
                "title": { "type": "string" },
                "terminology": { "type": "string" },
                "status": { "type": "string" },
                "version": { "type": "integer" },
                "updated": { "type": "string" },
                "snomed_release": { "type": ["string", "null"] },
                "active_concepts": { "type": "integer" },
                "excluded_concepts": { "type": "integer" },
                "pending_review": { "type": "integer" },
                "by_hierarchy": { "type": "array", "items": { "type": "object" } },
                "leaf_nodes": { "type": "integer" },
                "intermediate_nodes": { "type": "integer" }
            },
            "required": ["file", "title", "terminology", "status", "version", "updated", "active_concepts", "excluded_concepts", "pending_review", "by_hierarchy", "leaf_nodes", "intermediate_nodes"],
            "additionalProperties": false
        }),
        OutputShape::Message => Value::Bool(false),
    }
}

fn tool_definition<T>(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    output_shape: OutputShape,
    annotations: ToolAnnotations,
) -> Tool
where
    T: JsonSchema + 'static,
{
    Tool::new(name, description, JsonObject::new())
        .with_title(title)
        .with_input_schema::<T>()
        .with_raw_output_schema(structured_output_schema(output_shape))
        .with_annotations(annotations)
}

fn tool_definitions(semantic_search: bool) -> Vec<Tool> {
    let mut tools = vec![
        tool_definition::<SearchInput>(
            "snomed_search",
            "Search SNOMED CT",
            "Free-text FTS5 search returning concept identifiers, preferred terms, FSNs, and hierarchies.",
            OutputShape::SearchResults,
            read_only_annotations("Search SNOMED CT"),
        ),
        tool_definition::<ConceptInput>(
            "snomed_concept",
            "Get SNOMED CT concept",
            "Retrieve full detail for one SNOMED CT concept by SCTID.",
            OutputShape::Concept,
            read_only_annotations("Get SNOMED CT concept"),
        ),
        tool_definition::<ChildrenInput>(
            "snomed_children",
            "List concept children",
            "List the immediate IS-A children of a SNOMED CT concept.",
            OutputShape::ConceptSummaries,
            read_only_annotations("List concept children"),
        ),
        tool_definition::<ConceptInput>(
            "snomed_ancestors",
            "List concept ancestors",
            "Return the ancestor chain from a concept to the SNOMED CT root.",
            OutputShape::ConceptSummaries,
            read_only_annotations("List concept ancestors"),
        ),
        tool_definition::<HierarchyInput>(
            "snomed_hierarchy",
            "List hierarchy concepts",
            "List concepts in a named top-level SNOMED CT hierarchy.",
            OutputShape::ConceptSummaries,
            read_only_annotations("List hierarchy concepts"),
        ),
        tool_definition::<MapInput>(
            "snomed_map",
            "Map terminology codes",
            "Cross-map between SNOMED CT, CTV3, Read v2, ICD-10, and OPCS-4 using locally loaded mappings.",
            OutputShape::Mapping,
            read_only_annotations("Map terminology codes"),
        ),
        tool_definition::<LimitInput>(
            "snomed_refsets",
            "List reference sets",
            "List loaded SNOMED CT reference sets with preferred terms and member counts.",
            OutputShape::Refsets,
            read_only_annotations("List reference sets"),
        ),
        tool_definition::<RefsetInput>(
            "snomed_refset_members",
            "List reference set members",
            "List concepts belonging to a SNOMED CT reference set.",
            OutputShape::RefsetMembers,
            read_only_annotations("List reference set members"),
        ),
        tool_definition::<RefsetCompareInput>(
            "snomed_refset_compare",
            "Compare reference sets",
            "Compare two reference sets, returning only-in-A, only-in-B, and shared members with exact counts.",
            OutputShape::RefsetComparison,
            read_only_annotations("Compare reference sets"),
        ),
        tool_definition::<RefsetProfileInput>(
            "snomed_refset_profile",
            "Profile reference set",
            "Profile a reference set's members by top-level SNOMED CT hierarchy.",
            OutputShape::HierarchyCounts,
            read_only_annotations("Profile reference set"),
        ),
    ];
    if semantic_search {
        tools.push(tool_definition::<SemanticSearchInput>(
            "snomed_semantic_search",
            "Semantic SNOMED CT search",
            "Nearest-neighbour search over local SNOMED CT embeddings using the configured Ollama endpoint.",
            OutputShape::SemanticResults,
            open_world_read_only_annotations("Semantic SNOMED CT search"),
        ));
    }
    tools.extend([
        tool_definition::<CodelistListInput>(
            "codelist_list",
            "List codelists",
            "List .codelist files beneath the configured codelist root.",
            OutputShape::CodelistList,
            read_only_annotations("List codelists"),
        ),
        tool_definition::<CodelistFileInput>(
            "codelist_read",
            "Read codelist",
            "Read codelist metadata and active, excluded, and pending concepts.",
            OutputShape::Codelist,
            read_only_annotations("Read codelist"),
        ),
        tool_definition::<CodelistNewInput>(
            "codelist_new",
            "Create codelist",
            "Create a new .codelist file beneath the configured codelist root.",
            OutputShape::Message,
            mutating_annotations("Create codelist", false, false),
        ),
        tool_definition::<CodelistAddInput>(
            "codelist_add",
            "Add codelist concepts",
            "Add SNOMED CT concepts to a codelist, resolving terms and deduplicating existing active entries.",
            OutputShape::CodelistAdd,
            mutating_annotations("Add codelist concepts", false, true),
        ),
        tool_definition::<CodelistRemoveInput>(
            "codelist_remove",
            "Exclude codelist concept",
            "Move an active concept to the codelist's explicit exclusions.",
            OutputShape::Message,
            mutating_annotations("Exclude codelist concept", true, false),
        ),
        tool_definition::<CodelistFileInput>(
            "codelist_validate",
            "Validate codelist",
            "Validate a codelist against the local SNOMED CT database.",
            OutputShape::CodelistValidation,
            read_only_annotations("Validate codelist"),
        ),
        tool_definition::<CodelistFileInput>(
            "codelist_stats",
            "Summarise codelist",
            "Return codelist counts, hierarchy breakdown, and leaf/intermediate statistics.",
            OutputShape::CodelistStats,
            read_only_annotations("Summarise codelist"),
        ),
        tool_definition::<CodelistExportInput>(
            "codelist_export",
            "Export codelist",
            "Render a codelist as CSV, OpenCodelists CSV, or Markdown without writing another file.",
            OutputShape::Message,
            read_only_annotations("Export codelist"),
        ),
    ]);
    tools
}

/// Call one MCP tool directly for domain-level integration tests. Protocol tests
/// spawn the real server process instead.
#[doc(hidden)]
pub fn call_tool_for_test(conn: &Connection, name: &str, arguments: Value) -> Result<Value> {
    let root_dir = tempfile::tempdir()?;
    let root = CodelistRoot::new(root_dir.path())?;
    let arguments = arguments
        .as_object()
        .cloned()
        .context("tool arguments must be a JSON object")?;
    let text = match name {
        "codelist_list" => {
            run_typed::<CodelistListInput>(&arguments, name, |args| tool_codelist_list(args, &root))
        }
        _ => call_database_tool(conn, name, &arguments, None, &root),
    }?;
    Ok(serde_json::from_str(&text).unwrap_or_else(|_| json!({ "message": text })))
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

fn tool_search(conn: &Connection, args: &Value) -> Result<String> {
    let query = args["query"]
        .as_str()
        .context("snomed_search requires query")?;
    let limit = args["limit"].as_u64().unwrap_or(10).min(100) as u32;
    let rows =
        crate::sdk::query_search(conn, crate::sdk::SearchOptions::new(query, limit).literal())?;

    if rows.is_empty() {
        return Ok(format!("No results found for query: {}", query));
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

fn tool_concept(conn: &Connection, args: &Value, prov: Option<&Provenance>) -> Result<String> {
    let id = args["id"].as_str().context("snomed_concept requires id")?;

    match crate::sdk::query_concept(conn, id)? {
        Some(concept) => {
            let mut v = serde_json::to_value(concept)?;
            // Always cite the source release in MCP responses - LLM clients
            // benefit from being able to ground answers in a specific edition.
            provenance::inject_into_json(&mut v, prov, true);
            Ok(serde_json::to_string_pretty(&v)?)
        }
        None => Ok(format!("Concept {} not found", id)),
    }
}

fn tool_children(conn: &Connection, args: &Value) -> Result<String> {
    let id = args["id"].as_str().context("snomed_children requires id")?;
    let limit = args["limit"].as_u64().unwrap_or(50).min(500) as u32;
    let rows = crate::sdk::query_direct(conn, id, false, limit)?;

    if rows.is_empty() {
        return Ok(format!("No children found for concept {}", id));
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

fn tool_ancestors(conn: &Connection, args: &Value) -> Result<String> {
    Ok(tool_ancestors_with_tct_status(conn, args)?.0)
}

fn tool_ancestors_with_tct_status(conn: &Connection, args: &Value) -> Result<(String, bool)> {
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn)?;
    let id = args["id"]
        .as_str()
        .context("snomed_ancestors requires id")?;

    let tct = crate::ecl::eval::has_tct(conn)?;
    let rows = crate::sdk::query_ancestors_with_tct(conn, id, tct)?;

    if rows.is_empty() {
        return Ok((format!("No ancestors found for concept {}", id), tct));
    }

    Ok((serde_json::to_string_pretty(&rows)?, tct))
}

fn tool_hierarchy(conn: &Connection, args: &Value) -> Result<String> {
    let hierarchy = args["hierarchy"]
        .as_str()
        .context("snomed_hierarchy requires hierarchy")?;
    let limit = args["limit"].as_u64().unwrap_or(100).min(1000) as usize;

    let mut stmt = conn.prepare(
        "SELECT id, preferred_term, fsn
         FROM concepts
         WHERE hierarchy = ?1
         ORDER BY preferred_term
         LIMIT ?2",
    )?;

    let rows: Vec<Value> = stmt
        .query_map(params![hierarchy, limit as i64], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "preferred_term": row.get::<_, String>(1)?,
                "fsn": row.get::<_, String>(2)?
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if rows.is_empty() {
        return Ok(format!("No concepts found in hierarchy: {}", hierarchy));
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

fn tool_map(conn: &Connection, args: &Value) -> Result<String> {
    let code = args["code"].as_str().context("snomed_map requires code")?;
    let terminology: crate::sdk::Terminology = args["terminology"]
        .as_str()
        .context("snomed_map requires terminology")?
        .parse()?;
    let target = args["to"]
        .as_str()
        .map(str::parse::<crate::sdk::Terminology>)
        .transpose()?;
    let forward_history = args["forward_history"].as_bool().unwrap_or(false);

    if let Some(target) = target {
        let mappings = crate::sdk::query_map(conn, terminology, code, target, forward_history)?;
        if mappings.is_empty() {
            return Ok(format!(
                "No {} mapping found for {} code '{}'.",
                target.as_str().to_uppercase(),
                terminology.as_str().to_uppercase(),
                code
            ));
        }

        if target == crate::sdk::Terminology::Snomed {
            let mut concepts = Vec::new();
            for mapping in mappings {
                if let Some(concept) = crate::sdk::query_concept(conn, &mapping.target)? {
                    concepts.push(serde_json::to_value(concept)?);
                }
            }
            return Ok(serde_json::to_string_pretty(&json!({
                "code": code,
                "terminology": terminology,
                "snomed_concepts": concepts
            }))?);
        }

        return Ok(serde_json::to_string_pretty(&json!({
            "code": code,
            "from": terminology,
            "to": target,
            "mapped": mappings
        }))?);
    }

    match terminology {
        crate::sdk::Terminology::Snomed => {
            let targets = |target| {
                crate::sdk::query_map(conn, terminology, code, target, forward_history).map(
                    |rows| {
                        rows.into_iter()
                            .map(|mapping| mapping.target)
                            .collect::<Vec<_>>()
                    },
                )
            };
            let ctv3_codes = targets(crate::sdk::Terminology::Ctv3)?;
            let read2_codes = targets(crate::sdk::Terminology::Read2)?;
            let icd10_codes = targets(crate::sdk::Terminology::Icd10)?;
            let opcs4_codes = targets(crate::sdk::Terminology::Opcs4)?;

            if ctv3_codes.is_empty()
                && read2_codes.is_empty()
                && icd10_codes.is_empty()
                && opcs4_codes.is_empty()
            {
                return Ok(format!(
                    "No mappings found for SNOMED CT concept {} in this database.",
                    code
                ));
            }

            Ok(serde_json::to_string_pretty(&json!({
                "snomed_id": code,
                "ctv3_codes": ctv3_codes,
                "read2_codes": read2_codes,
                "icd10_codes": icd10_codes,
                "opcs4_codes": opcs4_codes
            }))?)
        }

        source => {
            let mappings = crate::sdk::query_map(
                conn,
                source,
                code,
                crate::sdk::Terminology::Snomed,
                forward_history,
            )?;
            let mut rows = Vec::new();
            for mapping in mappings {
                if let Some(concept) = crate::sdk::query_concept(conn, &mapping.target)? {
                    rows.push(serde_json::to_value(concept)?);
                }
            }

            if rows.is_empty() {
                return Ok(format!(
                    "No SNOMED CT mapping found for {} code '{}'. \
                     Mapping data may not be loaded in this database.",
                    source.as_str().to_uppercase(),
                    code
                ));
            }

            Ok(serde_json::to_string_pretty(&json!({
                "code": code,
                "terminology": source,
                "snomed_concepts": rows
            }))?)
        }
    }
}

fn tool_refsets(conn: &Connection, args: &Value) -> Result<String> {
    let limit = args["limit"].as_u64().unwrap_or(200).min(5000) as u32;
    let rows = crate::sdk::query_refsets(conn, Some(limit))?;

    if rows.is_empty() {
        return Ok("No refset members loaded. Rebuild with `sct ndjson --refsets simple` from an RF2 release that includes Simple refset files.".to_string());
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

fn tool_refset_members(conn: &Connection, args: &Value) -> Result<String> {
    let refset_id = args["refset_id"]
        .as_str()
        .context("snomed_refset_members requires refset_id")?;
    let limit = args["limit"].as_u64().unwrap_or(200).min(5000) as u32;

    let rows = crate::sdk::query_refset_members(conn, refset_id, Some(limit))?;

    if rows.is_empty() {
        return Ok(format!(
            "No members found for refset {}. It may not be a refset, or its members were not loaded.",
            refset_id
        ));
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

fn tool_refset_compare(conn: &Connection, args: &Value) -> Result<String> {
    let refset_id_a = args["refset_id_a"]
        .as_str()
        .context("snomed_refset_compare requires refset_id_a")?;
    let refset_id_b = args["refset_id_b"]
        .as_str()
        .context("snomed_refset_compare requires refset_id_b")?;
    let limit = args["limit"].as_u64().unwrap_or(200).min(5000) as u32;

    let cmp = crate::sdk::query_refset_compare(conn, refset_id_a, refset_id_b, Some(limit))?;
    Ok(serde_json::to_string_pretty(&cmp)?)
}

fn tool_refset_profile(conn: &Connection, args: &Value) -> Result<String> {
    let refset_id = args["refset_id"]
        .as_str()
        .context("snomed_refset_profile requires refset_id")?;

    let rows = crate::sdk::query_refset_profile(conn, refset_id)?;

    if rows.is_empty() {
        return Ok(format!(
            "No members found for refset {}. It may not be a refset, or its members were not loaded.",
            refset_id
        ));
    }

    Ok(serde_json::to_string_pretty(&rows)?)
}

// ---------------------------------------------------------------------------
// Codelist tool implementations
// ---------------------------------------------------------------------------

fn cl_path(args: &Value, root: &CodelistRoot) -> Result<PathBuf> {
    let s = args["file"].as_str().context("requires file")?;
    root.resolve_existing_file(s)
}

fn tool_codelist_list(args: &Value, root: &CodelistRoot) -> Result<String> {
    let dir = args["directory"].as_str().unwrap_or(".");
    let base = root.resolve_existing_directory(dir)?;

    let mut entries: Vec<Value> = Vec::new();
    for entry in walkdir::WalkDir::new(&base).follow_links(false).into_iter() {
        let entry = entry.with_context(|| format!("walking {}", root.display(&base)))?;
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|x| x.to_str()) != Some("codelist")
        {
            continue;
        }
        let path = entry.path();
        let item = match read_codelist(path) {
            Ok(cl) => {
                let active = cl
                    .body
                    .iter()
                    .filter(|l| matches!(l, ConceptLine::Active { .. }))
                    .count();
                json!({
                    "file": root.display(path),
                    "id": cl.front_matter.id,
                    "title": cl.front_matter.title,
                    "status": cl.front_matter.status,
                    "version": cl.front_matter.version,
                    "active_concepts": active,
                    "updated": cl.front_matter.updated,
                })
            }
            Err(e) => json!({ "file": root.display(path), "error": e.to_string() }),
        };
        entries.push(item);
    }
    entries.sort_by(|left, right| left["file"].as_str().cmp(&right["file"].as_str()));

    if entries.is_empty() {
        return Ok(format!(
            "No .codelist files found in {}",
            root.display(&base)
        ));
    }
    Ok(serde_json::to_string_pretty(&entries)?)
}

fn tool_codelist_read(args: &Value, root: &CodelistRoot) -> Result<String> {
    let path = cl_path(args, root)?;
    let cl = read_codelist(&path)?;
    let fm = &cl.front_matter;

    let active: Vec<Value> = cl
        .body
        .iter()
        .filter_map(|l| {
            if let ConceptLine::Active { id, term, comment } = l {
                Some(json!({ "id": id, "term": term, "comment": comment }))
            } else {
                None
            }
        })
        .collect();

    let excluded: Vec<Value> = cl
        .body
        .iter()
        .filter_map(|l| {
            if let ConceptLine::Excluded { id, term, comment } = l {
                Some(json!({ "id": id, "term": term, "comment": comment }))
            } else {
                None
            }
        })
        .collect();

    let pending: Vec<Value> = cl
        .body
        .iter()
        .filter_map(|l| {
            if let ConceptLine::PendingReview { id, term } = l {
                Some(json!({ "id": id, "term": term }))
            } else {
                None
            }
        })
        .collect();

    Ok(serde_json::to_string_pretty(&json!({
        "file": root.display(&path),
        "id": fm.id,
        "title": fm.title,
        "description": fm.description,
        "terminology": fm.terminology,
        "status": fm.status,
        "version": fm.version,
        "updated": fm.updated,
        "snomed_release": fm.snomed_release,
        "active_concepts": active,
        "excluded_concepts": excluded,
        "pending_review": pending,
    }))?)
}

fn tool_codelist_new(args: &Value, root: &CodelistRoot) -> Result<String> {
    let raw_path = args["file"]
        .as_str()
        .context("codelist_new requires file")?;
    let path = root.resolve_new_file(raw_path)?;

    let title = args["title"]
        .as_str()
        .context("codelist_new requires title")?
        .to_string();
    let terminology = args["terminology"]
        .as_str()
        .unwrap_or("SNOMED CT")
        .to_string();
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_lowercase()
        .replace(' ', "-");
    let today = today();

    let fm = FrontMatter {
        id,
        description: args["description"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| format!("{title} codes")),
        title,
        terminology: terminology.clone(),
        created: today.clone(),
        updated: today,
        version: 1,
        status: "draft".to_string(),
        licence: "CC-BY-4.0".to_string(),
        copyright: "Copyright holder. SNOMED CT content © IHTSDO.".to_string(),
        appropriate_use: "Describe appropriate use here.".to_string(),
        misuse: "Describe misuse here.".to_string(),
        includes: None,
        snomed_release: None,
        authors: args["author"].as_str().map(|name| {
            vec![crate::commands::codelist::Author {
                name: name.to_string(),
                orcid: None,
                affiliation: None,
                role: Some("author".to_string()),
            }]
        }),
        organisation: None,
        methodology: None,
        signoffs: None,
        warnings: Some(vec![
            Warning {
                code: "not-universal-definition".to_string(),
                severity: "info".to_string(),
                message: "Developed for a specific purpose - may not suit all uses.".to_string(),
            },
            Warning {
                code: "draft-not-reviewed".to_string(),
                severity: "info".to_string(),
                message: "Not yet reviewed. Check status before use.".to_string(),
            },
        ]),
        population: None,
        care_setting: None,
        tags: None,
        opencodelists_id: None,
        opencodelists_url: None,
        canonical_url: None,
    };

    let cl = CodelistFile {
        front_matter: fm,
        body: vec![
            ConceptLine::Blank,
            ConceptLine::Comment("# concepts".to_string()),
            ConceptLine::Blank,
        ],
    };
    write_new_codelist(&cl, &path)?;
    Ok(format!("Created {}", root.display(&path)))
}

fn tool_codelist_add(conn: &Connection, args: &Value, root: &CodelistRoot) -> Result<String> {
    let path = cl_path(args, root)?;
    let sctids: Vec<String> = args["sctids"]
        .as_array()
        .context("codelist_add requires sctids array")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(String::from)
                .context("codelist_add sctids must contain only strings")
        })
        .collect::<Result<_>>()?;
    if sctids.is_empty() {
        anyhow::bail!("sctids array is empty");
    }
    let comment = args["comment"].as_str().map(String::from);

    let mut cl = read_codelist(&path)?;
    let mut existing: std::collections::HashSet<String> = cl
        .body
        .iter()
        .filter_map(|l| {
            if matches!(l, ConceptLine::Active { .. }) {
                l.sctid().map(String::from)
            } else {
                None
            }
        })
        .collect();

    let mut added = 0usize;
    let mut not_found: Vec<String> = Vec::new();
    for id in &sctids {
        if !existing.insert(id.clone()) {
            continue;
        }
        match lookup_concept_row(conn, id)? {
            Some((term, true)) => {
                cl.body.push(ConceptLine::Active {
                    id: id.clone(),
                    term,
                    comment: comment.clone(),
                });
                added += 1;
            }
            Some((_, false)) | None => not_found.push(id.clone()),
        }
    }

    if added > 0 {
        cl.front_matter.updated = today();
        cl.front_matter.version += 1;
        write_codelist(&cl, &path)?;
    }

    let mut result = json!({ "added": added, "file": root.display(&path) });
    if !not_found.is_empty() {
        result["not_found"] = json!(not_found);
    }
    Ok(serde_json::to_string_pretty(&result)?)
}

fn tool_codelist_remove(args: &Value, root: &CodelistRoot) -> Result<String> {
    let path = cl_path(args, root)?;
    let sctid = args["sctid"]
        .as_str()
        .context("codelist_remove requires sctid")?;
    let comment = args["comment"].as_str().map(String::from);

    let mut cl = read_codelist(&path)?;
    let mut found = false;
    for line in &mut cl.body {
        if let ConceptLine::Active { id, term, .. } = line {
            if id == sctid {
                *line = ConceptLine::Excluded {
                    id: id.clone(),
                    term: term.clone(),
                    comment,
                };
                found = true;
                break;
            }
        }
    }
    if !found {
        anyhow::bail!(
            "SCTID {} not found as an active concept in {}",
            sctid,
            path.display()
        );
    }
    cl.front_matter.updated = today();
    cl.front_matter.version += 1;
    write_codelist(&cl, &path)?;
    Ok(format!(
        "Moved {} to excluded in {}",
        sctid,
        root.display(&path)
    ))
}

fn tool_codelist_validate(conn: &Connection, args: &Value, root: &CodelistRoot) -> Result<String> {
    let path = cl_path(args, root)?;
    let cl = read_codelist(&path)?;
    let fm = &cl.front_matter;

    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for (field, val) in [
        ("appropriate_use", fm.appropriate_use.as_str()),
        ("misuse", fm.misuse.as_str()),
    ] {
        if val.trim().is_empty() || val.starts_with("Describe") {
            if fm.status == "published" {
                errors.push(format!(
                    "`{field}` must be filled in for published codelists"
                ));
            } else {
                warnings.push(format!("`{field}` is a placeholder"));
            }
        }
    }
    if fm.status == "published" && fm.signoffs.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
        errors.push("published codelist requires at least one signoff".to_string());
    }

    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for line in &cl.body {
        if let Some(id) = line.sctid() {
            *seen.entry(id).or_insert(0) += 1;
        }
    }
    for (id, count) in &seen {
        if *count > 1 {
            errors.push(format!("SCTID {id} appears {count} times"));
        }
    }

    for line in &cl.body {
        match line {
            ConceptLine::Active { id, term, .. } => match lookup_concept_row(conn, id)? {
                None => errors.push(format!("{id}: not found in database")),
                Some((db_term, false)) => {
                    errors.push(format!("{id}: inactive in database ({db_term})"))
                }
                Some((db_term, true)) if db_term != *term => warnings.push(format!(
                    "{id}: stored term {term:?} differs from database {db_term:?}"
                )),
                _ => {}
            },
            ConceptLine::PendingReview { id, term } => {
                warnings.push(format!("{id} ({term}): pending review"))
            }
            _ => {}
        }
    }

    let active_count = cl
        .body
        .iter()
        .filter(|l| matches!(l, ConceptLine::Active { .. }))
        .count();
    Ok(serde_json::to_string_pretty(&json!({
        "file": root.display(&path),
        "active_concepts": active_count,
        "warnings": warnings,
        "errors": errors,
        "valid": errors.is_empty(),
    }))?)
}

fn tool_codelist_stats(conn: &Connection, args: &Value, root: &CodelistRoot) -> Result<String> {
    let path = cl_path(args, root)?;
    let cl = read_codelist(&path)?;
    let fm = &cl.front_matter;

    let active: Vec<&str> = cl
        .body
        .iter()
        .filter_map(|l| {
            if matches!(l, ConceptLine::Active { .. }) {
                l.sctid()
            } else {
                None
            }
        })
        .collect();
    let excluded_count = cl
        .body
        .iter()
        .filter(|l| matches!(l, ConceptLine::Excluded { .. }))
        .count();
    let pending_count = cl
        .body
        .iter()
        .filter(|l| matches!(l, ConceptLine::PendingReview { .. }))
        .count();

    let mut by_hierarchy: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut leaf_count = 0usize;
    let mut intermediate_count = 0usize;
    for id in &active {
        if let Some((hierarchy, children_count)) = lookup_hierarchy_and_children(conn, id)? {
            *by_hierarchy.entry(hierarchy).or_insert(0) += 1;
            if children_count == 0 {
                leaf_count += 1;
            } else {
                intermediate_count += 1;
            }
        }
    }

    let mut hierarchy_list: Vec<Value> = by_hierarchy
        .into_iter()
        .map(|(h, n)| json!({"hierarchy": h, "count": n}))
        .collect();
    hierarchy_list.sort_by(|a, b| b["count"].as_u64().cmp(&a["count"].as_u64()));

    Ok(serde_json::to_string_pretty(&json!({
        "file": root.display(&path),
        "title": fm.title,
        "terminology": fm.terminology,
        "status": fm.status,
        "version": fm.version,
        "updated": fm.updated,
        "snomed_release": fm.snomed_release,
        "active_concepts": active.len(),
        "excluded_concepts": excluded_count,
        "pending_review": pending_count,
        "by_hierarchy": hierarchy_list,
        "leaf_nodes": leaf_count,
        "intermediate_nodes": intermediate_count,
    }))?)
}

fn tool_codelist_export(args: &Value, root: &CodelistRoot) -> Result<String> {
    let path = cl_path(args, root)?;
    let cl = read_codelist(&path)?;
    let active: Vec<(&str, &str)> = cl
        .body
        .iter()
        .filter_map(|l| {
            if let ConceptLine::Active { id, term, .. } = l {
                Some((id.as_str(), term.as_str()))
            } else {
                None
            }
        })
        .collect();

    match args["format"].as_str().unwrap_or("csv") {
        "csv" => Ok(export_csv(&active)),
        "opencodelists-csv" => Ok(export_opencodelists_csv(&active)),
        "markdown" => Ok(export_markdown(&cl.front_matter, &active)),
        other => {
            anyhow::bail!("unsupported format: {other}. Use csv, opencodelists-csv, or markdown")
        }
    }
}

fn tool_semantic_search(args: &Value, semantic_cfg: Option<&SemanticConfig>) -> Result<String> {
    let cfg = semantic_cfg.context(
        "snomed_semantic_search is not available: start sct mcp with --embeddings <file>",
    )?;
    let query = args["query"]
        .as_str()
        .context("snomed_semantic_search requires query")?;
    let limit = args["limit"].as_u64().unwrap_or(10).min(100) as usize;

    let results =
        semantic::semantic_search(&cfg.embeddings, &cfg.ollama_url, &cfg.model, query, limit)?;

    let rows: Vec<Value> = results
        .iter()
        .map(|r| json!({ "id": r.id, "preferred_term": r.preferred_term, "similarity": r.score }))
        .collect();

    Ok(serde_json::to_string_pretty(&rows)?)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::time::Instant;

    #[test]
    fn every_tool_has_a_complete_typed_contract() {
        let tools = tool_definitions(true);
        let mut names = std::collections::HashSet::new();
        assert_eq!(tools.len(), 19);

        for tool in &tools {
            assert!(
                names.insert(tool.name.as_ref()),
                "duplicate tool {}",
                tool.name
            );
            assert_eq!(tool.input_schema["type"], "object", "{} input", tool.name);
            let output = tool.output_schema.as_ref().expect("output schema");
            assert_eq!(output["type"], "object", "{} output", tool.name);
            assert!(
                output["properties"].get("data").is_some(),
                "{} data",
                tool.name
            );
            assert!(
                output["properties"].get("error").is_some(),
                "{} error",
                tool.name
            );

            let annotations = tool.annotations.as_ref().expect("tool annotations");
            assert!(
                annotations.read_only_hint.is_some(),
                "{} read-only",
                tool.name
            );
            assert!(
                annotations.destructive_hint.is_some(),
                "{} destructive",
                tool.name
            );
            assert!(
                annotations.idempotent_hint.is_some(),
                "{} idempotent",
                tool.name
            );
            assert!(
                annotations.open_world_hint.is_some(),
                "{} open-world",
                tool.name
            );
        }

        let semantic = tools
            .iter()
            .find(|tool| tool.name == "snomed_semantic_search")
            .unwrap();
        assert_eq!(
            semantic.annotations.as_ref().unwrap().open_world_hint,
            Some(true)
        );

        let mapping = tools.iter().find(|tool| tool.name == "snomed_map").unwrap();
        let mapping_schema = Value::Object(mapping.input_schema.as_ref().clone()).to_string();
        assert!(mapping_schema.contains("\"enum\""));
        assert!(mapping_schema.contains("\"snomed\""));
        let add = tools
            .iter()
            .find(|tool| tool.name == "codelist_add")
            .unwrap();
        assert_eq!(add.input_schema["properties"]["sctids"]["minItems"], 1);
        let profile = tools
            .iter()
            .find(|tool| tool.name == "snomed_refset_profile")
            .unwrap();
        assert!(profile.input_schema["properties"].get("limit").is_none());
    }

    // -----------------------------------------------------------------------
    // Test database helpers
    // -----------------------------------------------------------------------

    fn create_test_schema(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS concepts (
                id             TEXT PRIMARY KEY,
                fsn            TEXT NOT NULL,
                preferred_term TEXT NOT NULL,
                synonyms       TEXT,
                hierarchy      TEXT,
                hierarchy_path TEXT,
                parents        TEXT,
                children_count INTEGER,
                attributes     TEXT,
                active         INTEGER NOT NULL,
                module         TEXT,
                effective_time TEXT,
                ctv3_codes     TEXT,
                read2_codes    TEXT,
                schema_version INTEGER NOT NULL DEFAULT 2
            );
            CREATE TABLE IF NOT EXISTS concept_isa (
                child_id  TEXT NOT NULL,
                parent_id TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS concept_maps (
                code        TEXT NOT NULL,
                terminology TEXT NOT NULL,
                concept_id  TEXT NOT NULL,
                PRIMARY KEY (code, terminology)
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS concepts_fts USING fts5(
                id,
                preferred_term,
                synonyms,
                fsn,
                content='concepts',
                content_rowid='rowid'
            );",
        )
        .unwrap();
    }

    /// Insert a concept with minimal required fields.
    /// `hierarchy_path` should be a JSON array like `["ROOT","CFIND"]`.
    fn insert_concept(
        conn: &Connection,
        id: &str,
        preferred_term: &str,
        fsn: &str,
        hierarchy: &str,
        hierarchy_path: &str,
        synonyms: &str, // JSON array string, e.g. `["syn1","syn2"]`
    ) {
        conn.execute(
            "INSERT INTO concepts
             (id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
              parents, children_count, attributes, active, module, effective_time,
              ctv3_codes, read2_codes, schema_version)
             VALUES (?1,?2,?3,?4,?5,?6,'[]',0,'{}',1,'900000000000207008','20240101','[]','[]',2)",
            params![id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path],
        )
        .unwrap();
    }

    /// Insert `n` duplicate IS-A rows (simulating real RF2 data which has ~6 per relationship).
    fn insert_isa(conn: &Connection, child_id: &str, parent_id: &str, n: usize) {
        for _ in 0..n {
            conn.execute(
                "INSERT INTO concept_isa (child_id, parent_id) VALUES (?1, ?2)",
                params![child_id, parent_id],
            )
            .unwrap();
        }
    }

    fn insert_map(conn: &Connection, code: &str, terminology: &str, concept_id: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO concept_maps (code, terminology, concept_id) VALUES (?1,?2,?3)",
            params![code, terminology, concept_id],
        )
        .unwrap();
    }

    fn rebuild_fts(conn: &Connection) {
        conn.execute_batch("INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild')")
            .unwrap();
    }

    /// Build a representative test database.
    ///
    /// Hierarchy:
    ///   ROOT (1000000)
    ///   ├── CFIND (2000000)  [clinical_finding]
    ///   │   ├── DM (3000000)
    ///   │   │   ├── DM1 (4000000)
    ///   │   │   └── DM2 (5000000)
    ///   │   └── HEART (6000000)
    ///   │       ├── MI  (7000000)  ctv3=X200E
    ///   │       └── HF  (8000000)
    ///   └── PROC (9000000)  [procedure]
    ///       └── CPROC (10000000)
    ///
    /// Each IS-A relationship has 6 duplicate rows (real RF2 characteristic).
    fn build_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_test_schema(&conn);

        // Concepts
        insert_concept(
            &conn,
            "1000000",
            "Root concept",
            "Root concept (SNOMED CT concept)",
            "root",
            r#"["Root concept"]"#,
            "[]",
        );
        insert_concept(
            &conn,
            "2000000",
            "Clinical finding",
            "Clinical finding (finding)",
            "clinical_finding",
            r#"["Root concept","Clinical finding"]"#,
            r#"["Finding"]"#,
        );
        insert_concept(
            &conn,
            "3000000",
            "Diabetes mellitus",
            "Diabetes mellitus (disorder)",
            "clinical_finding",
            r#"["Root concept","Clinical finding","Diabetes mellitus"]"#,
            r#"["DM","Diabetes"]"#,
        );
        insert_concept(
            &conn,
            "4000000",
            "Type 1 diabetes mellitus",
            "Type 1 diabetes mellitus (disorder)",
            "clinical_finding",
            r#"["Root concept","Clinical finding","Diabetes mellitus","Type 1 diabetes mellitus"]"#,
            "[]",
        );
        insert_concept(
            &conn,
            "5000000",
            "Type 2 diabetes mellitus",
            "Type 2 diabetes mellitus (disorder)",
            "clinical_finding",
            r#"["Root concept","Clinical finding","Diabetes mellitus","Type 2 diabetes mellitus"]"#,
            "[]",
        );
        insert_concept(
            &conn,
            "6000000",
            "Heart disease",
            "Heart disease (disorder)",
            "clinical_finding",
            r#"["Root concept","Clinical finding","Heart disease"]"#,
            r#"["Cardiac disease"]"#,
        );
        insert_concept(
            &conn,
            "7000000",
            "Myocardial infarction",
            "Myocardial infarction (disorder)",
            "clinical_finding",
            r#"["Root concept","Clinical finding","Heart disease","Myocardial infarction"]"#,
            r#"["Heart attack","MI"]"#,
        );
        insert_concept(
            &conn,
            "8000000",
            "Heart failure",
            "Heart failure (disorder)",
            "clinical_finding",
            r#"["Root concept","Clinical finding","Heart disease","Heart failure"]"#,
            "[]",
        );
        insert_concept(
            &conn,
            "9000000",
            "Procedure",
            "Procedure (procedure)",
            "procedure",
            r#"["Root concept","Procedure"]"#,
            "[]",
        );
        insert_concept(
            &conn,
            "10000000",
            "Cardiac procedure",
            "Cardiac procedure (procedure)",
            "procedure",
            r#"["Root concept","Procedure","Cardiac procedure"]"#,
            "[]",
        );

        // IS-A relationships (6 duplicates each, simulating real RF2 data)
        insert_isa(&conn, "2000000", "1000000", 6);
        insert_isa(&conn, "3000000", "2000000", 6);
        insert_isa(&conn, "4000000", "3000000", 6);
        insert_isa(&conn, "5000000", "3000000", 6);
        insert_isa(&conn, "6000000", "2000000", 6);
        insert_isa(&conn, "7000000", "6000000", 6);
        insert_isa(&conn, "8000000", "6000000", 6);
        insert_isa(&conn, "9000000", "1000000", 6);
        insert_isa(&conn, "10000000", "9000000", 6);

        // CTV3 mapping for MI
        insert_map(&conn, "X200E", "ctv3", "7000000");

        rebuild_fts(&conn);
        conn
    }

    /// Build a linear chain of `depth` concepts with `dup` IS-A rows each.
    /// Used to detect recursion explosion (UNION ALL) as a timing regression.
    fn build_chain_db(depth: usize, dup: usize) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_test_schema(&conn);

        for i in 0..depth {
            let id = format!("{}", 1_000_000 + i);
            let term = format!("Concept {i}");
            let fsn = format!("Concept {i} (disorder)");
            let path: Vec<String> = (0..=i).map(|j| format!("Concept {j}")).collect();
            let path_json = serde_json::to_string(&path).unwrap();
            insert_concept(
                &conn,
                &id,
                &term,
                &fsn,
                "clinical_finding",
                &path_json,
                "[]",
            );
            if i > 0 {
                let parent = format!("{}", 1_000_000 + i - 1);
                insert_isa(&conn, &id, &parent, dup);
            }
        }

        rebuild_fts(&conn);
        conn
    }

    // -----------------------------------------------------------------------
    // tool_children tests
    // -----------------------------------------------------------------------

    #[test]
    fn children_no_duplicates() {
        // With 6 duplicate IS-A rows per relationship, tool_children must still
        // return exactly one row per child (SELECT DISTINCT).
        let conn = build_test_db();
        let args = json!({"id": "3000000", "limit": 100});
        let result = tool_children(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(
            rows.len(),
            2,
            "DM should have exactly 2 children, not {}",
            rows.len()
        );
    }

    #[test]
    fn children_alphabetical_order() {
        let conn = build_test_db();
        let args = json!({"id": "3000000", "limit": 100});
        let result = tool_children(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        let terms: Vec<&str> = rows
            .iter()
            .map(|r| r["preferred_term"].as_str().unwrap())
            .collect();
        assert_eq!(
            terms,
            vec!["Type 1 diabetes mellitus", "Type 2 diabetes mellitus"],
            "children should be sorted alphabetically"
        );
    }

    #[test]
    fn children_empty_for_leaf() {
        let conn = build_test_db();
        let args = json!({"id": "4000000", "limit": 100});
        let result = tool_children(&conn, &args).unwrap();
        assert!(
            result.contains("No children found"),
            "leaf node should return no-children message"
        );
    }

    // -----------------------------------------------------------------------
    // tool_ancestors tests
    // -----------------------------------------------------------------------

    #[test]
    fn ancestors_no_duplicates() {
        // With 6 duplicate IS-A rows, ancestors must still return each ancestor once.
        let conn = build_test_db();
        let args = json!({"id": "4000000"});
        let result = tool_ancestors(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        // DM1 → DM → CFIND → ROOT  (3 ancestors)
        assert_eq!(
            rows.len(),
            3,
            "DM1 should have 3 ancestors, got {}: {}",
            rows.len(),
            result
        );
    }

    #[test]
    fn ancestors_depth_order() {
        // Ancestors should be ordered by depth descending (deepest first = closest to root last).
        // Wait - ORDER BY depth DESC means the deepest hierarchy_path is last alphabetically,
        // but in SNOMED depth is measured from root, so ROOT has depth 1 and leaves have max depth.
        // depth DESC = leaves first, root last.
        let conn = build_test_db();
        let args = json!({"id": "4000000"});
        let result = tool_ancestors(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        // Returned in ORDER BY depth DESC: DM (depth 3) → CFIND (depth 2) → ROOT (depth 1)
        assert_eq!(
            rows[0]["preferred_term"].as_str().unwrap(),
            "Diabetes mellitus"
        );
        assert_eq!(rows[2]["preferred_term"].as_str().unwrap(), "Root concept");
    }

    #[test]
    fn ancestors_timing_regression() {
        // A 25-deep linear chain with 6 duplicate IS-A rows would take astronomically long
        // with UNION ALL (6^25 row operations). With UNION it must complete quickly.
        let conn = build_chain_db(25, 6);
        let leaf_id = format!("{}", 1_000_000 + 24);
        let args = json!({"id": leaf_id});

        let start = Instant::now();
        let result = tool_ancestors(&conn, &args).unwrap();
        let elapsed = start.elapsed();

        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(
            rows.len(),
            24,
            "chain of 25 should have 24 ancestors, got {}",
            rows.len()
        );
        assert!(
            elapsed.as_millis() < 500,
            "ancestors on 25-deep chain with 6× duplicates took {}ms - UNION ALL explosion?",
            elapsed.as_millis()
        );
    }

    // -----------------------------------------------------------------------
    // tool_search tests
    // -----------------------------------------------------------------------

    #[test]
    fn search_by_preferred_term() {
        let conn = build_test_db();
        let args = json!({"query": "myocardial", "limit": 10});
        let result = tool_search(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"].as_str().unwrap(), "7000000");
    }

    #[test]
    fn search_by_synonym() {
        // "Heart attack" is a synonym of Myocardial infarction in the test DB.
        let conn = build_test_db();
        let args = json!({"query": "Heart attack", "limit": 10});
        let result = tool_search(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert!(
            rows.iter().any(|r| r["id"].as_str() == Some("7000000")),
            "search for synonym 'Heart attack' should find MI (7000000); got: {result}"
        );
    }

    #[test]
    fn search_no_results() {
        let conn = build_test_db();
        let args = json!({"query": "ZZZNOTFOUND", "limit": 10});
        let result = tool_search(&conn, &args).unwrap();
        assert!(
            result.contains("No results found"),
            "expected no-results message"
        );
    }

    // -----------------------------------------------------------------------
    // tool_concept tests
    // -----------------------------------------------------------------------

    #[test]
    fn concept_found_by_id() {
        let conn = build_test_db();
        let args = json!({"id": "7000000"});
        let result = tool_concept(&conn, &args, None).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            v["preferred_term"].as_str().unwrap(),
            "Myocardial infarction"
        );
        assert_eq!(v["hierarchy"].as_str().unwrap(), "clinical_finding");
    }

    #[test]
    fn concept_not_found() {
        let conn = build_test_db();
        let args = json!({"id": "9999999999"});
        let result = tool_concept(&conn, &args, None).unwrap();
        assert!(result.contains("not found"));
    }

    // -----------------------------------------------------------------------
    // tool_hierarchy tests
    // -----------------------------------------------------------------------

    #[test]
    fn hierarchy_filter() {
        let conn = build_test_db();
        let args = json!({"hierarchy": "procedure", "limit": 100});
        let result = tool_hierarchy(&conn, &args).unwrap();
        let rows: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(rows.len(), 2, "procedure hierarchy should have 2 concepts");
        assert!(rows.iter().all(|r| {
            let term = r["preferred_term"].as_str().unwrap_or("");
            term.contains("Procedure") || term.contains("procedure")
        }));
    }

    #[test]
    fn hierarchy_not_found() {
        let conn = build_test_db();
        let args = json!({"hierarchy": "nonexistent", "limit": 100});
        let result = tool_hierarchy(&conn, &args).unwrap();
        assert!(result.contains("No concepts found in hierarchy"));
    }

    // -----------------------------------------------------------------------
    // tool_map tests
    // -----------------------------------------------------------------------

    #[test]
    fn map_snomed_to_ctv3() {
        let conn = build_test_db();
        let args = json!({"code": "7000000", "terminology": "snomed"});
        let result = tool_map(&conn, &args).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let ctv3: Vec<&str> = v["ctv3_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert_eq!(ctv3, vec!["X200E"]);
    }

    #[test]
    fn map_ctv3_to_snomed() {
        let conn = build_test_db();
        let args = json!({"code": "X200E", "terminology": "ctv3"});
        let result = tool_map(&conn, &args).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let concepts = v["snomed_concepts"].as_array().unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0]["id"].as_str().unwrap(), "7000000");
    }

    #[test]
    fn map_no_mappings() {
        // DM has no CTV3 mappings in the test DB.
        let conn = build_test_db();
        let args = json!({"code": "3000000", "terminology": "snomed"});
        let result = tool_map(&conn, &args).unwrap();
        assert!(result.contains("No mappings found"));
    }

    #[test]
    fn map_unknown_terminology() {
        let conn = build_test_db();
        let args = json!({"code": "7000000", "terminology": "unknown"});
        assert!(tool_map(&conn, &args).is_err());
    }

    #[test]
    fn map_unknown_target_terminology() {
        let conn = build_test_db();
        let args = json!({"code": "7000000", "terminology": "snomed", "to": "unknown"});
        assert!(tool_map(&conn, &args).is_err());
    }

    #[test]
    fn map_directly_to_target_terminology() {
        let conn = build_test_db();
        let args = json!({"code": "7000000", "terminology": "snomed", "to": "ctv3"});
        let result = tool_map(&conn, &args).unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["from"], "snomed");
        assert_eq!(value["to"], "ctv3");
        assert_eq!(value["mapped"][0]["target"], "X200E");
    }

    #[test]
    fn map_directly_to_snomed_enriches_concepts() {
        let conn = build_test_db();
        let args = json!({"code": "X200E", "terminology": "ctv3", "to": "snomed"});
        let result = tool_map(&conn, &args).unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["snomed_concepts"][0]["id"], "7000000");
        assert_eq!(
            value["snomed_concepts"][0]["preferred_term"],
            "Myocardial infarction"
        );
    }

    /// Enforce the drift class behind issue #106: under
    /// `additionalProperties: false`, a value must not carry a key the schema
    /// does not declare, and must carry every `required` key. Recurses through
    /// `properties` and array `items`; unconstrained objects (`{"type":
    /// "object"}` with no `properties`) are deliberately not recursed into,
    /// matching how the schemas treat free-form sub-objects.
    fn assert_conforms(value: &Value, schema: &Value, path: &str) {
        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
            let obj = value
                .as_object()
                .unwrap_or_else(|| panic!("{path}: expected an object, got {value}"));
            if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                for key in obj.keys() {
                    assert!(
                        props.contains_key(key),
                        "{path}: response carries undeclared key `{key}` while its schema sets \
                         additionalProperties:false - a struct and its hand-written schema have \
                         drifted (the shape of issue #106)"
                    );
                }
            }
            if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
                for req in required {
                    let req = req.as_str().expect("required entry is a string");
                    assert!(
                        obj.contains_key(req),
                        "{path}: response is missing required key `{req}`"
                    );
                }
            }
            for (key, sub_schema) in props {
                if let Some(child) = obj.get(key) {
                    assert_conforms(child, sub_schema, &format!("{path}.{key}"));
                }
            }
        } else if let Some(items) = schema.get("items") {
            if let Some(array) = value.as_array() {
                for (index, element) in array.iter().enumerate() {
                    assert_conforms(element, items, &format!("{path}[{index}]"));
                }
            }
        }
    }

    /// Every MCP tool's real response must validate against its own declared
    /// `output_schema`. Regression test for issue #106, where five list tools
    /// serialised an `active` field their schemas forbade. The pre-existing
    /// `every_tool_has_a_complete_typed_contract` only checked each schema's
    /// shape, never a real response against it, so the drift shipped silently.
    #[test]
    fn tool_responses_validate_against_their_declared_schema() {
        let conn = build_test_db();
        let tools = tool_definitions(true);

        let cases: [(&str, Value); 5] = [
            ("snomed_search", json!({ "query": "diabetes", "limit": 10 })),
            ("snomed_children", json!({ "id": "3000000" })),
            ("snomed_ancestors", json!({ "id": "4000000" })),
            ("snomed_concept", json!({ "id": "7000000" })),
            (
                "snomed_map",
                json!({ "code": "X200E", "terminology": "ctv3", "to": "snomed" }),
            ),
        ];

        for (name, args) in cases {
            let data = call_tool_for_test(&conn, name, args)
                .unwrap_or_else(|error| panic!("{name} failed: {error:#}"));
            let tool = tools
                .iter()
                .find(|tool| tool.name.as_ref() == name)
                .expect("tool is defined");
            let output = tool.output_schema.as_ref().expect("output schema");
            assert_conforms(
                &data,
                &output["properties"]["data"],
                &format!("{name}.data"),
            );
        }
    }

    /// Belt-and-braces for the row structs the in-memory `build_test_db`
    /// cannot exercise (notably `RefsetMember`, since the fixture has no
    /// reference sets): serialise a sample of each and validate it against the
    /// item schema its tool emits. This is what would have caught issue #106.
    #[test]
    fn row_structs_match_their_item_schemas() {
        assert_conforms(
            &serde_json::to_value(crate::sdk::SearchHit {
                id: "22298006".into(),
                preferred_term: "Myocardial infarction".into(),
                fsn: "Myocardial infarction (disorder)".into(),
                hierarchy: "Clinical finding".into(),
                active: true,
            })
            .unwrap(),
            &data_schema(OutputShape::SearchResults)["items"],
            "SearchHit",
        );
        assert_conforms(
            &serde_json::to_value(crate::sdk::ConceptSummary {
                id: "22298006".into(),
                preferred_term: "Myocardial infarction".into(),
                fsn: "Myocardial infarction (disorder)".into(),
                active: true,
            })
            .unwrap(),
            &data_schema(OutputShape::ConceptSummaries)["items"],
            "ConceptSummary",
        );
        assert_conforms(
            &serde_json::to_value(crate::refset::RefsetMember {
                id: "22298006".into(),
                preferred_term: "Myocardial infarction".into(),
                fsn: "Myocardial infarction (disorder)".into(),
                hierarchy: "Clinical finding".into(),
                effective_time: "20260101".into(),
                active: true,
            })
            .unwrap(),
            &data_schema(OutputShape::RefsetMembers)["items"],
            "RefsetMember",
        );
        assert_conforms(
            &serde_json::to_value(crate::refset::RefsetSummary {
                id: "900000000000497000".into(),
                preferred_term: "CTV3 simple map".into(),
                fsn: "CTV3 simple map reference set (foundation metadata concept)".into(),
                module: "900000000000207008".into(),
                member_count: 3,
            })
            .unwrap(),
            &data_schema(OutputShape::Refsets)["items"],
            "RefsetSummary",
        );
        assert_conforms(
            &serde_json::to_value(crate::refset::HierarchyCount {
                hierarchy: "Clinical finding".into(),
                count: 42,
            })
            .unwrap(),
            &data_schema(OutputShape::HierarchyCounts)["items"],
            "HierarchyCount",
        );
    }

    /// Real fixture-backed database, built through the actual RF2 -> NDJSON
    /// -> SQLite pipeline against the committed synthetic fixture (mirrors
    /// `tests/fhir_conformance.rs::build_db`). Unlike `build_test_db`, this
    /// has genuine reference sets, needed to exercise `snomed_refsets` and
    /// friends over the live path rather than the hand-built in-memory schema.
    fn build_fixture_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let ndjson_path = dir.path().join("fixture.ndjson");
        let db_path = dir.path().join("fixture.db");
        let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rf2/SnomedCT_SyntheticTest_PRODUCTION_20260101T120000Z");
        crate::commands::ndjson::run(crate::commands::ndjson::Args {
            rf2_dirs: vec![fixture_dir],
            locale: "en-GB".to_string(),
            output: Some(ndjson_path.clone()),
            include_inactive: false,
            refsets: crate::commands::ndjson::RefsetMode::Simple,
        })
        .unwrap();
        crate::commands::sqlite::run(crate::commands::sqlite::Args {
            input: ndjson_path,
            output: Some(db_path.clone()),
            transitive_closure: false,
            include_self: false,
        })
        .unwrap();
        let conn = Connection::open(&db_path).unwrap();
        (dir, conn)
    }

    /// R58 (issue #106 follow-up): the refset tools were unverified over the
    /// live path because `build_test_db` carries no reference sets. Drive
    /// them from a real fixture database instead and validate each real
    /// response against its own declared `output_schema`, as
    /// `tool_responses_validate_against_their_declared_schema` already does
    /// for the five tools `build_test_db` can exercise.
    #[test]
    fn refset_tools_validate_against_their_declared_schema() {
        let (_dir, conn) = build_fixture_db();
        let tools = tool_definitions(true);
        let data_schema_for = |name: &str| {
            tools
                .iter()
                .find(|tool| tool.name.as_ref() == name)
                .unwrap_or_else(|| panic!("{name} is not a defined tool"))
                .output_schema
                .as_ref()
                .expect("output schema")["properties"]["data"]
                .clone()
        };

        // Simple refset carrying type 1 + type 2 diabetes mellitus in the
        // committed synthetic fixture (see tests/fhir_conformance.rs).
        const EXAMPLE_REFSET: &str = "991381000000107";

        let refsets = call_tool_for_test(&conn, "snomed_refsets", json!({}))
            .unwrap_or_else(|error| panic!("snomed_refsets failed: {error:#}"));
        assert_conforms(
            &refsets,
            &data_schema_for("snomed_refsets"),
            "snomed_refsets.data",
        );
        assert!(
            !refsets.as_array().unwrap().is_empty(),
            "snomed_refsets returned no data: {refsets}"
        );

        let members = call_tool_for_test(
            &conn,
            "snomed_refset_members",
            json!({ "refset_id": EXAMPLE_REFSET }),
        )
        .unwrap_or_else(|error| panic!("snomed_refset_members failed: {error:#}"));
        assert_conforms(
            &members,
            &data_schema_for("snomed_refset_members"),
            "snomed_refset_members.data",
        );
        assert!(
            !members.as_array().unwrap().is_empty(),
            "snomed_refset_members returned no data: {members}"
        );

        let compare = call_tool_for_test(
            &conn,
            "snomed_refset_compare",
            json!({ "refset_id_a": EXAMPLE_REFSET, "refset_id_b": EXAMPLE_REFSET }),
        )
        .unwrap_or_else(|error| panic!("snomed_refset_compare failed: {error:#}"));
        assert_conforms(
            &compare,
            &data_schema_for("snomed_refset_compare"),
            "snomed_refset_compare.data",
        );
        assert!(
            compare["in_both"]["count"].as_u64().unwrap() > 0,
            "snomed_refset_compare returned no data: {compare}"
        );

        let profile = call_tool_for_test(
            &conn,
            "snomed_refset_profile",
            json!({ "refset_id": EXAMPLE_REFSET }),
        )
        .unwrap_or_else(|error| panic!("snomed_refset_profile failed: {error:#}"));
        assert_conforms(
            &profile,
            &data_schema_for("snomed_refset_profile"),
            "snomed_refset_profile.data",
        );
        assert!(
            !profile.as_array().unwrap().is_empty(),
            "snomed_refset_profile returned no data: {profile}"
        );
    }

    /// R58, continued: the codelist read/report tools, driven end-to-end
    /// against a codelist built with the mutation tool functions
    /// (`tool_codelist_new` then `tool_codelist_add`) over real fixture
    /// concepts, each response checked against its own declared
    /// `output_schema`. Calls the tool functions directly (as the other unit
    /// tests in this module do, e.g. `tool_children`) rather than through
    /// `call_tool_for_test`, since `codelist_new` and `codelist_add` are
    /// mutation tools, and `codelist_read` is dispatched alongside them - all
    /// three reachable only from `call_tool_sync`, not `call_database_tool`.
    ///
    /// `snomed_semantic_search` is deliberately not covered here: it needs an
    /// embeddings artefact that `build_fixture_db` does not build.
    #[test]
    fn codelist_tools_validate_against_their_declared_schema() {
        let (_dir, conn) = build_fixture_db();
        let tools = tool_definitions(true);
        let data_schema_for = |name: &str| {
            tools
                .iter()
                .find(|tool| tool.name.as_ref() == name)
                .unwrap_or_else(|| panic!("{name} is not a defined tool"))
                .output_schema
                .as_ref()
                .expect("output schema")["properties"]["data"]
                .clone()
        };
        let to_value =
            |text: String| serde_json::from_str::<Value>(&text).expect("tool returns JSON");

        let codelist_dir = tempfile::tempdir().unwrap();
        let root = CodelistRoot::new(codelist_dir.path()).unwrap();

        tool_codelist_new(
            &json!({ "file": "diabetes.codelist", "title": "Diabetes codes" }),
            &root,
        )
        .unwrap_or_else(|error| panic!("codelist_new failed: {error:#}"));
        tool_codelist_add(
            &conn,
            // Type 1 + type 2 diabetes mellitus, active in the fixture.
            &json!({ "file": "diabetes.codelist", "sctids": ["46635009", "44054006"] }),
            &root,
        )
        .unwrap_or_else(|error| panic!("codelist_add failed: {error:#}"));

        let list = to_value(
            tool_codelist_list(&json!({}), &root)
                .unwrap_or_else(|error| panic!("codelist_list failed: {error:#}")),
        );
        assert_conforms(
            &list,
            &data_schema_for("codelist_list"),
            "codelist_list.data",
        );
        assert!(
            !list.as_array().unwrap().is_empty(),
            "codelist_list returned no data: {list}"
        );

        let read = to_value(
            tool_codelist_read(&json!({ "file": "diabetes.codelist" }), &root)
                .unwrap_or_else(|error| panic!("codelist_read failed: {error:#}")),
        );
        assert_conforms(
            &read,
            &data_schema_for("codelist_read"),
            "codelist_read.data",
        );
        assert_eq!(
            read["active_concepts"].as_array().unwrap().len(),
            2,
            "codelist_read: {read}"
        );

        let validate = to_value(
            tool_codelist_validate(&conn, &json!({ "file": "diabetes.codelist" }), &root)
                .unwrap_or_else(|error| panic!("codelist_validate failed: {error:#}")),
        );
        assert_conforms(
            &validate,
            &data_schema_for("codelist_validate"),
            "codelist_validate.data",
        );
        assert_eq!(
            validate["active_concepts"], 2,
            "codelist_validate: {validate}"
        );
        assert_eq!(validate["valid"], true, "codelist_validate: {validate}");

        let stats = to_value(
            tool_codelist_stats(&conn, &json!({ "file": "diabetes.codelist" }), &root)
                .unwrap_or_else(|error| panic!("codelist_stats failed: {error:#}")),
        );
        assert_conforms(
            &stats,
            &data_schema_for("codelist_stats"),
            "codelist_stats.data",
        );
        assert_eq!(stats["active_concepts"], 2, "codelist_stats: {stats}");
        assert!(
            !stats["by_hierarchy"].as_array().unwrap().is_empty(),
            "codelist_stats returned no data: {stats}"
        );
    }

    #[tokio::test]
    async fn bounded_reader_accepts_newline_delimited_json() {
        let body = r#"{"jsonrpc":"2.0","method":"ping"}"#;
        let raw = format!("{}\n", body);
        let mut reader = BufReader::new(raw.as_bytes());
        let mut line = Vec::new();
        assert!(read_bounded_line(&mut reader, &mut line).await.unwrap());
        assert_eq!(line, body.as_bytes());
    }

    #[tokio::test]
    async fn bounded_reader_returns_false_at_clean_eof() {
        let mut reader = BufReader::new(&b""[..]);
        let mut line = Vec::new();
        assert!(!read_bounded_line(&mut reader, &mut line).await.unwrap());
    }

    #[tokio::test]
    async fn bounded_reader_rejects_unterminated_input() {
        let mut reader = BufReader::new(&b"not-delimited"[..]);
        let mut line = Vec::new();
        let error = read_bounded_line(&mut reader, &mut line).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn bounded_reader_rejects_line_over_cap() {
        let mut raw = vec![b'x'; MAX_MESSAGE_SIZE + 1];
        raw.push(b'\n');
        let mut reader = BufReader::new(raw.as_slice());
        let mut line = Vec::new();
        let error = read_bounded_line(&mut reader, &mut line).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn bounded_transport_rejects_requests_beyond_the_in_flight_limit() {
        let (mut input, reader) = tokio::io::duplex(4096);
        let (writer, output) = tokio::io::duplex(4096);
        let mut transport = BoundedStdioTransport::new(reader, writer);

        for id in 1..=MAX_IN_FLIGHT_REQUESTS + 1 {
            input
                .write_all(
                    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n").as_bytes(),
                )
                .await
                .unwrap();
        }
        input
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .unwrap();

        let mut held = Vec::new();
        for _ in 0..MAX_IN_FLIGHT_REQUESTS {
            held.push(transport.receive().await.unwrap());
        }
        assert!(matches!(
            transport.receive().await,
            Some(JsonRpcMessage::Notification(_))
        ));

        let mut output = BufReader::new(output);
        let mut line = String::new();
        output.read_line(&mut line).await.unwrap();
        let error: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(error["id"], MAX_IN_FLIGHT_REQUESTS + 1);
        assert_eq!(error["error"]["code"], -32000);

        let completed = held.pop().unwrap();
        let completed_id = match &completed {
            JsonRpcMessage::Request(request) => request.id.clone(),
            _ => unreachable!("held messages are requests"),
        };
        drop(completed);

        let still_full_id = MAX_IN_FLIGHT_REQUESTS + 2;
        input
            .write_all(
                format!(
                    "{{\"jsonrpc\":\"2.0\",\"id\":{still_full_id},\"method\":\"ping\"}}\n\
                     {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(matches!(
            transport.receive().await,
            Some(JsonRpcMessage::Notification(_))
        ));
        line.clear();
        output.read_line(&mut line).await.unwrap();
        let error: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(error["id"], still_full_id);
        assert_eq!(error["error"]["code"], -32000);

        transport
            .send(TxJsonRpcMessage::<RoleServer>::response(
                rmcp::model::ServerResult::empty(()),
                completed_id,
            ))
            .await
            .unwrap();

        let accepted_id = MAX_IN_FLIGHT_REQUESTS + 3;
        input
            .write_all(
                format!("{{\"jsonrpc\":\"2.0\",\"id\":{accepted_id},\"method\":\"ping\"}}\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        assert!(matches!(
            transport.receive().await,
            Some(JsonRpcMessage::Request(_))
        ));
    }

    #[test]
    fn in_flight_slots_cover_response_and_cancellation_lifecycles() {
        let registry = InFlightRegistry::new();

        let normal_id = RequestId::Number(1);
        let normal = registry.start(normal_id.clone()).unwrap();
        drop(normal);
        assert_eq!(
            registry.slots.available_permits(),
            MAX_IN_FLIGHT_REQUESTS - 1
        );
        registry.response_started(&normal_id);
        registry.response_finished(&normal_id);
        assert_eq!(registry.slots.available_permits(), MAX_IN_FLIGHT_REQUESTS);

        let cancelled_id = RequestId::Number(2);
        let cancelled = registry.start(cancelled_id.clone()).unwrap();
        registry.cancel(&cancelled_id);
        assert_eq!(
            registry.slots.available_permits(),
            MAX_IN_FLIGHT_REQUESTS - 1
        );
        drop(cancelled);
        assert_eq!(registry.slots.available_permits(), MAX_IN_FLIGHT_REQUESTS);

        let writing_id = RequestId::Number(3);
        let writing = registry.start(writing_id.clone()).unwrap();
        drop(writing);
        registry.response_started(&writing_id);
        registry.cancel(&writing_id);
        assert_eq!(
            registry.slots.available_permits(),
            MAX_IN_FLIGHT_REQUESTS - 1
        );
        registry.response_finished(&writing_id);
        assert_eq!(registry.slots.available_permits(), MAX_IN_FLIGHT_REQUESTS);
    }

    #[test]
    fn mutation_cancelled_while_waiting_for_lock_does_not_run() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let directory = tempfile::tempdir().unwrap();
        let server = SctMcp::new(
            Connection::open_in_memory().unwrap(),
            None,
            None,
            CodelistRoot::new(directory.path()).unwrap(),
        );
        let worker = server.clone();
        let mutation_lock = server.codelist_mutations.clone();
        let guard = mutation_lock.lock().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let checks = Arc::new(AtomicUsize::new(0));
        let worker_cancelled = cancelled.clone();
        let worker_checks = checks.clone();
        let request = CallToolRequestParams::new("codelist_new").with_arguments(
            json!({ "file": "cancelled.codelist", "title": "Cancelled" })
                .as_object()
                .unwrap()
                .clone(),
        );

        let handle = std::thread::spawn(move || {
            worker.call_tool_sync(request, || {
                let check = worker_checks.fetch_add(1, Ordering::SeqCst);
                check > 0 && worker_cancelled.load(Ordering::SeqCst)
            })
        });
        let deadline = Instant::now() + std::time::Duration::from_secs(1);
        while checks.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < deadline,
                "worker did not check cancellation"
            );
            std::thread::yield_now();
        }
        cancelled.store(true, Ordering::SeqCst);
        drop(guard);

        let response = handle.join().unwrap().unwrap();
        match response {
            CallToolResponse::Complete(result) => assert_eq!(result.is_error, Some(true)),
            _ => panic!("cancelled tool returned a non-complete response"),
        }
        assert!(!directory.path().join("cancelled.codelist").exists());
        assert!(checks.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn codelist_root_rejects_traversal_and_outside_absolute_paths() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let root = CodelistRoot::new(directory.path()).unwrap();

        assert!(root.resolve_new_file("../outside.codelist").is_err());
        assert!(root
            .resolve_existing_file(outside.path().to_str().unwrap())
            .is_err());
    }

    #[test]
    fn codelist_root_allows_nested_new_files_only_with_expected_extension() {
        let directory = tempfile::tempdir().unwrap();
        let root = CodelistRoot::new(directory.path()).unwrap();

        let path = root.resolve_new_file("nested/example.codelist").unwrap();
        assert_eq!(root.display(&path), "nested/example.codelist");
        assert!(path.parent().unwrap().is_dir());
        assert!(root.resolve_new_file("nested/example.txt").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn codelist_root_rejects_symlink_components() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), directory.path().join("linked")).unwrap();
        let root = CodelistRoot::new(directory.path()).unwrap();

        assert!(root.resolve_new_file("linked/example.codelist").is_err());
    }
}
