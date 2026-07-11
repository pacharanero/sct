// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct serve` - a FHIR R4 terminology server over the SQLite artefact.
//!
//! Phase 1: `/metadata` (CapabilityStatement), `CodeSystem/$lookup`,
//! `$validate-code`, `$subsumes`, and `ValueSet/$expand` (text filter + full
//! ECL via [`crate::ecl`]). See `spec/commands/serve.md`. The operation logic
//! lives in [`ops`] as pure functions; the handlers here are thin transport.

pub mod fhir;
pub mod ops;
pub mod pool;
pub mod valuesets;

use anyhow::{Context, Result};
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use rusqlite::Connection;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use crate::index::query::Index;
use fhir::FhirError;
use pool::ConnectionPool;
use valuesets::ValueSetRegistry;

/// Private page cache per pooled connection (KiB). Modest on purpose: with
/// `mmap_size` set in `open_db_readonly`, reads come from the shared
/// memory-mapped file, so a large per-connection cache would just multiply
/// resident memory across the pool for little benefit.
const POOL_CACHE_KIB: u32 = 8192;

#[derive(Parser, Debug)]
pub struct Args {
    /// SNOMED CT SQLite database produced by `sct sqlite`. Discovered via the
    /// usual path-resolution chain when omitted (see `docs/path-resolution.md`).
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub db: Option<PathBuf>,

    /// TCP port to listen on.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// Host/address to bind.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// FHIR base path. Set to `/fhir` for Ontoserver-compatible URLs.
    #[arg(long, default_value = "/")]
    pub fhir_base: String,

    /// Directory of `.codelist` files to serve as named FHIR ValueSets
    /// (default `./codelists`, or `$SCT_CODELISTS` / `[codelists] dir`).
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub codelists: Option<PathBuf>,

    /// FST index (from `sct fst build`) that powers the `GET /autocomplete`
    /// search-as-you-type endpoint. Auto-discovered as `snomed.fst` next to the
    /// database when omitted; if none is found, `/autocomplete` returns 501.
    #[arg(long, value_parser = crate::paths::tilde_pathbuf)]
    pub fst: Option<PathBuf>,

    /// Refuse write operations (always true; the server is read-only).
    #[arg(long, default_value_t = true)]
    pub read_only: bool,

    /// Size of the read-only SQLite connection pool. Each request borrows a warm
    /// connection instead of opening a fresh one, so this also bounds how many
    /// queries run concurrently. `0` auto-sizes to 2x the logical CPU count
    /// (clamped to 4..64).
    #[arg(long, default_value_t = 0)]
    pub pool_size: usize,
}

#[derive(Clone)]
struct AppState {
    /// Warm pool of read-only connections shared by every DB-backed operation.
    pool: Arc<ConnectionPool>,
    impl_url: Arc<String>,
    registry: Arc<ValueSetRegistry>,
    translate_available: bool,
    /// FST index backing `/autocomplete`, if one was supplied/discovered.
    fst: Option<Arc<Index>>,
}

pub fn run(args: Args) -> Result<()> {
    let db = crate::paths::resolve_db(args.db.as_deref())?.path;
    // Open once up front so a bad/missing DB fails before we bind the port, and
    // nudge the user about the transitive-closure table while we're here.
    {
        let conn = crate::commands::open_db_readonly(&db, None)
            .with_context(|| format!("opening database {}", db.display()))?;
        crate::ecl::warn_if_no_tct(&conn);
    }

    let addr = format!("{}:{}", args.host, args.port);
    let listener = std::net::TcpListener::bind(&addr).with_context(|| format!("binding {addr}"))?;
    let base = normalise_base(&args.fhir_base);
    eprintln!(
        "sct serve: FHIR R4 terminology server on http://{addr}{base}\n  database: {}\n  try: curl 'http://{addr}{base}/metadata'",
        db.display()
    );
    let codelists = crate::paths::codelist_registry(args.codelists.as_deref());
    let fst = resolve_fst(args.fst.as_deref(), &db);
    serve_listener(
        db,
        &args.fhir_base,
        Some(codelists),
        fst,
        args.pool_size,
        listener,
    )
}

/// Resolve the connection-pool size: an explicit non-zero request, else 2x the
/// logical CPU count clamped to a sane 4..=64.
fn resolve_pool_size(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cores * 2).clamp(4, 64)
}

