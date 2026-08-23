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
    extract::{Path, RawQuery, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use rusqlite::Connection;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::index::query::Index;
use fhir::FhirError;
use pool::ConnectionPool;
use valuesets::ValueSetRegistry;

/// Private page cache per pooled connection (KiB). Modest on purpose: with
/// `mmap_size` set in `open_db_readonly`, reads come from the shared
/// memory-mapped file, so a large per-connection cache would just multiply
/// resident memory across the pool for little benefit.
const POOL_CACHE_KIB: u32 = 8192;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BATCH_ENTRIES: usize = 100;

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
        crate::ecl::warn_if_tct_unusable(&conn, "transitive FHIR hierarchy evaluation")?;
    }

    let addr = format!("{}:{}", args.host, args.port);
    let listener = std::net::TcpListener::bind(&addr).with_context(|| format!("binding {addr}"))?;
    let base = normalise_base(&args.fhir_base);
    if !is_loopback_listener(&listener)? {
        eprintln!(
            "sct serve: WARNING - binding to non-loopback address {} exposes this FHIR \
             server, with no authentication, to anything that can reach this host and port. \
             `$expand` and other operations can be expensive to compute; only bind beyond \
             127.0.0.1/::1/localhost if you have your own network or auth controls in front.",
            args.host
        );
    }
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

/// Resolve the FST index for `/autocomplete`: an explicit `--fst` path, else an
/// index sitting beside the database (`None` if there is none).
///
/// Since `sct fst build` names its index after its input, the sibling is
/// usually `<release>.fst` rather than `snomed.fst`; `find_fst_index` prefers
/// the canonical name and falls back to the newest index in that directory.
fn resolve_fst(explicit: Option<&FsPath>, db: &FsPath) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    crate::paths::find_fst_index(db.parent().unwrap_or(FsPath::new(".")))
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

fn is_loopback_listener(listener: &std::net::TcpListener) -> Result<bool> {
    Ok(listener
        .local_addr()
        .context("reading bound listener address")?
        .ip()
        .is_loopback())
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
        .route("/CodeSystem", get(code_system_search))
        .route("/CodeSystem/{id}", get(code_system_read))
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
        .layer(middleware::from_fn(request_timeout))
        .with_state(state);
    if base.is_empty() {
        app
    } else {
        Router::new().nest(base, app)
    }
}

async fn request_timeout(request: Request, next: Next) -> Response {
    match tokio::time::timeout(REQUEST_TIMEOUT, next.run(request)).await {
        Ok(response) => response,
        Err(_) => fhir_err(FhirError::timeout(format!(
            "request exceeded the {} second limit",
            REQUEST_TIMEOUT.as_secs()
        ))),
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

async fn lookup(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    if let Some(e) = unsupported_lookup_input(&params, &body) {
        return fhir_err(e);
    }
    if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
        return fhir_err(e);
    }
    let Some(code) = param(&params, "code").map(str::to_string) else {
        return fhir_err(FhirError::invalid("missing required parameter 'code'"));
    };
    let version = param(&params, "version").map(str::to_string);
    let props = params_all(&params, "property");
    run_db(&st, move |c| {
        ops::check_lookup_version(c, version.as_deref())?;
        ops::lookup(c, &code, &props)
    })
    .await
}

async fn validate_code(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    if let Some(e) = unsupported_validate_code_input(&params, &body) {
        return fhir_err(e);
    }
    if let Err(e) = ops::check_lookup_system(param(&params, "url")) {
        return fhir_err(e);
    }
    let Some(code) = param(&params, "code").map(str::to_string) else {
        return fhir_err(FhirError::invalid("missing required parameter 'code'"));
    };
    let display = param(&params, "display").map(str::to_string);
    let version = param(&params, "version").map(str::to_string);
    run_db(&st, move |c| {
        ops::check_lookup_version(c, version.as_deref())?;
        ops::validate_code(c, &code, display.as_deref())
    })
    .await
}

async fn subsumes(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    if let Some(e) = unsupported_subsumes_input(&params, &body) {
        return fhir_err(e);
    }
    if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
        return fhir_err(e);
    }
    let (Some(a), Some(b)) = (
        param(&params, "codeA").map(str::to_string),
        param(&params, "codeB").map(str::to_string),
    ) else {
        return fhir_err(FhirError::invalid(
            "missing required parameters 'codeA' and 'codeB'",
        ));
    };
    let version = param(&params, "version").map(str::to_string);
    run_db(&st, move |c| {
        ops::check_lookup_version(c, version.as_deref())?;
        ops::subsumes(c, &a, &b)
    })
    .await
}