/// Resolve the FST index for `/autocomplete`: an explicit `--fst` path, else a
/// `snomed.fst` sibling of the database if one exists (`None` otherwise).
fn resolve_fst(explicit: Option<&FsPath>, db: &FsPath) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    let sibling = db.parent().unwrap_or(FsPath::new(".")).join("snomed.fst");
    sibling.exists().then_some(sibling)
}

/// Serve the FHIR router on an already-bound std listener, blocking. Shared by
/// `run` and by integration tests (which bind an ephemeral port first).
/// `codelists` is the directory of `.codelist` files to expose as ValueSets
/// (`None` to serve none).
#[doc(hidden)]
pub fn serve_listener(
    db: PathBuf,
    fhir_base: &str,
    codelists: Option<PathBuf>,
    fst: Option<PathBuf>,
    pool_size: usize,
    listener: std::net::TcpListener,
) -> Result<()> {
    let base = normalise_base(fhir_base);
    let addr = listener.local_addr().context("listener address")?;
    let impl_url = format!("http://{addr}{base}");
    let registry = match &codelists {
        Some(dir) => valuesets::load_registry(dir, &impl_url),
        None => ValueSetRegistry::default(),
    };
    if !registry.is_empty() {
        if let Some(dir) = &codelists {
            eprintln!(
                "  serving {} ValueSet(s) from {}",
                registry.len(),
                dir.display()
            );
        }
    }
    // Load the FST index for /autocomplete, if supplied/discovered. A failure to
    // open it is a warning, not fatal - the rest of the server still serves.
    let fst_index = fst.as_ref().and_then(|path| match Index::open(path) {
        Ok(ix) => {
            eprintln!(
                "  autocomplete: GET /autocomplete backed by {}",
                path.display()
            );
            Some(Arc::new(ix))
        }
        Err(e) => {
            eprintln!(
                "  warning: FST index {} failed to open ({e:#}); /autocomplete disabled",
                path.display()
            );
            None
        }
    });

    let translate_available = table_exists(&db, "crossmaps")?;
    let pool_size = resolve_pool_size(pool_size);
    let pool = ConnectionPool::open(&db, pool_size, POOL_CACHE_KIB)
        .with_context(|| format!("opening connection pool for {}", db.display()))?;
    eprintln!("  connection pool: {pool_size} warm read-only connection(s)");
    let state = AppState {
        translate_available,
        pool: Arc::new(pool),
        impl_url: Arc::new(impl_url),
        registry: Arc::new(registry),
        fst: fst_index,
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    rt.block_on(async move {
        listener.set_nonblocking(true).context("set_nonblocking")?;
        let listener = tokio::net::TcpListener::from_std(listener).context("from_std")?;
        let app = build_router(state, &base);
        axum::serve(listener, app).await.context("serving")?;
        Ok::<_, anyhow::Error>(())
    })
}

fn table_exists(db: &FsPath, table: &str) -> Result<bool> {
    let conn = crate::commands::open_db_readonly(db, None)
        .with_context(|| format!("opening database {}", db.display()))?;
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
        )",
        [table],
        |r| r.get(0),
    )?;
    Ok(exists != 0)
}

fn normalise_base(base: &str) -> String {
    let b = base.trim_end_matches('/');
    if b.is_empty() {
        String::new()
    } else if b.starts_with('/') {
        b.to_string()
    } else {
        format!("/{b}")
    }
}

fn build_router(state: AppState, base: &str) -> Router {
    let app = Router::new()
        .route("/", post(batch))
        .route("/metadata", get(metadata))
        .route("/CodeSystem/$lookup", get(lookup).post(lookup))
        .route(
            "/CodeSystem/$validate-code",
            get(validate_code).post(validate_code),
        )
        .route("/CodeSystem/$subsumes", get(subsumes).post(subsumes))
        .route("/ValueSet/$expand", get(expand).post(expand))
        .route(
            "/ValueSet/$validate-code",
            get(vs_validate_code).post(vs_validate_code),
        )
        .route("/ValueSet", get(valueset_search))
        .route("/ValueSet/{id}", get(valueset_read))
        .route("/ValueSet/{id}/$expand", get(valueset_expand_id))
        .route("/ConceptMap/$translate", get(translate).post(translate))
        .route("/autocomplete", get(autocomplete))
        .with_state(state);
    if base.is_empty() {
        app
    } else {
        Router::new().nest(base, app)
    }
}

// --- handlers ---------------------------------------------------------------

async fn metadata(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    // `?mode=terminology` returns a TerminologyCapabilities instead of the
    // CapabilityStatement (FHIR's terminology-server discovery convention).
    let params = parse_query(q.as_deref().unwrap_or(""));
    if param(&params, "mode") == Some("terminology") {
        return fhir_ok(fhir::terminology_capabilities(
            env!("CARGO_PKG_VERSION"),
            &st.impl_url,
            st.translate_available,
        ));
    }
    fhir_ok(fhir::capability_statement(
        env!("CARGO_PKG_VERSION"),
        &st.impl_url,
        st.translate_available,
    ))
}

/// `GET /autocomplete?q=<partial>&count=<n>` - search-as-you-type over the FST
/// index, the same [`Index::search_typeahead`] core as `sct sayt`. Plain JSON
/// (not FHIR): `{"query": "...", "hits": [{"id","display","score","tag"}, ...]}`,
/// with `id` a string (SCTIDs exceed JavaScript's safe-integer range). Returns
/// `501` if the server was started without an FST index.
async fn autocomplete(State(st): State<AppState>, RawQuery(q): RawQuery) -> Response {
    let params = parse_query(q.as_deref().unwrap_or(""));
    let query = param(&params, "q").unwrap_or("");
    let count = param(&params, "count")
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(10)
        .clamp(1, 100);
    let Some(index) = &st.fst else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({
                "error": "autocomplete is unavailable: start `sct serve` with `--fst <snomed.fst>` (build one with `sct fst build`)"
            })),
        )
            .into_response();
    };
    let hits = index.search_typeahead(query, count, true);
    Json(serde_json::json!({
        "query": query,
        "hits": hits.iter().map(|h| h.to_json()).collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn lookup(State(st): State<AppState>, headers: HeaderMap, RawQuery(q): RawQuery) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    let Some(code) = param(&params, "code").map(str::to_string) else {
        return fhir_err(FhirError::invalid("missing required parameter 'code'"));
    };
    let props = params_all(&params, "property");
    run_db(&st, move |c| ops::lookup(c, &code, &props)).await
}

async fn validate_code(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    let Some(code) = param(&params, "code").map(str::to_string) else {
        return fhir_err(FhirError::invalid("missing required parameter 'code'"));
    };
    let display = param(&params, "display").map(str::to_string);
    run_db(&st, move |c| {
        ops::validate_code(c, &code, display.as_deref())
    })
    .await
}

async fn subsumes(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    let (Some(a), Some(b)) = (
        param(&params, "codeA").map(str::to_string),
        param(&params, "codeB").map(str::to_string),
    ) else {
        return fhir_err(FhirError::invalid(
            "missing required parameters 'codeA' and 'codeB'",
        ));
    };
    run_db(&st, move |c| ops::subsumes(c, &a, &b)).await
}

async fn expand(State(st): State<AppState>, headers: HeaderMap, RawQuery(q): RawQuery) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    let (count, offset, include_designations) = pagination(&params);

    // A `url` naming a stored `.codelist` ValueSet expands its member set.
    if let Some(url) = param(&params, "url") {
        if let Some(vs) = st.registry.resolve_url(url) {
            let members = vs.members.clone();
            return run_db(&st, move |c| {
                ops::expand_members(c, &members, count, offset, include_designations)
            })
            .await;
        }
    }

    let ecl = param(&params, "url").and_then(parse_implicit_ecl);
    let filter = param(&params, "filter").map(str::to_string);
    run_db(&st, move |c| {
        ops::expand(
            c,
            ecl.as_deref(),
            filter.as_deref(),
            count,
            offset,
            include_designations,
        )
    })
    .await
}

/// `GET /ValueSet` - a searchset Bundle of the registered ValueSets (metadata
/// only), optionally filtered by `?url=` or `?_id=`.
async fn valueset_search(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    let url = param(&params, "url");
    let id = param(&params, "_id").or_else(|| param(&params, "id"));
    let resources: Vec<serde_json::Value> = st
        .registry
        .iter()
        .filter(|vs| url.is_none_or(|u| vs.canonical_url == u))
        .filter(|vs| id.is_none_or(|i| vs.front_matter.id == i))
        .map(|vs| vs.summary_resource())
        .collect();
    fhir_ok(fhir::bundle_searchset(resources))
}

/// `GET /ValueSet/{id}` - the full ValueSet resource (with `compose`).
async fn valueset_read(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    match st.registry.get(&id) {
        Some(vs) => fhir_ok(vs.to_resource()),
        None => fhir_err(FhirError::not_found(format!("ValueSet '{id}' not found"))),
    }
}

/// `GET /ValueSet/{id}/$expand` - expand a stored ValueSet by id.
async fn valueset_expand_id(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    RawQuery(q): RawQuery,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let Some(vs) = st.registry.get(&id) else {
        return fhir_err(FhirError::not_found(format!("ValueSet '{id}' not found")));
    };
    let members = vs.members.clone();
    let params = parse_query(q.as_deref().unwrap_or(""));
    let (count, offset, include_designations) = pagination(&params);
    run_db(&st, move |c| {
        ops::expand_members(c, &members, count, offset, include_designations)
    })
    .await
}

/// `GET|POST /ConceptMap/$translate` - map `code` in `system` to `targetsystem`
/// using the cross-terminology maps (SNOMED CT / ICD-10 / OPCS-4 / CTV3 / Read v2).
async fn translate(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    let Some(code) = param(&params, "code").map(str::to_string) else {
        return fhir_err(FhirError::invalid(
            "`code` parameter is required".to_string(),
        ));
    };
    let Some(system) = param(&params, "system").map(str::to_string) else {
        return fhir_err(FhirError::invalid(
            "`system` parameter is required".to_string(),
        ));
    };
    let Some(target) = param(&params, "targetsystem")
        .or(param(&params, "target"))
        .map(str::to_string)
    else {
        return fhir_err(FhirError::invalid(
            "`targetsystem` parameter is required".to_string(),
        ));
    };
    run_db(&st, move |c| ops::translate(c, &system, &code, &target)).await
}

/// `GET|POST /ValueSet/$validate-code` - is `code` in the ValueSet named by
/// `url` (a stored `.codelist` or an implicit ECL value set)?
async fn vs_validate_code(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    let Some(code) = param(&params, "code").map(str::to_string) else {
        return fhir_err(FhirError::invalid(
            "`code` parameter is required".to_string(),
        ));
    };
    let Some(url) = param(&params, "url").map(str::to_string) else {
        return fhir_err(FhirError::invalid(
            "`url` parameter is required (the ValueSet to validate against)".to_string(),
        ));
    };

    if let Some(vs) = st.registry.resolve_url(&url) {
        let members: std::collections::HashSet<String> =
            vs.members.iter().map(|(id, _)| id.clone()).collect();
        let vs_url = vs.canonical_url.clone();
        return run_db(&st, move |c| {
            ops::validate_code_in_set(c, &members, &code, &vs_url)
        })
        .await;
    }
    if let Some(ecl) = parse_implicit_ecl(&url) {
        return run_db(&st, move |c| ops::validate_code_in_ecl(c, &ecl, &code)).await;
    }
    fhir_err(FhirError::not_found(format!(
        "ValueSet '{url}' not found and not an implicit ECL value set"
    )))
}

/// `POST /` (the FHIR base) - a batch `Bundle` of read operations. Each entry's
/// `request.url` (a GET operation URL) is dispatched against one shared
/// connection, and results come back as a `batch-response` Bundle in the same
/// order - one round trip instead of N. Being read-only, `transaction` Bundles
/// are accepted and treated the same as `batch`.
async fn batch(State(st): State<AppState>, headers: HeaderMap, body: String) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let bundle: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return fhir_err(FhirError::invalid(format!(
                "request body is not valid JSON: {e}"
            )))
        }
    };
    if bundle["resourceType"] != "Bundle" {
        return fhir_err(FhirError::invalid("expected a Bundle resource".to_string()));
    }
    if !matches!(bundle["type"].as_str(), Some("batch") | Some("transaction")) {
        return fhir_err(FhirError::invalid(format!(
            "Bundle.type must be 'batch' (got {:?})",
            bundle["type"].as_str()
        )));
    }
    let entries = bundle["entry"].as_array().cloned().unwrap_or_default();
    let pool = st.pool.clone();
    let registry = st.registry.clone();
    let joined = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, FhirError> {
        pool.with(|conn| {
            let responses: Vec<serde_json::Value> = entries
                .iter()
                .map(|entry| {
                    let method = entry["request"]["method"].as_str().unwrap_or("GET");
                    let url = entry["request"]["url"].as_str().unwrap_or("");
                    let (status, resource) = run_operation(conn, &registry, method, url);
                    serde_json::json!({
                        "response": { "status": status.to_string() },
                        "resource": resource,
                    })
                })
                .collect();
            Ok(fhir::bundle_batch_response(responses))
        })
    })
    .await;
    match joined {
        Ok(Ok(v)) => fhir_ok(v),
        Ok(Err(e)) => fhir_err(e),
        Err(e) => fhir_err(FhirError::exception(format!("internal task error: {e}"))),
    }
}