/// `GET /CodeSystem` - a searchset Bundle wrapping the single `CodeSystem`
/// resource this server serves (SNOMED CT), optionally filtered by `?url=` or
/// `?_id=` the same way `GET /ValueSet` is. A filter that matches nothing
/// yields an empty Bundle, never a 404 - `CodeSystem` is a search endpoint.
async fn code_system_search(
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
    if url.is_some_and(|u| u != fhir::SNOMED_SYSTEM)
        || id.is_some_and(|i| i != fhir::CODE_SYSTEM_ID)
    {
        return fhir_ok(fhir::bundle_searchset(vec![]));
    }
    run_db(&st, |c| {
        ops::code_system_resource(c).map(|cs| fhir::bundle_searchset(vec![cs]))
    })
    .await
}

/// `GET /CodeSystem/{id}` - the SNOMED CT `CodeSystem` resource metadata (no
/// embedded concept list - see [`fhir::code_system`]'s doc comment).
async fn code_system_read(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    if id != fhir::CODE_SYSTEM_ID {
        return fhir_err(FhirError::not_found(format!("CodeSystem '{id}' not found")));
    }
    run_db(&st, ops::code_system_resource).await
}

async fn expand(
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    if let Some(e) = unsupported_expand_input(&params, &body) {
        return fhir_err(e);
    }
    let (count, offset, include_designations, active_only) = match pagination(&params) {
        Ok(v) => v,
        Err(e) => return fhir_err(e),
    };
    let designation_tokens = params_all(&params, "designation");
    let include_designations = include_designations || !designation_tokens.is_empty();
    let display_language = param(&params, "displayLanguage").map(str::to_string);
    let version_pins = version_pins(&params);
    let include_definition = flag(&params, "includeDefinition");

    // A `url` naming a stored `.codelist` ValueSet expands its member set.
    if let Some(url) = param(&params, "url") {
        if let Some(vs) = st.registry.resolve_url(url) {
            let members = vs.members.clone();
            let definition = include_definition.then(|| vs.to_resource());
            return run_db(&st, move |c| {
                ops::check_system_versions(c, &version_pins)?;
                let mut out = ops::expand_members(
                    c,
                    &members,
                    count,
                    offset,
                    include_designations,
                    display_language.as_deref(),
                )?;
                ops::apply_designation_filter(&mut out, &designation_tokens);
                if let Some(definition) = definition {
                    fhir::attach_definition(&mut out, definition);
                }
                Ok(out)
            })
            .await;
        }
    }

    let ecl = match implicit_ecl_for_expand(param(&params, "url")) {
        Ok(ecl) => ecl,
        Err(e) => return fhir_err(e),
    };
    let filter = param(&params, "filter").map(str::to_string);
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    run_db(&st, move |c| {
        ops::check_system_versions(c, &version_pins)?;
        let mut out = ops::expand(
            c,
            ecl.as_deref(),
            filter.as_deref(),
            count,
            offset,
            include_designations,
            active_only,
            Some(deadline),
            display_language.as_deref(),
        )?;
        ops::apply_designation_filter(&mut out, &designation_tokens);
        if include_definition {
            fhir::attach_definition(&mut out, fhir::implicit_valueset_definition(ecl.as_deref()));
        }
        Ok(out)
    })
    .await
}