/// Dispatch one batch entry against an open connection: parse the GET operation
/// URL, route it to the matching op, and return `(http_status, resource)` where
/// `resource` is the operation result or an OperationOutcome. Read-only, so only
/// `GET` entries are supported.
fn run_operation(
    conn: &Connection,
    registry: &ValueSetRegistry,
    method: &str,
    url: &str,
) -> (u16, serde_json::Value) {
    if !method.eq_ignore_ascii_case("GET") {
        let e = FhirError::invalid(format!(
            "batch entries support GET only on this read-only server (got {method:?})"
        ));
        return (e.status, e.outcome());
    }
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let path = path.trim_start_matches('/');
    let params = parse_query(query);

    let result: Result<serde_json::Value, FhirError> = match path {
        "CodeSystem/$lookup" => match param(&params, "code") {
            Some(code) => ops::lookup(conn, code, &params_all(&params, "property")),
            None => Err(FhirError::invalid(
                "missing required parameter 'code'".to_string(),
            )),
        },
        "CodeSystem/$validate-code" => match param(&params, "code") {
            Some(code) => ops::validate_code(conn, code, param(&params, "display")),
            None => Err(FhirError::invalid(
                "missing required parameter 'code'".to_string(),
            )),
        },
        "CodeSystem/$subsumes" => match (param(&params, "codeA"), param(&params, "codeB")) {
            (Some(a), Some(b)) => ops::subsumes(conn, a, b),
            _ => Err(FhirError::invalid(
                "missing required parameters 'codeA' and 'codeB'".to_string(),
            )),
        },
        "ValueSet/$expand" => {
            let (count, offset, desig) = pagination(&params);
            if let Some(vs) = param(&params, "url").and_then(|u| registry.resolve_url(u)) {
                ops::expand_members(conn, &vs.members, count, offset, desig)
            } else {
                let ecl = param(&params, "url").and_then(parse_implicit_ecl);
                ops::expand(
                    conn,
                    ecl.as_deref(),
                    param(&params, "filter"),
                    count,
                    offset,
                    desig,
                )
            }
        }
        "ValueSet/$validate-code" => match (param(&params, "code"), param(&params, "url")) {
            (Some(code), Some(url)) => {
                if let Some(vs) = registry.resolve_url(url) {
                    let members: std::collections::HashSet<String> =
                        vs.members.iter().map(|(id, _)| id.clone()).collect();
                    ops::validate_code_in_set(conn, &members, code, &vs.canonical_url)
                } else if let Some(ecl) = parse_implicit_ecl(url) {
                    ops::validate_code_in_ecl(conn, &ecl, code)
                } else {
                    Err(FhirError::not_found(format!("ValueSet '{url}' not found")))
                }
            }
            _ => Err(FhirError::invalid(
                "missing required parameters 'code' and 'url'".to_string(),
            )),
        },
        "ConceptMap/$translate" => match (
            param(&params, "system"),
            param(&params, "code"),
            param(&params, "targetsystem").or(param(&params, "target")),
        ) {
            (Some(system), Some(code), Some(target)) => ops::translate(conn, system, code, target),
            _ => Err(FhirError::invalid(
                "missing required parameters 'system', 'code', 'targetsystem'".to_string(),
            )),
        },
        other => Err(FhirError::not_found(format!(
            "unsupported batch operation path '{other}'"
        ))),
    };

    match result {
        Ok(v) => (200, v),
        Err(e) => (e.status, e.outcome()),
    }
}