/// `GET /ValueSet` - a searchset Bundle of the registered ValueSets (metadata
/// only), optionally filtered by `?url=`, `?_id=`, and/or `?status=` (the
/// FHIR `draft` | `active` | `retired` | `unknown` value set, matching each
/// list's front-matter `status` through
/// [`crate::commands::codelist::fhir_status`]).
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
    let status = param(&params, "status");
    let resources: Vec<serde_json::Value> = st
        .registry
        .iter()
        .filter(|vs| url.is_none_or(|u| vs.canonical_url == u))
        .filter(|vs| id.is_none_or(|i| vs.front_matter.id == i))
        .filter(|vs| {
            status.is_none_or(|s| {
                crate::commands::codelist::fhir_status(&vs.front_matter.status) == s
            })
        })
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
    // `activeOnly` is intentionally unused here - see `pagination`'s doc comment.
    let (count, offset, include_designations, _active_only) = match pagination(&params) {
        Ok(v) => v,
        Err(e) => return fhir_err(e),
    };
    let designation_tokens = params_all(&params, "designation");
    let include_designations = include_designations || !designation_tokens.is_empty();
    let display_language = param(&params, "displayLanguage").map(str::to_string);
    let version_pins = version_pins(&params);
    let definition = flag(&params, "includeDefinition").then(|| vs.to_resource());
    run_db(&st, move |c| {
        ops::check_system_versions(c, &version_pins)?;
        let mut out = ops::expand_members(
            c,
            &members,
            count,
            offset,
            include_designations,
            display_language.as_deref(),
        )?;
        ops::apply_designation_filter(&mut out, &designation_tokens);
        if let Some(definition) = definition {
            fhir::attach_definition(&mut out, definition);
        }
        Ok(out)
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
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let params = parse_query(q.as_deref().unwrap_or(""));
    if let Some(e) = unsupported_vs_validate_code_input(&params, &body) {
        return fhir_err(e);
    }
    if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
        return fhir_err(e);
    }
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
    let display = param(&params, "display").map(str::to_string);
    let system_version = param(&params, "systemVersion").map(str::to_string);

    if let Some(vs) = st.registry.resolve_url(&url) {
        let members: std::collections::HashSet<String> =
            vs.members.iter().map(|(id, _)| id.clone()).collect();
        let vs_url = vs.canonical_url.clone();
        return run_db(&st, move |c| {
            ops::check_lookup_version(c, system_version.as_deref())?;
            ops::validate_code_in_set(c, &members, &code, &vs_url, display.as_deref())
        })
        .await;
    }
    if let Some(ecl) = parse_implicit_ecl(&url) {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        return run_db(&st, move |c| {
            ops::check_lookup_version(c, system_version.as_deref())?;
            ops::validate_code_in_ecl(c, &ecl, &code, Some(deadline), display.as_deref())
        })
        .await;
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
    if entries.len() > MAX_BATCH_ENTRIES {
        return fhir_err(FhirError::invalid(format!(
            "Bundle.entry contains {} entries; the maximum is {MAX_BATCH_ENTRIES}",
            entries.len()
        )));
    }
    let pool = st.pool.clone();
    let registry = st.registry.clone();
    // One deadline for the whole batch (one HTTP request, one 30s budget) -
    // not recomputed per entry, so entries later in the Bundle don't each get
    // a fresh 30 seconds.
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let joined = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, FhirError> {
        pool.with(|conn| {
            let responses: Vec<serde_json::Value> = entries
                .iter()
                .map(|entry| {
                    let method = entry["request"]["method"].as_str().unwrap_or("GET");
                    let url = entry["request"]["url"].as_str().unwrap_or("");
                    let (status, resource) = run_operation(conn, &registry, method, url, deadline);
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
    deadline: Instant,
) -> (u16, serde_json::Value) {
    if !method.eq_ignore_ascii_case("GET") {
        let e = FhirError::invalid(format!(
            "batch entries support GET only on this read-only server (got {method:?})"
        ));
        return (e.status, e.outcome());
    }
    // A FHIR batch entry's operation URL may contain a percent-encoded implicit
    // ValueSet URL (`url=http%3A...%3Ffhir_vs...`). Split before decoding so
    // the inner `?` remains part of the `url` parameter, not a second route
    // delimiter.
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let path = path.trim_start_matches('/');
    let params = parse_query(query);

    let result: Result<serde_json::Value, FhirError> = match path {
        "CodeSystem/$lookup" => {
            if let Some(e) = unsupported_lookup_input(&params, "") {
                return (e.status, e.outcome());
            }
            if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
                return (e.status, e.outcome());
            }
            match param(&params, "code") {
                Some(code) => ops::check_lookup_version(conn, param(&params, "version"))
                    .and_then(|()| ops::lookup(conn, code, &params_all(&params, "property"))),
                None => Err(FhirError::invalid(
                    "missing required parameter 'code'".to_string(),
                )),
            }
        }
        "CodeSystem/$validate-code" => {
            if let Some(e) = unsupported_validate_code_input(&params, "") {
                return (e.status, e.outcome());
            }
            if let Err(e) = ops::check_lookup_system(param(&params, "url")) {
                return (e.status, e.outcome());
            }
            match param(&params, "code") {
                Some(code) => ops::check_lookup_version(conn, param(&params, "version"))
                    .and_then(|()| ops::validate_code(conn, code, param(&params, "display"))),
                None => Err(FhirError::invalid(
                    "missing required parameter 'code'".to_string(),
                )),
            }
        }
        "CodeSystem/$subsumes" => {
            if let Some(e) = unsupported_subsumes_input(&params, "") {
                return (e.status, e.outcome());
            }
            if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
                return (e.status, e.outcome());
            }
            match (param(&params, "codeA"), param(&params, "codeB")) {
                (Some(a), Some(b)) => ops::check_lookup_version(conn, param(&params, "version"))
                    .and_then(|()| ops::subsumes(conn, a, b)),
                _ => Err(FhirError::invalid(
                    "missing required parameters 'codeA' and 'codeB'".to_string(),
                )),
            }
        }
        "ValueSet/$expand" => {
            let (count, offset, desig, active_only) = match pagination(&params) {
                Ok(v) => v,
                Err(e) => return (e.status, e.outcome()),
            };
            let designation_tokens = params_all(&params, "designation");
            let desig = desig || !designation_tokens.is_empty();
            let display_language = param(&params, "displayLanguage");
            if let Some(e) = unsupported_expand_input(&params, "") {
                return (e.status, e.outcome());
            }
            if let Err(e) = ops::check_system_versions(conn, &version_pins(&params)) {
                return (e.status, e.outcome());
            }
            let include_definition = flag(&params, "includeDefinition");
            if let Some(vs) = param(&params, "url").and_then(|u| registry.resolve_url(u)) {
                ops::expand_members(conn, &vs.members, count, offset, desig, display_language).map(
                    |mut out| {
                        ops::apply_designation_filter(&mut out, &designation_tokens);
                        if include_definition {
                            fhir::attach_definition(&mut out, vs.to_resource());
                        }
                        out
                    },
                )
            } else {
                let ecl = match implicit_ecl_for_expand(param(&params, "url")) {
                    Ok(ecl) => ecl,
                    Err(e) => return (e.status, e.outcome()),
                };
                ops::expand(
                    conn,
                    ecl.as_deref(),
                    param(&params, "filter"),
                    count,
                    offset,
                    desig,
                    active_only,
                    Some(deadline),
                    display_language,
                )
                .map(|mut out| {
                    ops::apply_designation_filter(&mut out, &designation_tokens);
                    if include_definition {
                        fhir::attach_definition(
                            &mut out,
                            fhir::implicit_valueset_definition(ecl.as_deref()),
                        );
                    }
                    out
                })
            }
        }
        "ValueSet/$validate-code" => {
            if let Some(e) = unsupported_vs_validate_code_input(&params, "") {
                return (e.status, e.outcome());
            }
            if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
                return (e.status, e.outcome());
            }
            match (param(&params, "code"), param(&params, "url")) {
                (Some(code), Some(url)) => {
                    ops::check_lookup_version(conn, param(&params, "systemVersion")).and_then(
                        |()| {
                            if let Some(vs) = registry.resolve_url(url) {
                                let members: std::collections::HashSet<String> =
                                    vs.members.iter().map(|(id, _)| id.clone()).collect();
                                ops::validate_code_in_set(
                                    conn,
                                    &members,
                                    code,
                                    &vs.canonical_url,
                                    param(&params, "display"),
                                )
                            } else if let Some(ecl) = parse_implicit_ecl(url) {
                                ops::validate_code_in_ecl(
                                    conn,
                                    &ecl,
                                    code,
                                    Some(deadline),
                                    param(&params, "display"),
                                )
                            } else {
                                Err(FhirError::not_found(format!("ValueSet '{url}' not found")))
                            }
                        },
                    )
                }
                _ => Err(FhirError::invalid(
                    "missing required parameters 'code' and 'url'".to_string(),
                )),
            }
        }
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
    match parse_implicit(url) {
        Some(ImplicitValueSet::Ecl(ecl)) => Some(ecl),
        _ => None,
    }
}

/// The SNOMED CT implicit value set named by an `$expand` `url`.
#[derive(Debug, PartialEq, Eq)]
enum ImplicitValueSet {
    /// `?fhir_vs` - every concept in the loaded edition.
    All,
    /// A form answered by evaluating ECL. The R4 SNOMED CT page defines
    /// `?fhir_vs=isa/[sctid]` as "all concept ids that have a transitive is-a
    /// relationship with [sctid], including the concept itself" (`<<[sctid]`)
    /// and `?fhir_vs=refset/[sctid]` as that reference set's active members
    /// (`^[sctid]`), so both reduce to the ECL engine already in place.
    Ecl(String),
    /// A `fhir_vs` form this server does not implement. Named so the client
    /// gets told, rather than silently handed a different value set.
    Unsupported(String),
}

/// Classify an `$expand` `url` as a SNOMED CT implicit value set.
///
/// `None` means "not a SNOMED implicit value set URL at all", which the caller
/// must treat as an unknown value set. Returning the whole code system for an
/// unrecognised URL - the previous behaviour - is a silent wrong answer: a
/// client asking for the descendants of one concept received every concept in
/// the edition, with a 200 and no indication anything had been substituted.
fn parse_implicit(url: &str) -> Option<ImplicitValueSet> {
    if !url.contains("fhir_vs") {
        return None;
    }
    let Some(after) = url.split("fhir_vs=").nth(1) else {
        // Bare `?fhir_vs`, with no value: the whole code system.
        return Some(ImplicitValueSet::All);
    };
    let after = after.split('&').next().unwrap_or(after);
    if after.is_empty() {
        return Some(ImplicitValueSet::All);
    }

    let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_string());
    if let Some(ecl) = after.strip_prefix("ecl/") {
        return Some(match non_empty(ecl) {
            Some(ecl) => ImplicitValueSet::Ecl(ecl),
            None => ImplicitValueSet::Unsupported(after.to_string()),
        });
    }
    if let Some(sctid) = after.strip_prefix("isa/") {
        return Some(match non_empty(sctid) {
            Some(sctid) => ImplicitValueSet::Ecl(format!("<<{sctid}")),
            None => ImplicitValueSet::Unsupported(after.to_string()),
        });
    }
    if let Some(sctid) = after.strip_prefix("refset/") {
        return Some(match non_empty(sctid) {
            Some(sctid) => ImplicitValueSet::Ecl(format!("^{sctid}")),
            None => ImplicitValueSet::Unsupported(after.to_string()),
        });
    }
    // `?fhir_vs=refset` (the set of reference sets) is a distinct query rather
    // than an ECL expression, and is not implemented; say so.
    Some(ImplicitValueSet::Unsupported(after.to_string()))
}

/// Resolve an `$expand` `url` to the ECL to evaluate, or an error explaining
/// why it cannot be expanded. `Ok(None)` means "expand the whole code system".
fn implicit_ecl_for_expand(url: Option<&str>) -> Result<Option<String>, FhirError> {
    let Some(url) = url else {
        // No `url` at all: a bare/`filter`-only expansion over the code system.
        return Ok(None);
    };
    match parse_implicit(url) {
        Some(ImplicitValueSet::All) => Ok(None),
        Some(ImplicitValueSet::Ecl(ecl)) => Ok(Some(ecl)),
        Some(ImplicitValueSet::Unsupported(form)) => Err(FhirError::invalid(format!(
            "implicit SNOMED CT value set `fhir_vs={form}` is not supported; \
             use `fhir_vs`, `fhir_vs=ecl/[ecl]`, `fhir_vs=isa/[sctid]`, or `fhir_vs=refset/[sctid]`"
        ))),
        None => Err(FhirError::not_found(format!("ValueSet '{url}' not found"))),
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

const DEFAULT_EXPANSION_COUNT: usize = 100;
const MAX_EXPANSION_COUNT: usize = 1000;

fn pagination_usize(
    params: &[(String, String)],
    key: &str,
    default: usize,
) -> Result<usize, FhirError> {
    match param(params, key) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| FhirError::invalid(format!("`{key}` must be a non-negative integer"))),
    }
}

/// Parse and validate the common `count` / `offset` / `includeDesignations` /
/// `activeOnly` expansion params. `count` is capped centrally so all HTTP and
/// batch routes report the same effective page size; `ops` retains its own
/// cap as defence in depth. `activeOnly` defaults to true, per
/// `spec/commands/serve.md`; it applies only to the implicit SNOMED
/// CodeSystem expansion (`ops::expand`) - a stored `.codelist` ValueSet's
/// fixed member list (`ops::expand_members`) does not honour it yet.
fn pagination(params: &[(String, String)]) -> Result<(usize, usize, bool, bool), FhirError> {
    let count =
        pagination_usize(params, "count", DEFAULT_EXPANSION_COUNT)?.min(MAX_EXPANSION_COUNT);
    let offset = pagination_usize(params, "offset", 0)?;
    if i64::try_from(offset).is_err() {
        return Err(FhirError::invalid("`offset` is too large"));
    }
    let include_designations = param(params, "includeDesignations")
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let active_only = param(params, "activeOnly")
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(true);
    Ok((count, offset, include_designations, active_only))
}

/// The versions an `$expand` request requires the SNOMED CT system to be at.
///
/// `check-system-version` asserts a version outright. `system-version` supplies
/// one "if the value set does not specify which one to use" - and an implicit
/// SNOMED value set never does, so for this server the two amount to the same
/// requirement, and both must be checked rather than one silently ignored.
fn version_pins(params: &[(String, String)]) -> Vec<String> {
    let mut pins = params_all(params, "check-system-version");
    pins.extend(params_all(params, "system-version"));
    pins
}

/// Refuse an `$expand` request this server cannot honour, instead of quietly
/// expanding something else.
///
/// FHIR's standard invocation for `$expand` is a POST carrying a `Parameters`
/// resource, which is also the only way to supply an inline `valueSet`
/// definition. `sct` reads parameters from the query string only, so a body
/// would be silently discarded - and a discarded value set definition left
/// `$expand` with nothing to expand, which it read as "the whole code system".
/// Answering a request for two concepts with every concept in the edition is a
/// wrong answer, not graceful degradation.
///
/// The named parameters are those whose whole purpose is to *narrow or
/// redirect* the expansion, so ignoring one silently broadens the result. R4
/// explicitly sanctions refusing several of them - `date`, for instance, says
/// the server should honour it "or return an error if this is not possible".
///
/// `excludeNested`, `excludeNotForUI`, and `excludePostCoordinated` are
/// deliberately absent from this list: this server's expansions are always
/// flat, never emit navigation-only entries, and never emit post-coordinated
/// codes, so accepting and ignoring them cannot narrow or broaden anything -
/// see the `CannotAffectResult` dispositions in `tests/fhir_conformance.rs`.
fn unsupported_expand_input(params: &[(String, String)], body: &str) -> Option<FhirError> {
    if !body.trim().is_empty() {
        return Some(FhirError::invalid(
            "this server reads $expand parameters from the query string; a request body \
             (including an inline `valueSet` definition) is not supported",
        ));
    }
    const UNSUPPORTED: [(&str, &str); 6] = [
        ("valueSet", "supply a value set by `url` instead"),
        (
            "valueSetVersion",
            "this server serves whichever version of a stored value set is on disk and \
             cannot select another; implicit SNOMED CT value sets have no version of their own",
        ),
        ("context", "resolve the binding yourself and pass `url`"),
        (
            "date",
            "this server holds a single release and cannot expand as at a past date",
        ),
        (
            "exclude-system",
            "this server serves SNOMED CT only, so excluding a system cannot be honoured",
        ),
        (
            "force-system-version",
            "this server holds a single release and cannot override its version",
        ),
    ];
    UNSUPPORTED.iter().find_map(|(name, hint)| {
        param(params, name).map(|_| {
            FhirError::invalid(format!(
                "`{name}` is not supported by this server: {hint}. It is refused rather than \
                 ignored, because ignoring it would silently widen the expansion"
            ))
        })
    })
}

/// Refuse a `$lookup` request this server cannot honour, instead of quietly
/// answering with different input (roadmap `R17b-lookup`).
///
/// `coding` is R4's alternative to separate `system`/`code` parameters: a
/// structured `Coding` datatype, standardly supplied inside a POST
/// `Parameters` body. `sct` reads `$lookup` parameters from the query string
/// only (see the body check below), so it is refused by name rather than
/// silently discarded even if a client sends it as a bare query token - the
/// same shape of bug `$expand`'s `valueSet` handling was fixed for under R17.
/// `date` asks for concept information as of a historical point in time,
/// which this single-release server cannot serve, matching `$expand`'s
/// `date` handling.
fn unsupported_lookup_input(params: &[(String, String)], body: &str) -> Option<FhirError> {
    if !body.trim().is_empty() {
        return Some(FhirError::invalid(
            "this server reads $lookup parameters from the query string; a request body \
             (including a `coding` Parameters part) is not supported - supply `system` and \
             `code` instead",
        ));
    }
    const UNSUPPORTED: [(&str, &str); 2] = [
        (
            "coding",
            "supply separate `system` and `code` query parameters instead",
        ),
        (
            "date",
            "this server holds a single release and cannot look up concept information as at \
             a past date",
        ),
    ];
    UNSUPPORTED.iter().find_map(|(name, hint)| {
        param(params, name).map(|_| {
            FhirError::invalid(format!(
                "`{name}` is not supported by this server: {hint}. It is refused rather than \
                 ignored, because ignoring it would silently answer using different input"
            ))
        })
    })
}

/// Refuse a `CodeSystem/$subsumes` request this server cannot honour, instead
/// of quietly answering with different input (roadmap `R17b-subsumes`).
/// `codingA`/`codingB` are structured `Coding` datatypes that may name a
/// different code system than `system` - honouring them silently would let
/// the operation compare across systems this server has no map for; this
/// server reads query-string `codeA`/`codeB` SCTIDs only (see the body check
/// below), so each is refused by name rather than silently discarded.
fn unsupported_subsumes_input(params: &[(String, String)], body: &str) -> Option<FhirError> {
    if !body.trim().is_empty() {
        return Some(FhirError::invalid(
            "this server reads $subsumes parameters from the query string; a request body \
             (including an inline `codingA` or `codingB` Parameters part) is not supported - \
             supply `codeA` and `codeB` instead",
        ));
    }
    const UNSUPPORTED: [(&str, &str); 2] = [
        (
            "codingA",
            "supply a separate `codeA` query parameter instead",
        ),
        (
            "codingB",
            "supply a separate `codeB` query parameter instead",
        ),
    ];
    UNSUPPORTED.iter().find_map(|(name, hint)| {
        param(params, name).map(|_| {
            FhirError::invalid(format!(
                "`{name}` is not supported by this server: {hint}. It is refused rather than \
                 ignored, because ignoring it would silently answer using different input"
            ))
        })
    })
}

/// Refuse a `CodeSystem/$validate-code` request this server cannot honour,
/// instead of quietly answering with different input (roadmap
/// `R17b-validate-code`). `codeSystem`, `coding`, and `codeableConcept` are
/// all structured datatypes normally supplied inline or in a POST body; this
/// server reads query-string parameters only (see the body check below), so
/// each is refused by name rather than silently discarded. `date` matches
/// `$lookup` and `$expand`'s handling: a single-release server cannot
/// validate as of a past point in time.
fn unsupported_validate_code_input(params: &[(String, String)], body: &str) -> Option<FhirError> {
    if !body.trim().is_empty() {
        return Some(FhirError::invalid(
            "this server reads $validate-code parameters from the query string; a request \
             body (including an inline `codeSystem`, `coding`, or `codeableConcept`) is not \
             supported - supply `url` and `code` instead",
        ));
    }
    const UNSUPPORTED: [(&str, &str); 4] = [
        (
            "codeSystem",
            "this server validates only against its own loaded SNOMED CT release; an inline \
             code system cannot be honoured",
        ),
        (
            "coding",
            "supply separate `url` (or omit it) and `code` query parameters instead",
        ),
        (
            "codeableConcept",
            "supply separate `url` (or omit it) and `code` query parameters instead",
        ),
        (
            "date",
            "this server holds a single release and cannot validate a code as at a past date",
        ),
    ];
    UNSUPPORTED.iter().find_map(|(name, hint)| {
        param(params, name).map(|_| {
            FhirError::invalid(format!(
                "`{name}` is not supported by this server: {hint}. It is refused rather than \
                 ignored, because ignoring it would silently answer using different input"
            ))
        })
    })
}

/// Refuse a `ValueSet/$validate-code` request this server cannot honour
/// (roadmap `R17b-validate-code`), the same treatment
/// [`unsupported_validate_code_input`] gives the `CodeSystem` form:
/// `valueSet`/`valueSetVersion`/`context` mirror `$expand`'s refusal of an
/// inline definition, a pinned version of a stored value set, and an
/// unresolved binding context; `coding`/`codeableConcept`/`date` mirror
/// `$lookup` and the `CodeSystem` form's refusal of structured POST-body
/// datatypes and historical-date validation.
fn unsupported_vs_validate_code_input(
    params: &[(String, String)],
    body: &str,
) -> Option<FhirError> {
    if !body.trim().is_empty() {
        return Some(FhirError::invalid(
            "this server reads $validate-code parameters from the query string; a request \
             body (including an inline `valueSet`, `coding`, or `codeableConcept`) is not \
             supported - supply `url` and `code` instead",
        ));
    }
    const UNSUPPORTED: [(&str, &str); 6] = [
        ("valueSet", "supply a value set by `url` instead"),
        (
            "valueSetVersion",
            "this server serves whichever version of a stored value set is on disk and \
             cannot select another; implicit SNOMED CT value sets have no version of their own",
        ),
        ("context", "resolve the binding yourself and pass `url`"),
        (
            "coding",
            "supply separate `system` and `code` query parameters instead",
        ),
        (
            "codeableConcept",
            "supply separate `system` and `code` query parameters instead",
        ),
        (
            "date",
            "this server holds a single release and cannot validate a code as at a past date",
        ),
    ];
    UNSUPPORTED.iter().find_map(|(name, hint)| {
        param(params, name).map(|_| {
            FhirError::invalid(format!(
                "`{name}` is not supported by this server: {hint}. It is refused rather than \
                 ignored, because ignoring it would silently answer using different input"
            ))
        })
    })
}

/// A boolean `$expand` flag, defaulting to false when absent.
fn flag(params: &[(String, String)], key: &str) -> bool {
    param(params, key)
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
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
    fn parses_encoded_implicit_valueset_url_without_splitting_its_query() {
        let params = parse_query(
            "url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs%3Decl%2F22298006&includeDesignations=true",
        );
        assert_eq!(
            param(&params, "url"),
            Some("http://snomed.info/sct?fhir_vs=ecl/22298006")
        );
        assert_eq!(
            parse_implicit_ecl(param(&params, "url").unwrap()),
            Some("22298006".to_string())
        );
    }

    /// Every one of these narrows or redirects an expansion, so ignoring one
    /// silently *widens* the result - the failure mode that made a POST with a
    /// two-concept inline value set return the whole code system.
    #[test]
    fn expand_refuses_inputs_it_would_otherwise_silently_ignore() {
        let body_refused = unsupported_expand_input(&[], r#"{"resourceType":"Parameters"}"#)
            .expect("a request body must not be silently discarded");
        assert_eq!(body_refused.status, 400);
        assert!(body_refused.diagnostics.contains("valueSet"));

        for name in [
            "valueSet",
            "context",
            "date",
            "exclude-system",
            "force-system-version",
        ] {
            let refused = unsupported_expand_input(&parse_query(&format!("{name}=x")), "")
                .unwrap_or_else(|| panic!("`{name}` must be refused, not ignored"));
            assert_eq!(refused.status, 400);
            assert!(refused.diagnostics.contains(name));
        }

        // Whitespace-only bodies are what a plain GET looks like; supported
        // parameters must still pass straight through.
        assert!(unsupported_expand_input(&[], "  \n ").is_none());
        assert!(unsupported_expand_input(
            &parse_query("url=http://snomed.info/sct?fhir_vs&count=10&activeOnly=false"),
            "",
        )
        .is_none());
    }

    /// `system-version` supplies a version when the value set does not specify
    /// one, which an implicit SNOMED value set never does - so it carries the
    /// same force here as `check-system-version` and must be checked too.
    #[test]
    fn version_pins_cover_both_spellings() {
        let pins = version_pins(&parse_query(
            "check-system-version=http://snomed.info/sct|a&system-version=http://snomed.info/sct|b",
        ));
        assert_eq!(
            pins,
            vec!["http://snomed.info/sct|a", "http://snomed.info/sct|b"]
        );
        assert!(version_pins(&[]).is_empty());
    }

    #[test]
    fn classifies_every_implicit_valueset_form() {
        use ImplicitValueSet::*;
        let p = |u: &str| parse_implicit(u);
        let sct = "http://snomed.info/sct";

        assert_eq!(p(&format!("{sct}?fhir_vs")), Some(All));
        assert_eq!(p(&format!("{sct}?fhir_vs=")), Some(All));
        assert_eq!(
            p(&format!("{sct}?fhir_vs=ecl/<<73211009")),
            Some(Ecl("<<73211009".into()))
        );
        // `isa` and `refset` are defined by the SNOMED CT R4 page and reduce to
        // ECL; before this they fell through to the whole code system.
        assert_eq!(
            p(&format!("{sct}?fhir_vs=isa/73211009")),
            Some(Ecl("<<73211009".into()))
        );
        assert_eq!(
            p(&format!("{sct}?fhir_vs=refset/900000000000497000")),
            Some(Ecl("^900000000000497000".into()))
        );

        // Defined but not implemented, and malformed forms: named, not guessed.
        assert_eq!(
            p(&format!("{sct}?fhir_vs=refset")),
            Some(Unsupported("refset".into()))
        );
        assert_eq!(
            p(&format!("{sct}?fhir_vs=ecl/")),
            Some(Unsupported("ecl/".into()))
        );
        assert_eq!(
            p(&format!("{sct}?fhir_vs=nonsense/1")),
            Some(Unsupported("nonsense/1".into()))
        );

        // Not a SNOMED implicit value set URL at all.
        assert_eq!(p("http://example.org/ValueSet/nope"), None);
    }

    /// An `$expand` URL the server does not recognise must never quietly
    /// become "the whole code system".
    #[test]
    fn unrecognised_expand_urls_fail_instead_of_expanding_everything() {
        assert_eq!(implicit_ecl_for_expand(None).unwrap(), None);
        assert_eq!(
            implicit_ecl_for_expand(Some("http://snomed.info/sct?fhir_vs")).unwrap(),
            None
        );
        assert_eq!(
            implicit_ecl_for_expand(Some("http://snomed.info/sct?fhir_vs=isa/73211009")).unwrap(),
            Some("<<73211009".to_string())
        );

        let unknown = implicit_ecl_for_expand(Some("http://example.org/ValueSet/nope"))
            .expect_err("an unknown value set must not expand");
        assert_eq!(unknown.status, 404);

        let unsupported = implicit_ecl_for_expand(Some("http://snomed.info/sct?fhir_vs=refset"))
            .expect_err("an unimplemented form must not expand");
        assert_eq!(unsupported.status, 400);
        assert!(unsupported.diagnostics.contains("refset"));
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
    fn recognises_bound_loopback_addresses() {
        let ipv4 = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        assert!(is_loopback_listener(&ipv4).unwrap());

        if let Ok(ipv6) = std::net::TcpListener::bind("[::1]:0") {
            assert!(is_loopback_listener(&ipv6).unwrap());
        }
    }

    #[test]
    fn rejects_bound_non_loopback_addresses() {
        let listener = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
        assert!(!is_loopback_listener(&listener).unwrap());
    }

    #[test]
    fn normalises_fhir_base() {
        assert_eq!(normalise_base("/"), "");
        assert_eq!(normalise_base("/fhir"), "/fhir");
        assert_eq!(normalise_base("fhir/"), "/fhir");
    }

    #[test]
    fn pagination_defaults_caps_and_parses() {
        assert_eq!(pagination(&[]).unwrap(), (100, 0, false, true));
        assert_eq!(
            pagination(&parse_query(
                "count=5000&offset=42&includeDesignations=true&activeOnly=false"
            ))
            .unwrap(),
            (1000, 42, true, false)
        );
    }

    #[test]
    fn pagination_rejects_malformed_numbers() {
        for query in ["count=nope", "count=-1", "offset=nope", "offset=-1"] {
            assert!(pagination(&parse_query(query)).is_err(), "accepted {query}");
        }
    }
}