// --- helpers ----------------------------------------------------------------

/// Run a DB operation on a blocking thread with a fresh read-only connection,
/// turning the `Result<Value, FhirError>` into an HTTP response.
async fn run_db<F>(st: &AppState, f: F) -> Response
where
    F: FnOnce(&Connection) -> Result<serde_json::Value, FhirError> + Send + 'static,
{
    let pool = st.pool.clone();
    let joined = tokio::task::spawn_blocking(move || pool.with(|conn| f(conn))).await;
    match joined {
        Ok(Ok(value)) => fhir_ok(value),
        Ok(Err(e)) => fhir_err(e),
        Err(e) => fhir_err(FhirError::exception(format!("internal task error: {e}"))),
    }
}

fn fhir_ok(body: serde_json::Value) -> Response {
    fhir_response(StatusCode::OK, &body)
}

fn fhir_err(e: FhirError) -> Response {
    let status = StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    fhir_response(status, &e.outcome())
}

fn fhir_response(status: StatusCode, body: &serde_json::Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/fhir+json")],
        serde_json::to_string(body).unwrap_or_else(|_| "{}".into()),
    )
        .into_response()
}

/// 406 if the client asks exclusively for XML (not supported).
fn reject_xml(headers: &HeaderMap) -> Option<Response> {
    let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok())?;
    let a = accept.to_lowercase();
    if a.contains("xml") && !a.contains("json") && !a.contains("*/*") {
        Some(fhir_err(FhirError {
            status: 406,
            code: "not-supported",
            diagnostics: "XML is not supported; request application/fhir+json".into(),
        }))
    } else {
        None
    }
}

/// Extract an ECL expression from a FHIR implicit SNOMED ValueSet `url`, e.g.
/// `http://snomed.info/sct?fhir_vs=ecl/<<73211009`. Returns `None` for the
/// "all concepts" form (`?fhir_vs` with no value) or a non-ECL url.
fn parse_implicit_ecl(url: &str) -> Option<String> {
    let after = url.split("fhir_vs=").nth(1)?;
    let after = after.split('&').next().unwrap_or(after);
    let ecl = after.strip_prefix("ecl/")?;
    if ecl.is_empty() {
        None
    } else {
        Some(ecl.to_string())
    }
}

/// Parse a raw query string into key/value pairs, percent-decoding both sides
/// (and `+` → space). Handles repeated keys (FHIR uses `property=` repeatedly).
fn parse_query(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let mut it = pair.splitn(2, '=');
            let k = pct_decode(it.next().unwrap_or(""));
            let v = pct_decode(it.next().unwrap_or(""));
            (k, v)
        })
        .collect()
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Parse the common `count` / `offset` / `includeDesignations` expansion params.
fn pagination(params: &[(String, String)]) -> (usize, usize, bool) {
    let count = param(params, "count")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100usize);
    let offset = param(params, "offset")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0usize);
    let include_designations = param(params, "includeDesignations")
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    (count, offset, include_designations)
}

fn params_all(params: &[(String, String)], key: &str) -> Vec<String> {
    params
        .iter()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .collect()
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(byte) => {
                    out.push(byte);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'%');
                    i += 1;
                }
            },
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repeated_and_encoded_params() {
        let params =
            parse_query("code=22298006&property=parent&property=child&filter=heart+attack");
        assert_eq!(param(&params, "code"), Some("22298006"));
        assert_eq!(params_all(&params, "property"), vec!["parent", "child"]);
        assert_eq!(param(&params, "filter"), Some("heart attack"));
    }

    #[test]
    fn extracts_ecl_from_implicit_url() {
        assert_eq!(
            parse_implicit_ecl("http://snomed.info/sct?fhir_vs=ecl/<<73211009"),
            Some("<<73211009".to_string())
        );
        assert_eq!(parse_implicit_ecl("http://snomed.info/sct?fhir_vs"), None);
        assert_eq!(
            parse_implicit_ecl("http://snomed.info/sct?fhir_vs=ecl/"),
            None
        );
    }

    #[test]
    fn normalises_fhir_base() {
        assert_eq!(normalise_base("/"), "");
        assert_eq!(normalise_base("/fhir"), "/fhir");
        assert_eq!(normalise_base("fhir/"), "/fhir");
    }
}
