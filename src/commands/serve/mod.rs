// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct serve` - a FHIR R4 terminology server over the SQLite artefact.
//!
//! Phase 1: `/metadata` (CapabilityStatement), `CodeSystem/$lookup`,
//! `$validate-code`, `$subsumes`, and `ValueSet/$expand` (text filter + the
//! documented ECL subset via [`crate::ecl`]). See `spec/commands/serve.md`. The operation logic
//! lives in [`ops`] as pure functions; the handlers here are thin transport.

pub mod fhir;
pub mod ops;
pub mod pool;
pub mod valuesets;

use anyhow::{bail, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Path, RawQuery, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
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
use url::Url;

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

    /// Externally reachable absolute FHIR base URL used in generated resource
    /// URLs and metadata. Include the public base path, for example
    /// `https://fhir.example.org/fhir`. Defaults to the bound listener URL.
    #[arg(long, value_name = "URL")]
    pub public_url: Option<String>,

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

    /// Send CORS headers for this origin (repeatable), so a browser page
    /// served from it can call this server directly. Matched exactly against
    /// the request `Origin`; the matching origin is echoed back with `Vary:
    /// Origin`, never a wildcard. Pass the literal `*` to opt in to
    /// `Access-Control-Allow-Origin: *` for every origin. Omitted entirely by
    /// default: no CORS headers and no preflight handling at all, unchanged
    /// from before this flag existed - real deployments front `sct serve`
    /// with the Caddy layer (`Caddyfile`, `CORS_ORIGINS`), which already
    /// supplies CORS; this flag is for reaching the server directly from a
    /// browser.
    #[arg(long = "cors-origin", value_name = "ORIGIN")]
    pub cors_origins: Vec<String>,
}

#[derive(Clone)]
struct AppState {
    /// Warm pool of read-only connections shared by every DB-backed operation.
    pool: Arc<ConnectionPool>,
    /// Immutable release metadata loaded once when the pool opens.
    release_version: Option<Arc<str>>,
    /// TCT capability validated once when the read-only pool opens.
    tct_available: bool,
    impl_url: Arc<String>,
    registry: Arc<ValueSetRegistry>,
    translate_available: bool,
    /// FST index backing `/autocomplete`, if one was supplied/discovered.
    fst: Option<Arc<Index>>,
    /// `--cors-origin` values, verbatim. Empty means CORS is off: no headers,
    /// no preflight handling - see [`cors_middleware`].
    cors_origins: Arc<Vec<String>>,
}

/// Route and externally advertised base URLs for [`serve_listener_with_public_url`].
#[doc(hidden)]
pub struct ServeListenerUrls<'a> {
    pub fhir_base: &'a str,
    pub public_url: Option<&'a str>,
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
    let loopback = is_loopback_listener(&listener)?;
    if !loopback {
        eprintln!(
            "sct serve: WARNING - binding to non-loopback address {} exposes this FHIR \
             server, with no authentication, to anything that can reach this host and port. \
             `$expand` and other operations can be expensive to compute; only bind beyond \
             127.0.0.1/::1/localhost if you have your own network or auth controls in front.",
            args.host
        );
        if should_warn_about_wildcard_cors(loopback, &args.cors_origins) {
            eprintln!(
                "sct serve: WARNING - `--cors-origin *` combined with a non-loopback bind lets \
                 any web page anyone visits call this unauthenticated server directly; prefer \
                 naming specific origins unless that is really intended."
            );
        }
    }
    eprintln!(
        "sct serve: FHIR R4 terminology server on http://{addr}{base}\n  database: {}\n  try: curl 'http://{addr}{base}/metadata'",
        db.display()
    );
    let codelists = crate::paths::codelist_registry(args.codelists.as_deref());
    let fst = resolve_fst(args.fst.as_deref(), &db);
    serve_listener_with_public_url(
        db,
        ServeListenerUrls {
            fhir_base: &args.fhir_base,
            public_url: args.public_url.as_deref(),
        },
        Some(codelists),
        fst,
        args.pool_size,
        args.cors_origins,
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
    cors_origins: Vec<String>,
    listener: std::net::TcpListener,
) -> Result<()> {
    serve_listener_with_public_url(
        db,
        ServeListenerUrls {
            fhir_base,
            public_url: None,
        },
        codelists,
        fst,
        pool_size,
        cors_origins,
        listener,
    )
}

/// Variant of [`serve_listener`] with an explicit externally advertised FHIR
/// base URL. Production deployments behind a reverse proxy need this because a
/// bound wildcard or container address is not a client-reachable resource URL.
#[doc(hidden)]
pub fn serve_listener_with_public_url(
    db: PathBuf,
    urls: ServeListenerUrls<'_>,
    codelists: Option<PathBuf>,
    fst: Option<PathBuf>,
    pool_size: usize,
    cors_origins: Vec<String>,
    listener: std::net::TcpListener,
) -> Result<()> {
    let base = normalise_base(urls.fhir_base);
    let addr = listener.local_addr().context("listener address")?;
    let impl_url = advertised_url(urls.public_url, addr, &base)?;
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
    let tct_available = pool.with(crate::ecl::eval::has_tct)?;
    let release_version = pool.with(ops::release_version).map(Arc::from);
    eprintln!("  connection pool: {pool_size} warm read-only connection(s)");
    let state = AppState {
        translate_available,
        pool: Arc::new(pool),
        release_version,
        tct_available,
        impl_url: Arc::new(impl_url),
        registry: Arc::new(registry),
        fst: fst_index,
        cors_origins: Arc::new(cors_origins),
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

fn should_warn_about_wildcard_cors(loopback: bool, cors_origins: &[String]) -> bool {
    !loopback && cors_origins.iter().any(|origin| origin == "*")
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

fn advertised_url(
    public_url: Option<&str>,
    addr: std::net::SocketAddr,
    fhir_base: &str,
) -> Result<String> {
    let Some(public_url) = public_url else {
        return Ok(format!("http://{addr}{fhir_base}"));
    };
    let public_url = public_url.trim_end_matches('/');
    let uri: Uri = public_url
        .parse()
        .with_context(|| format!("invalid --public-url {public_url:?}"))?;
    let parsed =
        Url::parse(public_url).with_context(|| format!("invalid --public-url {public_url:?}"))?;
    let scheme = parsed.scheme();
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        || uri.authority().is_none()
        || parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port() == Some(0)
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || has_invalid_percent_escape(uri.path())
        || uri.path().split('/').any(is_url_dot_segment)
    {
        bail!(
            "invalid --public-url {public_url:?}: expected an absolute http(s) FHIR base URL with a host, valid optional port, and no credentials, query, fragment, or dot path segments"
        );
    }
    Ok(public_url.to_string())
}

fn has_invalid_percent_escape(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if !bytes
                .get(i + 1..i + 3)
                .is_some_and(|escape| escape.iter().all(u8::is_ascii_hexdigit))
            {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

fn is_url_dot_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let mut dots = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'.' {
            dots += 1;
            i += 1;
        } else if bytes[i] == b'%'
            && bytes.get(i + 1) == Some(&b'2')
            && bytes
                .get(i + 2)
                .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'e'))
        {
            dots += 1;
            i += 3;
        } else {
            return false;
        }
    }
    matches!(dots, 1 | 2)
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
        .route(
            "/ValueSet/{id}/$expand",
            get(valueset_expand_id).post(valueset_expand_id),
        )
        .route("/ConceptMap/$translate", get(translate).post(translate))
        .route("/autocomplete", get(autocomplete))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(request_timeout))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            cors_middleware,
        ))
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

/// `--cors-origin` handling, outermost so it sees every request (including
/// `OPTIONS`, which no route otherwise answers) and every response (including
/// error responses from deeper layers). A no-op, byte for byte, when no
/// `--cors-origin` was given: no headers read or written, no `OPTIONS`
/// interception - CORS stays entirely off by default.
async fn cors_middleware(State(st): State<AppState>, request: Request, next: Next) -> Response {
    if st.cors_origins.is_empty() {
        return next.run(request).await;
    }
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let allow = origin
        .as_deref()
        .and_then(|o| matched_cors_origin(&st.cors_origins, o));
    let named_origin_mode = !st.cors_origins.iter().any(|origin| origin == "*");
    let is_preflight = request.method() == Method::OPTIONS
        && origin.is_some()
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);

    if is_preflight {
        let mut headers = HeaderMap::new();
        if named_origin_mode {
            append_vary_origin(&mut headers);
        }
        if let Some(allow) = &allow {
            insert_allow_origin(&mut headers, allow);
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET, POST, OPTIONS"),
            );
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("Content-Type, Accept"),
            );
        }
        // No `Origin`, or one that matches nothing: no CORS headers and no
        // error status either, the normal browser-blocked path - a preflight
        // still gets a response, it just carries nothing that lets the
        // browser proceed with the real request.
        return (StatusCode::NO_CONTENT, headers).into_response();
    }

    let mut response = next.run(request).await;
    if named_origin_mode {
        append_vary_origin(response.headers_mut());
    }
    if let Some(allow) = &allow {
        insert_allow_origin(response.headers_mut(), allow);
    }
    response
}

/// A `--cors-origin` match for one request's `Origin`: either the configured
/// wildcard, or the one configured origin that equals it exactly.
enum CorsMatch<'a> {
    Wildcard,
    Origin(&'a str),
}

/// Match a request's `Origin` against the configured `--cors-origin` list.
/// The literal `*` wins outright (an explicit opt-in to wildcard CORS);
/// otherwise the origin must equal one configured entry exactly - no scheme,
/// port, or subdomain wildcarding.
fn matched_cors_origin<'a>(configured: &'a [String], origin: &str) -> Option<CorsMatch<'a>> {
    if configured.iter().any(|o| o == "*") {
        return Some(CorsMatch::Wildcard);
    }
    configured
        .iter()
        .find(|o| o.as_str() == origin)
        .map(|o| CorsMatch::Origin(o.as_str()))
}

/// Set `Access-Control-Allow-Origin` (`insert`, not `append` - exactly one
/// value on every response, never stacked). `Access-Control-Allow-Credentials`
/// is never sent - this server has no authentication, and it is invalid
/// alongside a wildcard origin anyway.
fn insert_allow_origin(headers: &mut HeaderMap, allow: &CorsMatch) {
    match allow {
        CorsMatch::Wildcard => {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            );
        }
        CorsMatch::Origin(origin) => {
            if let Ok(value) = HeaderValue::from_str(origin) {
                headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
            }
        }
    }
}

/// Preserve existing cache variation while recording that named-origin CORS
/// depends on `Origin`. This applies to matching and non-matching responses;
/// otherwise a cache could reuse a denied response for an allowed origin.
fn append_vary_origin(headers: &mut HeaderMap) {
    let already_varies = headers.get_all(header::VARY).iter().any(|value| {
        value.to_str().ok().is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("origin"))
        })
    });
    if !already_varies {
        headers.append(header::VARY, HeaderValue::from_static("Origin"));
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
    let params = match parse_query(q.as_deref().unwrap_or("")) {
        Ok(params) => params,
        Err(error) => return fhir_err(error),
    };
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
    let params = match parse_query(q.as_deref().unwrap_or("")) {
        Ok(params) => params,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": error.diagnostics})),
            )
                .into_response();
        }
    };
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
    method: Method,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let input = match parse_operation_input(
        OperationKind::Lookup,
        &method,
        q.as_deref().unwrap_or(""),
        &body,
    ) {
        Ok(input) => input,
        Err(e) => return fhir_err(e),
    };
    let params = input.params;
    if let Some(e) = unsupported_lookup_input(&params) {
        return fhir_err(e);
    }
    if let Err(e) = validate_operation_values(OperationKind::Lookup, &params) {
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
    let loaded_version = st.release_version.clone();
    run_db(&st, move |c| {
        ops::check_lookup_version_against(
            loaded_version.as_deref(),
            "version",
            version.as_deref(),
        )?;
        ops::lookup_with_version(c, &code, &props, loaded_version.as_deref())
    })
    .await
}

async fn validate_code(
    State(st): State<AppState>,
    method: Method,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let input = match parse_operation_input(
        OperationKind::ValidateCode,
        &method,
        q.as_deref().unwrap_or(""),
        &body,
    ) {
        Ok(input) => input,
        Err(e) => return fhir_err(e),
    };
    let params = input.params;
    if let Some(e) = unsupported_validate_code_input(&params) {
        return fhir_err(e);
    }
    if let Err(e) = validate_operation_values(OperationKind::ValidateCode, &params) {
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
    let loaded_version = st.release_version.clone();
    run_db(&st, move |c| {
        ops::check_lookup_version_against(
            loaded_version.as_deref(),
            "version",
            version.as_deref(),
        )?;
        ops::validate_code(c, &code, display.as_deref())
    })
    .await
}

async fn subsumes(
    State(st): State<AppState>,
    method: Method,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let input = match parse_operation_input(
        OperationKind::Subsumes,
        &method,
        q.as_deref().unwrap_or(""),
        &body,
    ) {
        Ok(input) => input,
        Err(e) => return fhir_err(e),
    };
    let params = input.params;
    if let Some(e) = unsupported_subsumes_input(&params) {
        return fhir_err(e);
    }
    if let Err(e) = validate_operation_values(OperationKind::Subsumes, &params) {
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
    let tct_available = st.tct_available;
    let loaded_version = st.release_version.clone();
    run_db(&st, move |c| {
        ops::check_lookup_version_against(
            loaded_version.as_deref(),
            "version",
            version.as_deref(),
        )?;
        ops::subsumes_with_tct(c, &a, &b, tct_available)
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
    let params = match parse_query(q.as_deref().unwrap_or("")) {
        Ok(params) => params,
        Err(error) => return fhir_err(error),
    };
    let url = param(&params, "url");
    let id = param(&params, "_id").or_else(|| param(&params, "id"));
    if url.is_some_and(|u| u != fhir::SNOMED_SYSTEM)
        || id.is_some_and(|i| i != fhir::CODE_SYSTEM_ID)
    {
        return fhir_ok(fhir::bundle_searchset(vec![]));
    }
    let full_url = format!("{}/CodeSystem/{}", st.impl_url, fhir::CODE_SYSTEM_ID);
    run_db(&st, move |c| {
        ops::code_system_resource(c).map(|cs| fhir::bundle_searchset(vec![(full_url, cs)]))
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
    method: Method,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let input = match parse_operation_input(
        OperationKind::Expand,
        &method,
        q.as_deref().unwrap_or(""),
        &body,
    ) {
        Ok(input) => input,
        Err(e) => return fhir_err(e),
    };
    let params = input.params;
    if let Some(e) = unsupported_expand_input(&params) {
        return fhir_err(e);
    }
    if let Err(e) = validate_operation_values(OperationKind::Expand, &params) {
        return fhir_err(e);
    }
    if input.inline_value_set.is_some() && param(&params, "url").is_some() {
        return fhir_err(FhirError::invalid(
            "conflicting ValueSet selectors: supply either `url` or inline `valueSet`, not both",
        ));
    }
    let (count, offset, include_designations, active_only) = match pagination(&params) {
        Ok(v) => v,
        Err(e) => return fhir_err(e),
    };
    let designation_tokens = params_all(&params, "designation");
    let include_designations = include_designations || !designation_tokens.is_empty();
    let properties = match ops::parse_expand_properties(&params_all(&params, "property")) {
        Ok(p) => p,
        Err(e) => return fhir_err(e),
    };
    let display_language = param(&params, "displayLanguage").map(str::to_string);
    let version_pins = version_pins(&params);
    let include_definition = flag(&params, "includeDefinition");

    if let Some(value_set) = input.inline_value_set {
        let value_set = match parse_inline_value_set(value_set) {
            Ok(value_set) => value_set,
            Err(e) => return fhir_err(e),
        };
        if matches!(value_set.target, InlineValueSetTarget::Members(_))
            && param(&params, "filter").is_some()
        {
            return fhir_err(FhirError::invalid(
                "`filter` is not supported with an extensional inline `valueSet`; it is refused rather than ignored",
            ));
        }
        let filter = param(&params, "filter").map(str::to_string);
        let definition = include_definition.then_some(value_set.definition);
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let loaded_version = st.release_version.clone();
        return run_db(&st, move |c| {
            ops::check_system_versions_against(loaded_version.as_deref(), &version_pins)?;
            let mut out = match &value_set.target {
                InlineValueSetTarget::Ecl(ecl) => ops::expand(
                    c,
                    ecl.as_deref(),
                    filter.as_deref(),
                    count,
                    offset,
                    include_designations,
                    active_only,
                    Some(deadline),
                    display_language.as_deref(),
                )?,
                InlineValueSetTarget::Members(members) => ops::expand_members_with_active(
                    c,
                    members,
                    count,
                    offset,
                    include_designations,
                    active_only,
                    display_language.as_deref(),
                )?,
            };
            ops::apply_designation_filter(&mut out, &designation_tokens);
            ops::apply_expansion_properties(c, &mut out, &properties)?;
            if let Some(definition) = definition {
                fhir::attach_definition(&mut out, definition);
            }
            Ok(out)
        })
        .await;
    }

    // A `url` naming a stored `.codelist` ValueSet expands its member set.
    if let Some(url) = param(&params, "url") {
        if let Some(vs) = st.registry.resolve_url(url) {
            let members = vs.members.clone();
            let definition = include_definition.then(|| vs.to_resource());
            let loaded_version = st.release_version.clone();
            return run_db(&st, move |c| {
                ops::check_system_versions_against(loaded_version.as_deref(), &version_pins)?;
                let mut out = ops::expand_members(
                    c,
                    &members,
                    count,
                    offset,
                    include_designations,
                    display_language.as_deref(),
                )?;
                ops::apply_designation_filter(&mut out, &designation_tokens);
                ops::apply_expansion_properties(c, &mut out, &properties)?;
                if let Some(definition) = definition {
                    fhir::attach_definition(&mut out, definition);
                }
                Ok(out)
            })
            .await;
        }
    }

    let target = match implicit_expand_target(param(&params, "url")) {
        Ok(target) => target,
        Err(e) => return fhir_err(e),
    };
    let filter = param(&params, "filter").map(str::to_string);
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let loaded_version = st.release_version.clone();
    run_db(&st, move |c| {
        ops::check_system_versions_against(loaded_version.as_deref(), &version_pins)?;
        let mut out = match &target {
            ExpandTarget::Refsets => ops::expand_refsets(
                c,
                filter.as_deref(),
                count,
                offset,
                include_designations,
                display_language.as_deref(),
            )?,
            ExpandTarget::Ecl(ecl) => ops::expand(
                c,
                ecl.as_deref(),
                filter.as_deref(),
                count,
                offset,
                include_designations,
                active_only,
                Some(deadline),
                display_language.as_deref(),
            )?,
        };
        ops::apply_designation_filter(&mut out, &designation_tokens);
        ops::apply_expansion_properties(c, &mut out, &properties)?;
        if include_definition {
            let definition = match &target {
                ExpandTarget::Refsets => fhir::implicit_refsets_valueset_definition(),
                ExpandTarget::Ecl(ecl) => fhir::implicit_valueset_definition(ecl.as_deref()),
            };
            fhir::attach_definition(&mut out, definition);
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
    let params = match parse_query(q.as_deref().unwrap_or("")) {
        Ok(params) => params,
        Err(error) => return fhir_err(error),
    };
    let url = param(&params, "url");
    let id = param(&params, "_id").or_else(|| param(&params, "id"));
    let status = param(&params, "status");
    let resources: Vec<(String, serde_json::Value)> = st
        .registry
        .iter()
        .filter(|vs| url.is_none_or(|u| vs.canonical_url == u))
        .filter(|vs| id.is_none_or(|i| vs.front_matter.id == i))
        .filter(|vs| {
            status.is_none_or(|s| {
                crate::commands::codelist::fhir_status(&vs.front_matter.status) == s
            })
        })
        .map(|vs| {
            (
                format!("{}/ValueSet/{}", st.impl_url, vs.front_matter.id),
                vs.summary_resource(),
            )
        })
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
    method: Method,
    headers: HeaderMap,
    Path(id): Path<String>,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let input = match parse_operation_input(
        OperationKind::ExpandInstance,
        &method,
        q.as_deref().unwrap_or(""),
        &body,
    ) {
        Ok(input) => input,
        Err(e) => return fhir_err(e),
    };
    let params = input.params;
    if let Some(e) = unsupported_expand_input(&params) {
        return fhir_err(e);
    }
    if let Err(e) = validate_operation_values(OperationKind::ExpandInstance, &params) {
        return fhir_err(e);
    }
    if param(&params, "url").is_some() {
        return fhir_err(FhirError::invalid(
            "`url` must not be supplied when expanding a ValueSet instance selected by the route",
        ));
    }
    let Some(vs) = st.registry.get(&id) else {
        return fhir_err(FhirError::not_found(format!("ValueSet '{id}' not found")));
    };
    let members = vs.members.clone();
    // `activeOnly` is intentionally unused here - see `pagination`'s doc comment.
    let (count, offset, include_designations, _active_only) = match pagination(&params) {
        Ok(v) => v,
        Err(e) => return fhir_err(e),
    };
    let designation_tokens = params_all(&params, "designation");
    let include_designations = include_designations || !designation_tokens.is_empty();
    let properties = match ops::parse_expand_properties(&params_all(&params, "property")) {
        Ok(p) => p,
        Err(e) => return fhir_err(e),
    };
    let display_language = param(&params, "displayLanguage").map(str::to_string);
    let version_pins = version_pins(&params);
    let definition = flag(&params, "includeDefinition").then(|| vs.to_resource());
    let loaded_version = st.release_version.clone();
    run_db(&st, move |c| {
        ops::check_system_versions_against(loaded_version.as_deref(), &version_pins)?;
        let mut out = ops::expand_members(
            c,
            &members,
            count,
            offset,
            include_designations,
            display_language.as_deref(),
        )?;
        ops::apply_designation_filter(&mut out, &designation_tokens);
        ops::apply_expansion_properties(c, &mut out, &properties)?;
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
    method: Method,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let input = match parse_operation_input(
        OperationKind::Translate,
        &method,
        q.as_deref().unwrap_or(""),
        &body,
    ) {
        Ok(input) => input,
        Err(e) => return fhir_err(e),
    };
    let params = input.params;
    if let Some(e) = unsupported_translate_input(&params) {
        return fhir_err(e);
    }
    if let Err(e) = validate_operation_values(OperationKind::Translate, &params) {
        return fhir_err(e);
    }
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
    let Some(target) = param(&params, "targetsystem").map(str::to_string) else {
        return fhir_err(FhirError::invalid(
            "`targetsystem` parameter is required".to_string(),
        ));
    };
    let version = param(&params, "version").map(str::to_string);
    let loaded_version = st.release_version.clone();
    run_db(&st, move |c| {
        ops::check_lookup_version_against(
            loaded_version.as_deref(),
            "version",
            version.as_deref(),
        )?;
        ops::translate(c, &system, &code, &target)
    })
    .await
}

/// `GET|POST /ValueSet/$validate-code` - is `code` in the ValueSet named by
/// `url` (a stored `.codelist` or an implicit ECL value set)?
async fn vs_validate_code(
    State(st): State<AppState>,
    method: Method,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: String,
) -> Response {
    if let Some(r) = reject_xml(&headers) {
        return r;
    }
    let input = match parse_operation_input(
        OperationKind::ValueSetValidateCode,
        &method,
        q.as_deref().unwrap_or(""),
        &body,
    ) {
        Ok(input) => input,
        Err(e) => return fhir_err(e),
    };
    let params = input.params;
    if let Some(e) = unsupported_vs_validate_code_input(&params) {
        return fhir_err(e);
    }
    if let Err(e) = validate_operation_values(OperationKind::ValueSetValidateCode, &params) {
        return fhir_err(e);
    }
    if input.inline_value_set.is_some() && param(&params, "url").is_some() {
        return fhir_err(FhirError::invalid(
            "conflicting ValueSet selectors: supply either `url` or inline `valueSet`, not both",
        ));
    }
    if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
        return fhir_err(e);
    }
    let Some(code) = param(&params, "code").map(str::to_string) else {
        return fhir_err(FhirError::invalid(
            "`code` parameter is required".to_string(),
        ));
    };
    let display = param(&params, "display").map(str::to_string);
    let system_version = param(&params, "systemVersion").map(str::to_string);

    if let Some(value_set) = input.inline_value_set {
        let value_set = match parse_inline_value_set(value_set) {
            Ok(value_set) => value_set,
            Err(e) => return fhir_err(e),
        };
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let loaded_version = st.release_version.clone();
        return run_db(&st, move |c| {
            ops::check_lookup_version_against(
                loaded_version.as_deref(),
                "systemVersion",
                system_version.as_deref(),
            )?;
            match &value_set.target {
                InlineValueSetTarget::Members(members) => {
                    let members = members.iter().map(|(id, _)| id.clone()).collect();
                    ops::validate_code_in_set(
                        c,
                        &members,
                        &code,
                        &value_set.canonical,
                        display.as_deref(),
                    )
                }
                InlineValueSetTarget::Ecl(Some(ecl)) => {
                    ops::validate_code_in_ecl(c, ecl, &code, Some(deadline), display.as_deref())
                }
                InlineValueSetTarget::Ecl(None) => ops::validate_code(c, &code, display.as_deref()),
            }
        })
        .await;
    }

    let Some(url) = param(&params, "url").map(str::to_string) else {
        return fhir_err(FhirError::invalid(
            "`url` or inline `valueSet` parameter is required (the ValueSet to validate against)",
        ));
    };

    if let Some(vs) = st.registry.resolve_url(&url) {
        let members: std::collections::HashSet<String> =
            vs.members.iter().map(|(id, _)| id.clone()).collect();
        let vs_url = vs.canonical_url.clone();
        let loaded_version = st.release_version.clone();
        return run_db(&st, move |c| {
            ops::check_lookup_version_against(
                loaded_version.as_deref(),
                "systemVersion",
                system_version.as_deref(),
            )?;
            ops::validate_code_in_set(c, &members, &code, &vs_url, display.as_deref())
        })
        .await;
    }
    if let Some(ecl) = parse_implicit_ecl(&url) {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let loaded_version = st.release_version.clone();
        return run_db(&st, move |c| {
            ops::check_lookup_version_against(
                loaded_version.as_deref(),
                "systemVersion",
                system_version.as_deref(),
            )?;
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
    let tct_available = st.tct_available;
    let release_version = st.release_version.clone();
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
                    let (status, resource) = if entry.get("resource").is_some() {
                        // No operation in this query-only batch API consumes an
                        // entry payload; silently discarding one changes input.
                        let error = FhirError::invalid(
                            "batch entry resource is not supported; supply GET operation parameters in request.url",
                        );
                        (error.status, error.outcome())
                    } else {
                        run_operation(
                            conn,
                            &registry,
                            method,
                            url,
                            deadline,
                            tct_available,
                            release_version.as_deref(),
                        )
                    };
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
    tct_available: bool,
    release_version: Option<&str>,
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
    let params = match parse_query(query) {
        Ok(params) => params,
        Err(error) => return (error.status, error.outcome()),
    };

    let result: Result<serde_json::Value, FhirError> = match path {
        "CodeSystem/$lookup" => {
            if let Err(e) = validate_operation_query(OperationKind::Lookup, &params) {
                return (e.status, e.outcome());
            }
            if let Some(e) = unsupported_lookup_input(&params) {
                return (e.status, e.outcome());
            }
            if let Err(e) = validate_operation_values(OperationKind::Lookup, &params) {
                return (e.status, e.outcome());
            }
            if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
                return (e.status, e.outcome());
            }
            match param(&params, "code") {
                Some(code) => ops::check_lookup_version_against(
                    release_version,
                    "version",
                    param(&params, "version"),
                )
                .and_then(|()| {
                    ops::lookup_with_version(
                        conn,
                        code,
                        &params_all(&params, "property"),
                        release_version,
                    )
                }),
                None => Err(FhirError::invalid(
                    "missing required parameter 'code'".to_string(),
                )),
            }
        }
        "CodeSystem/$validate-code" => {
            if let Err(e) = validate_operation_query(OperationKind::ValidateCode, &params) {
                return (e.status, e.outcome());
            }
            if let Some(e) = unsupported_validate_code_input(&params) {
                return (e.status, e.outcome());
            }
            if let Err(e) = validate_operation_values(OperationKind::ValidateCode, &params) {
                return (e.status, e.outcome());
            }
            if let Err(e) = ops::check_lookup_system(param(&params, "url")) {
                return (e.status, e.outcome());
            }
            match param(&params, "code") {
                Some(code) => ops::check_lookup_version_against(
                    release_version,
                    "version",
                    param(&params, "version"),
                )
                .and_then(|()| ops::validate_code(conn, code, param(&params, "display"))),
                None => Err(FhirError::invalid(
                    "missing required parameter 'code'".to_string(),
                )),
            }
        }
        "CodeSystem/$subsumes" => {
            if let Err(e) = validate_operation_query(OperationKind::Subsumes, &params) {
                return (e.status, e.outcome());
            }
            if let Some(e) = unsupported_subsumes_input(&params) {
                return (e.status, e.outcome());
            }
            if let Err(e) = validate_operation_values(OperationKind::Subsumes, &params) {
                return (e.status, e.outcome());
            }
            if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
                return (e.status, e.outcome());
            }
            match (param(&params, "codeA"), param(&params, "codeB")) {
                (Some(a), Some(b)) => ops::check_lookup_version_against(
                    release_version,
                    "version",
                    param(&params, "version"),
                )
                .and_then(|()| ops::subsumes_with_tct(conn, a, b, tct_available)),
                _ => Err(FhirError::invalid(
                    "missing required parameters 'codeA' and 'codeB'".to_string(),
                )),
            }
        }
        "ValueSet/$expand" => {
            if let Err(e) = validate_operation_query(OperationKind::Expand, &params) {
                return (e.status, e.outcome());
            }
            if let Some(e) = unsupported_expand_input(&params) {
                return (e.status, e.outcome());
            }
            if let Err(e) = validate_operation_values(OperationKind::Expand, &params) {
                return (e.status, e.outcome());
            }
            let (count, offset, desig, active_only) = match pagination(&params) {
                Ok(v) => v,
                Err(e) => return (e.status, e.outcome()),
            };
            let designation_tokens = params_all(&params, "designation");
            let desig = desig || !designation_tokens.is_empty();
            let properties = match ops::parse_expand_properties(&params_all(&params, "property")) {
                Ok(p) => p,
                Err(e) => return (e.status, e.outcome()),
            };
            let display_language = param(&params, "displayLanguage");
            if let Err(e) =
                ops::check_system_versions_against(release_version, &version_pins(&params))
            {
                return (e.status, e.outcome());
            }
            let include_definition = flag(&params, "includeDefinition");
            if let Some(vs) = param(&params, "url").and_then(|u| registry.resolve_url(u)) {
                ops::expand_members(conn, &vs.members, count, offset, desig, display_language)
                    .and_then(|mut out| {
                        ops::apply_designation_filter(&mut out, &designation_tokens);
                        ops::apply_expansion_properties(conn, &mut out, &properties)?;
                        if include_definition {
                            fhir::attach_definition(&mut out, vs.to_resource());
                        }
                        Ok(out)
                    })
            } else {
                let target = match implicit_expand_target(param(&params, "url")) {
                    Ok(target) => target,
                    Err(e) => return (e.status, e.outcome()),
                };
                let expanded = match &target {
                    ExpandTarget::Refsets => ops::expand_refsets(
                        conn,
                        param(&params, "filter"),
                        count,
                        offset,
                        desig,
                        display_language,
                    ),
                    ExpandTarget::Ecl(ecl) => ops::expand(
                        conn,
                        ecl.as_deref(),
                        param(&params, "filter"),
                        count,
                        offset,
                        desig,
                        active_only,
                        Some(deadline),
                        display_language,
                    ),
                };
                expanded.and_then(|mut out| {
                    ops::apply_designation_filter(&mut out, &designation_tokens);
                    ops::apply_expansion_properties(conn, &mut out, &properties)?;
                    if include_definition {
                        let definition = match &target {
                            ExpandTarget::Refsets => fhir::implicit_refsets_valueset_definition(),
                            ExpandTarget::Ecl(ecl) => {
                                fhir::implicit_valueset_definition(ecl.as_deref())
                            }
                        };
                        fhir::attach_definition(&mut out, definition);
                    }
                    Ok(out)
                })
            }
        }
        "ValueSet/$validate-code" => {
            if let Err(e) = validate_operation_query(OperationKind::ValueSetValidateCode, &params) {
                return (e.status, e.outcome());
            }
            if let Some(e) = unsupported_vs_validate_code_input(&params) {
                return (e.status, e.outcome());
            }
            if let Err(e) = validate_operation_values(OperationKind::ValueSetValidateCode, &params)
            {
                return (e.status, e.outcome());
            }
            if let Err(e) = ops::check_lookup_system(param(&params, "system")) {
                return (e.status, e.outcome());
            }
            match (param(&params, "code"), param(&params, "url")) {
                (Some(code), Some(url)) => ops::check_lookup_version_against(
                    release_version,
                    "systemVersion",
                    param(&params, "systemVersion"),
                )
                .and_then(|()| {
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
                }),
                _ => Err(FhirError::invalid(
                    "missing required parameters 'code' and 'url'".to_string(),
                )),
            }
        }
        "ConceptMap/$translate" => {
            if let Err(e) = validate_operation_query(OperationKind::Translate, &params) {
                return (e.status, e.outcome());
            }
            if let Some(e) = unsupported_translate_input(&params) {
                return (e.status, e.outcome());
            }
            if let Err(e) = validate_operation_values(OperationKind::Translate, &params) {
                return (e.status, e.outcome());
            }
            match (
                param(&params, "system"),
                param(&params, "code"),
                param(&params, "targetsystem"),
            ) {
                (Some(system), Some(code), Some(target)) => ops::check_lookup_version_against(
                    release_version,
                    "version",
                    param(&params, "version"),
                )
                .and_then(|()| ops::translate(conn, system, code, target)),
                _ => Err(FhirError::invalid(
                    "missing required parameters 'system', 'code', 'targetsystem'".to_string(),
                )),
            }
        }
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
            expression: Vec::new(),
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
        // Every other form is deliberately not an ECL expression: `All` is the
        // whole code system and `Refsets` is answered by a direct query, so
        // neither has a constraint `$validate-code` could evaluate. Spelled out
        // rather than matched with `_` so that adding an implicit form forces a
        // decision at this call site instead of silently arriving here as
        // "not an implicit ECL value set" - which is the shape of the defect
        // `R17` exists to prevent.
        Some(
            ImplicitValueSet::All | ImplicitValueSet::Refsets | ImplicitValueSet::Unsupported(_),
        )
        | None => None,
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
    /// `?fhir_vs=refset` - the set of all SNOMED CT reference sets. Not an ECL
    /// constraint (it has no is-a-shaped definition this server can evaluate),
    /// so it is answered directly from `crate::refset::list_refsets`: every
    /// reference set with at least one member loaded in this edition.
    Refsets,
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
        return Some(match crate::sctid::validate_syntax(sctid) {
            Ok(()) => ImplicitValueSet::Ecl(format!("<<{sctid}")),
            Err(_) => ImplicitValueSet::Unsupported(after.to_string()),
        });
    }
    if let Some(sctid) = after.strip_prefix("refset/") {
        return Some(match crate::sctid::validate_syntax(sctid) {
            Ok(()) => ImplicitValueSet::Ecl(format!("^{sctid}")),
            Err(_) => ImplicitValueSet::Unsupported(after.to_string()),
        });
    }
    // `?fhir_vs=refset` (the set of reference sets) is a distinct query rather
    // than an ECL expression - see [`ImplicitValueSet::Refsets`].
    if after == "refset" {
        return Some(ImplicitValueSet::Refsets);
    }
    Some(ImplicitValueSet::Unsupported(after.to_string()))
}

/// What an `$expand` `url` resolves to: either the ECL to evaluate
/// (`Ecl(None)` means "expand the whole code system"), or the implicit
/// "set of reference sets" value set, which is answered by a direct query
/// rather than the ECL engine (see [`ImplicitValueSet::Refsets`]).
#[derive(Debug, PartialEq, Eq)]
enum ExpandTarget {
    Ecl(Option<String>),
    Refsets,
}

/// Resolve an `$expand` `url` to what it names, or an error explaining why it
/// cannot be expanded.
fn implicit_expand_target(url: Option<&str>) -> Result<ExpandTarget, FhirError> {
    let Some(url) = url else {
        // No `url` at all: a bare/`filter`-only expansion over the code system.
        return Ok(ExpandTarget::Ecl(None));
    };
    match parse_implicit(url) {
        Some(ImplicitValueSet::All) => Ok(ExpandTarget::Ecl(None)),
        Some(ImplicitValueSet::Ecl(ecl)) => Ok(ExpandTarget::Ecl(Some(ecl))),
        Some(ImplicitValueSet::Refsets) => Ok(ExpandTarget::Refsets),
        Some(ImplicitValueSet::Unsupported(form)) => Err(FhirError::invalid(format!(
            "implicit SNOMED CT value set `fhir_vs={form}` is not supported; \
             use `fhir_vs`, `fhir_vs=ecl/[ecl]`, `fhir_vs=isa/[sctid]`, \
             `fhir_vs=refset/[sctid]`, or `fhir_vs=refset`"
        ))),
        None => Err(FhirError::not_found(format!("ValueSet '{url}' not found"))),
    }
}

#[derive(Clone, Copy)]
enum OperationKind {
    Lookup,
    ValidateCode,
    Subsumes,
    Expand,
    ExpandInstance,
    ValueSetValidateCode,
    Translate,
}

#[derive(Clone, Copy)]
struct OperationParameter {
    name: &'static str,
    value_key: &'static str,
    repeatable: bool,
}

const fn operation_parameter(
    name: &'static str,
    value_key: &'static str,
    repeatable: bool,
) -> OperationParameter {
    OperationParameter {
        name,
        value_key,
        repeatable,
    }
}

const LOOKUP_PARAMETERS: &[OperationParameter] = &[
    operation_parameter("code", "valueCode", false),
    operation_parameter("system", "valueUri", false),
    operation_parameter("version", "valueString", false),
    operation_parameter("coding", "valueCoding", false),
    operation_parameter("date", "valueDateTime", false),
    operation_parameter("displayLanguage", "valueCode", false),
    operation_parameter("property", "valueCode", true),
];

const VALIDATE_CODE_PARAMETERS: &[OperationParameter] = &[
    operation_parameter("url", "valueUri", false),
    operation_parameter("codeSystem", "resource", false),
    operation_parameter("code", "valueCode", false),
    operation_parameter("version", "valueString", false),
    operation_parameter("display", "valueString", false),
    operation_parameter("coding", "valueCoding", false),
    operation_parameter("codeableConcept", "valueCodeableConcept", false),
    operation_parameter("date", "valueDateTime", false),
    operation_parameter("abstract", "valueBoolean", false),
    operation_parameter("displayLanguage", "valueCode", false),
];

const SUBSUMES_PARAMETERS: &[OperationParameter] = &[
    operation_parameter("codeA", "valueCode", false),
    operation_parameter("codeB", "valueCode", false),
    operation_parameter("system", "valueUri", false),
    operation_parameter("version", "valueString", false),
    operation_parameter("codingA", "valueCoding", false),
    operation_parameter("codingB", "valueCoding", false),
];

const EXPAND_PARAMETERS: &[OperationParameter] = &[
    operation_parameter("url", "valueUri", false),
    operation_parameter("valueSet", "resource", false),
    operation_parameter("valueSetVersion", "valueString", false),
    operation_parameter("context", "valueUri", false),
    operation_parameter("contextDirection", "valueCode", false),
    operation_parameter("filter", "valueString", false),
    operation_parameter("date", "valueDateTime", false),
    operation_parameter("offset", "valueInteger", false),
    operation_parameter("count", "valueInteger", false),
    operation_parameter("includeDesignations", "valueBoolean", false),
    operation_parameter("designation", "valueString", true),
    operation_parameter("includeDefinition", "valueBoolean", false),
    operation_parameter("activeOnly", "valueBoolean", false),
    operation_parameter("excludeNested", "valueBoolean", false),
    operation_parameter("excludeNotForUI", "valueBoolean", false),
    operation_parameter("excludePostCoordinated", "valueBoolean", false),
    operation_parameter("displayLanguage", "valueCode", false),
    operation_parameter("exclude-system", "valueCanonical", true),
    operation_parameter("system-version", "valueCanonical", true),
    operation_parameter("check-system-version", "valueCanonical", true),
    operation_parameter("force-system-version", "valueCanonical", true),
    // `property` is an R5 addition already shipped by this server.
    operation_parameter("property", "valueCode", true),
];

const VALUE_SET_VALIDATE_CODE_PARAMETERS: &[OperationParameter] = &[
    operation_parameter("url", "valueUri", false),
    operation_parameter("context", "valueUri", false),
    operation_parameter("valueSet", "resource", false),
    operation_parameter("valueSetVersion", "valueString", false),
    operation_parameter("code", "valueCode", false),
    operation_parameter("system", "valueUri", false),
    operation_parameter("systemVersion", "valueString", false),
    operation_parameter("display", "valueString", false),
    operation_parameter("coding", "valueCoding", false),
    operation_parameter("codeableConcept", "valueCodeableConcept", false),
    operation_parameter("date", "valueDateTime", false),
    operation_parameter("abstract", "valueBoolean", false),
    operation_parameter("displayLanguage", "valueCode", false),
];

const TRANSLATE_PARAMETERS: &[OperationParameter] = &[
    operation_parameter("url", "valueUri", false),
    operation_parameter("conceptMap", "resource", false),
    operation_parameter("conceptMapVersion", "valueString", false),
    operation_parameter("code", "valueCode", false),
    operation_parameter("system", "valueUri", false),
    operation_parameter("version", "valueString", false),
    operation_parameter("source", "valueUri", false),
    operation_parameter("coding", "valueCoding", false),
    operation_parameter("codeableConcept", "valueCodeableConcept", false),
    operation_parameter("target", "valueUri", false),
    operation_parameter("targetsystem", "valueUri", false),
    operation_parameter("dependency", "part", true),
    operation_parameter("reverse", "valueBoolean", false),
];

impl OperationKind {
    fn parameters(self) -> &'static [OperationParameter] {
        match self {
            Self::Lookup => LOOKUP_PARAMETERS,
            Self::ValidateCode => VALIDATE_CODE_PARAMETERS,
            Self::Subsumes => SUBSUMES_PARAMETERS,
            Self::Expand | Self::ExpandInstance => EXPAND_PARAMETERS,
            Self::ValueSetValidateCode => VALUE_SET_VALIDATE_CODE_PARAMETERS,
            Self::Translate => TRANSLATE_PARAMETERS,
        }
    }

    fn parameter(self, name: &str) -> Option<OperationParameter> {
        self.parameters()
            .iter()
            .copied()
            .find(|parameter| parameter.name == name)
    }

    fn allows_inline_value_set(self) -> bool {
        matches!(self, Self::Expand | Self::ValueSetValidateCode)
    }
}

/// One operation request after GET query parameters and POST `Parameters`
/// primitives have been merged. Inline ValueSets remain typed JSON until the
/// operation resolves their compose definition against its existing engine.
struct OperationInput {
    params: Vec<(String, String)>,
    inline_value_set: Option<serde_json::Value>,
}

/// Parse the standard FHIR operation transport without letting body data
/// silently override query data. Scalar and repeatable values use the same
/// ordered string representation as GET; a value supplied in both channels is
/// accepted only when the complete ordered value list is equal.
fn parse_operation_input(
    operation: OperationKind,
    method: &Method,
    raw_query: &str,
    body: &str,
) -> Result<OperationInput, FhirError> {
    let mut params = parse_query(raw_query)?;
    if !body.trim().is_empty() && method != Method::POST {
        return Err(FhirError::invalid(
            "a request body is supported only for POST FHIR operations",
        ));
    }
    validate_operation_query(operation, &params)?;
    if body.trim().is_empty() {
        return Ok(OperationInput {
            params,
            inline_value_set: None,
        });
    }
    let resource: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| FhirError::invalid(format!("request body is not valid JSON: {error}")))?;
    if resource.get("resourceType").and_then(|v| v.as_str()) != Some("Parameters") {
        return Err(FhirError::invalid(
            "FHIR operation POST body must be a Parameters resource",
        ));
    }
    let resource = resource
        .as_object()
        .expect("a Parameters resource is a JSON object");
    if let Some(field) = resource.keys().find(|field| {
        !matches!(
            field.as_str(),
            "resourceType" | "id" | "meta" | "implicitRules" | "language" | "parameter"
        )
    }) {
        return Err(FhirError::invalid(format!(
            "unsupported element `{field}` on Parameters resource"
        ))
        .at(field));
    }
    let entries = match resource.get("parameter") {
        None => &[][..],
        Some(value) => value.as_array().ok_or_else(|| {
            FhirError::invalid("Parameters.parameter must be an array").at("Parameters.parameter")
        })?,
    };

    let mut body_values: Vec<(String, String, usize)> = Vec::new();
    let mut inline_value_set = None;
    for (index, entry) in entries.iter().enumerate() {
        let location = format!("Parameters.parameter[{index}]");
        let object = entry.as_object().ok_or_else(|| {
            FhirError::invalid("each Parameters.parameter entry must be an object").at(&location)
        })?;
        let name = object.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
            FhirError::invalid("Parameters.parameter.name must be a non-empty string")
                .at(format!("{location}.name"))
        })?;
        if name.is_empty() {
            return Err(
                FhirError::invalid("Parameters.parameter.name must be a non-empty string")
                    .at(format!("{location}.name")),
            );
        }
        let parameter = operation.parameter(name).ok_or_else(|| {
            FhirError::invalid(format!(
                "operation parameter `{name}` is not defined for this operation"
            ))
            .at(format!("{location}.name"))
        })?;
        if object.contains_key("extension") || object.contains_key("modifierExtension") {
            return Err(FhirError::invalid(format!(
                "extensions on operation parameter `{name}` are not supported"
            ))
            .at(&location));
        }

        let payloads: Vec<(&str, &serde_json::Value)> = object
            .iter()
            .filter_map(|(key, value)| {
                (key.starts_with("value") || key == "resource" || key == "part")
                    .then_some((key.as_str(), value))
            })
            .collect();
        if payloads.len() != 1 {
            return Err(FhirError::invalid(format!(
                "operation parameter `{name}` must contain exactly one value[x], resource, or part payload"
            ))
            .at(&location));
        }
        let (kind, value) = payloads[0];
        if let Some(field) = object
            .keys()
            .find(|field| !matches!(field.as_str(), "id" | "name") && field.as_str() != kind)
        {
            return Err(FhirError::invalid(format!(
                "unsupported element `{field}` on operation parameter `{name}`"
            ))
            .at(format!("{location}.{field}")));
        }
        if kind != parameter.value_key {
            return Err(FhirError::invalid(format!(
                "operation parameter `{name}` must use `{}`, not `{kind}`",
                parameter.value_key
            ))
            .at(format!("{location}.{kind}")));
        }
        if kind == "resource" {
            if name != "valueSet" || !operation.allows_inline_value_set() {
                return Err(FhirError::invalid(format!(
                    "structured resource parameter `{name}` is not supported by this operation"
                ))
                .at(format!("{location}.resource")));
            }
            if !value.is_object() {
                return Err(FhirError::invalid("`valueSet` resource must be an object")
                    .at(format!("{location}.resource")));
            }
            if inline_value_set.is_some() {
                return Err(FhirError::invalid(
                    "operation parameter `valueSet` must not be repeated",
                )
                .at(&location));
            }
            if params.iter().any(|(key, _)| key == "valueSet") {
                return Err(FhirError::invalid(
                    "conflicting `valueSet` values were supplied in the query and POST body",
                )
                .at(&location));
            }
            inline_value_set = Some(value.clone());
            continue;
        }

        if kind == "part" || !is_supported_primitive(kind) {
            return Err(FhirError::invalid(format!(
                "structured datatype `{kind}` for operation parameter `{name}` is not supported"
            ))
            .at(format!("{location}.{kind}")));
        }

        let primitive = match kind {
            "valueBoolean" => value
                .as_bool()
                .map(|value| value.to_string())
                .ok_or_else(|| {
                    FhirError::invalid(format!("`{kind}` must be a JSON boolean"))
                        .at(format!("{location}.{kind}"))
                })?,
            "valueInteger" => value
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .map(|value| value.to_string())
                .ok_or_else(|| {
                    FhirError::invalid(format!(
                        "`{kind}` must be a JSON integer in the FHIR integer range"
                    ))
                    .at(format!("{location}.{kind}"))
                })?,
            "valueString" | "valueUri" | "valueCode" | "valueCanonical" | "valueDateTime" => {
                value.as_str().map(str::to_string).ok_or_else(|| {
                    FhirError::invalid(format!("`{kind}` must be a JSON string"))
                        .at(format!("{location}.{kind}"))
                })?
            }
            _ => unreachable!("supported primitive checked above"),
        };
        if let Some(requirement) = invalid_operation_primitive(name, kind, &primitive, false) {
            return Err(FhirError::invalid(format!("`{kind}` {requirement}"))
                .at(format!("{location}.{kind}")));
        }
        body_values.push((name.to_string(), primitive, index));
    }

    let mut names = Vec::new();
    for (name, _, _) in &body_values {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    for name in names {
        let values: Vec<String> = body_values
            .iter()
            .filter(|(candidate, _, _)| candidate == &name)
            .map(|(_, value, _)| value.clone())
            .collect();
        let parameter = operation
            .parameter(&name)
            .expect("body parameter was checked above");
        if !parameter.repeatable && values.len() > 1 {
            let index = body_values
                .iter()
                .filter(|(candidate, _, _)| candidate == &name)
                .nth(1)
                .map(|(_, _, index)| *index)
                .unwrap_or(0);
            return Err(FhirError::invalid(format!(
                "operation parameter `{name}` must not be repeated"
            ))
            .at(format!("Parameters.parameter[{index}]")));
        }
        let query_values = params_all(&params, &name);
        if query_values.is_empty() {
            params.extend(values.into_iter().map(|value| (name.clone(), value)));
        } else if query_values != values {
            let index = body_values
                .iter()
                .find(|(candidate, _, _)| candidate == &name)
                .map(|(_, _, index)| *index)
                .unwrap_or(0);
            return Err(FhirError::invalid(format!(
                "conflicting `{name}` values were supplied in the query and POST body"
            ))
            .at(format!("Parameters.parameter[{index}]")));
        }
    }

    Ok(OperationInput {
        params,
        inline_value_set,
    })
}

fn validate_operation_query(
    operation: OperationKind,
    params: &[(String, String)],
) -> Result<(), FhirError> {
    for (name, _) in params {
        let parameter = operation.parameter(name).ok_or_else(|| {
            FhirError::invalid(format!(
                "query parameter `{name}` is not defined for this operation"
            ))
            .at(name)
        })?;
        if !parameter.repeatable && params.iter().filter(|(key, _)| key == name).count() > 1 {
            return Err(FhirError::invalid(format!(
                "operation parameter `{name}` must not be repeated"
            ))
            .at(name));
        }
    }
    Ok(())
}

fn validate_operation_values(
    operation: OperationKind,
    params: &[(String, String)],
) -> Result<(), FhirError> {
    for (name, value) in params {
        let parameter = operation
            .parameter(name)
            .expect("operation parameter names were checked at the request boundary");
        match parameter.value_key {
            "valueBoolean" if !matches!(value.as_str(), "true" | "false") => {
                return Err(FhirError::invalid(format!(
                    "query parameter `{name}` must be a FHIR boolean (`true` or `false`)"
                ))
                .at(name));
            }
            "valueInteger" if !valid_fhir_integer(value) => {
                return Err(FhirError::invalid(format!(
                    "query parameter `{name}` must be a FHIR integer"
                ))
                .at(name));
            }
            "resource" | "part" | "valueCoding" | "valueCodeableConcept" => {
                return Err(FhirError::invalid(format!(
                    "structured operation parameter `{name}` cannot be supplied in the query string"
                ))
                .at(name));
            }
            _ => {}
        }
        if let Some(requirement) =
            invalid_operation_primitive(name, parameter.value_key, value, true)
        {
            return Err(
                FhirError::invalid(format!("query parameter `{name}` {requirement}")).at(name),
            );
        }
    }
    Ok(())
}

fn is_supported_primitive(value_key: &str) -> bool {
    matches!(
        value_key,
        "valueBoolean"
            | "valueInteger"
            | "valueString"
            | "valueUri"
            | "valueCode"
            | "valueCanonical"
            | "valueDateTime"
    )
}

fn invalid_fhir_primitive(
    value_key: &str,
    value: &str,
    allow_empty_string: bool,
) -> Option<&'static str> {
    match value_key {
        "valueString" if !valid_fhir_string(value, allow_empty_string) => {
            Some("must be a valid FHIR string of at most 1 MiB")
        }
        "valueUri" if !valid_fhir_uri(value) => {
            Some("must be a valid FHIR uri without whitespace or control characters")
        }
        "valueCanonical" if !valid_fhir_canonical(value) => {
            Some("must be a valid FHIR canonical without whitespace or control characters")
        }
        "valueCode" if !valid_fhir_code(value) => {
            Some("must be a valid FHIR code with no leading, trailing, or repeated whitespace")
        }
        "valueDateTime" if !valid_fhir_date_time(value) => Some("must be a valid FHIR dateTime"),
        _ => None,
    }
}

fn invalid_operation_primitive(
    name: &str,
    value_key: &str,
    value: &str,
    allow_empty_string: bool,
) -> Option<&'static str> {
    // The SNOMED implicit ECL canonical is conventionally supplied as one
    // form-encoded `url` parameter, so percent-decoding exposes ECL spaces.
    // Keep that established form while rejecting whitespace in other URIs.
    if name == "url" && value_key == "valueUri" {
        let suffix = value
            .strip_prefix(fhir::SNOMED_SYSTEM)
            .and_then(|value| value.strip_prefix("?fhir_vs=ecl/"));
        if suffix.is_some_and(|ecl| {
            !ecl.is_empty()
                && ecl.chars().all(|character| {
                    character == ' ' || (!character.is_whitespace() && !character.is_control())
                })
        }) {
            return None;
        }
    }
    invalid_fhir_primitive(value_key, value, allow_empty_string)
}

fn valid_fhir_string(value: &str, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty())
        && value.chars().count() <= 1024 * 1024
        && value
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\t' | '\r' | '\n'))
}

fn valid_fhir_uri(value: &str) -> bool {
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return false;
    }
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (bytes
                .get(index + 1)
                .is_none_or(|byte| !byte.is_ascii_hexdigit())
                || bytes
                    .get(index + 2)
                    .is_none_or(|byte| !byte.is_ascii_hexdigit()))
        {
            return false;
        }
    }
    let base = url::Url::parse("http://fhir.invalid/").expect("static URL is valid");
    url::Url::options()
        .base_url(Some(&base))
        .parse(value)
        .is_ok()
}

fn valid_fhir_canonical(value: &str) -> bool {
    let mut parts = value.split('|');
    let Some(uri) = parts.next() else {
        return false;
    };
    let version = parts.next();
    parts.next().is_none()
        && valid_fhir_uri(uri)
        && version.is_none_or(|version| {
            version
                .chars()
                .all(|character| !character.is_whitespace() && !character.is_control())
        })
}

fn valid_fhir_code(value: &str) -> bool {
    !value.is_empty()
        && value.split(' ').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|character| !character.is_whitespace() && !character.is_control())
        })
}

fn valid_fhir_integer(value: &str) -> bool {
    let digits = value
        .strip_prefix('-')
        .or_else(|| value.strip_prefix('+'))
        .unwrap_or(value);
    !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'))
        && !(digits == "0" && (value.starts_with('-') || value.starts_with('+')))
        && value.parse::<i32>().is_ok()
}

fn valid_fhir_date_time(value: &str) -> bool {
    if !value.is_ascii() || value.len() < 4 || !value[..4].bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    if &value[..4] == "0000" {
        return false;
    }
    match value.len() {
        4 => return true,
        7 => {
            return value.as_bytes()[4] == b'-'
                && value[5..7]
                    .parse::<u8>()
                    .is_ok_and(|month| (1..=12).contains(&month));
        }
        10 => return chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok(),
        _ => {}
    }
    if value.as_bytes().get(10) != Some(&b'T')
        || chrono::NaiveDate::parse_from_str(&value[..10], "%Y-%m-%d").is_err()
    {
        return false;
    }
    let time_and_zone = &value[11..];
    let clock = if let Some(clock) = time_and_zone.strip_suffix('Z') {
        clock
    } else if time_and_zone.len() >= 6 {
        let split = time_and_zone.len() - 6;
        let zone = &time_and_zone[split..];
        let zone_bytes = zone.as_bytes();
        if !matches!(zone_bytes[0], b'+' | b'-') || zone_bytes[3] != b':' {
            return false;
        }
        let Ok(hours) = zone[1..3].parse::<u8>() else {
            return false;
        };
        let Ok(minutes) = zone[4..6].parse::<u8>() else {
            return false;
        };
        if hours > 14 || minutes > 59 || (hours == 14 && minutes != 0) {
            return false;
        }
        &time_and_zone[..split]
    } else {
        return false;
    };
    let (clock, fraction) = match clock.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (clock, None),
    };
    if fraction.is_some_and(|fraction| {
        fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    }) || clock.len() != 8
        || clock.as_bytes()[2] != b':'
        || clock.as_bytes()[5] != b':'
    {
        return false;
    }
    let (Ok(hours), Ok(minutes), Ok(seconds)) = (
        clock[..2].parse::<u8>(),
        clock[3..5].parse::<u8>(),
        clock[6..8].parse::<u8>(),
    ) else {
        return false;
    };
    hours <= 23 && minutes <= 59 && seconds <= 60
}

#[derive(Debug)]
enum InlineValueSetTarget {
    /// `None` means the whole loaded SNOMED CT code system.
    Ecl(Option<String>),
    Members(Vec<(String, String)>),
}

#[derive(Debug)]
struct InlineValueSet {
    target: InlineValueSetTarget,
    canonical: String,
    definition: serde_json::Value,
}

/// Parse the inline ValueSet forms that map exactly to existing server
/// semantics: the whole loaded SNOMED code system, one ECL constraint, or an
/// extensional list of SNOMED concepts with optional explicit exclusions.
fn parse_inline_value_set(value: serde_json::Value) -> Result<InlineValueSet, FhirError> {
    let invalid = |message: String| {
        FhirError::invalid(message).at("Parameters.parameter.where(name='valueSet').resource")
    };
    if value.get("resourceType").and_then(|v| v.as_str()) != Some("ValueSet") {
        return Err(invalid(
            "inline `valueSet` resourceType must be `ValueSet`".to_string(),
        ));
    }
    let object = value
        .as_object()
        .expect("a ValueSet resource is a JSON object");
    if let Some(field) = object.keys().find(|field| {
        !matches!(
            field.as_str(),
            "resourceType"
                | "id"
                | "url"
                | "version"
                | "name"
                | "title"
                | "status"
                | "experimental"
                | "date"
                | "publisher"
                | "description"
                | "immutable"
                | "purpose"
                | "copyright"
                | "compose"
        )
    }) {
        return Err(invalid(format!(
            "inline `valueSet.{field}` is not supported"
        )));
    }
    for field in [
        "id",
        "url",
        "version",
        "name",
        "title",
        "status",
        "date",
        "publisher",
        "description",
        "purpose",
        "copyright",
    ] {
        if object.get(field).is_some_and(|value| !value.is_string()) {
            return Err(invalid(format!(
                "inline `valueSet.{field}` must be a string"
            )));
        }
    }
    for field in [
        "version",
        "name",
        "title",
        "publisher",
        "description",
        "purpose",
        "copyright",
    ] {
        if object
            .get(field)
            .and_then(|value| value.as_str())
            .is_some_and(|value| !valid_fhir_string(value, false))
        {
            return Err(invalid(format!(
                "inline `valueSet.{field}` must be a valid non-empty FHIR string"
            )));
        }
    }
    if object
        .get("id")
        .and_then(|value| value.as_str())
        .is_some_and(|value| {
            value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
        })
    {
        return Err(invalid(
            "inline `valueSet.id` must be a valid FHIR id".to_string(),
        ));
    }
    if object
        .get("url")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !valid_fhir_uri(value))
    {
        return Err(invalid(
            "inline `valueSet.url` must be a valid FHIR uri without whitespace or control characters"
                .to_string(),
        ));
    }
    if object
        .get("date")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !valid_fhir_date_time(value))
    {
        return Err(invalid(
            "inline `valueSet.date` must be a valid FHIR dateTime".to_string(),
        ));
    }
    for field in ["experimental", "immutable"] {
        if object.get(field).is_some_and(|value| !value.is_boolean()) {
            return Err(invalid(format!(
                "inline `valueSet.{field}` must be a boolean"
            )));
        }
    }
    let status = object
        .get("status")
        .and_then(|value| value.as_str())
        .ok_or_else(|| invalid("inline `valueSet.status` is required".to_string()))?;
    if !matches!(status, "draft" | "active" | "retired" | "unknown") {
        return Err(invalid(format!(
            "inline `valueSet.status` must be draft, active, retired, or unknown, not `{status}`"
        )));
    }
    let canonical = object
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("inline ValueSet")
        .to_string();
    let compose = object
        .get("compose")
        .and_then(|v| v.as_object())
        .ok_or_else(|| invalid("inline `valueSet.compose` is required".to_string()))?;
    let definition = value.clone();
    if let Some(field) = compose
        .keys()
        .find(|field| !matches!(field.as_str(), "include" | "exclude"))
    {
        return Err(invalid(format!(
            "inline `valueSet.compose.{field}` is not supported"
        )));
    }
    let includes = compose
        .get("include")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid("inline `valueSet.compose.include` must be an array".to_string()))?;
    if includes.is_empty() {
        return Err(invalid(
            "inline `valueSet.compose.include` must not be empty".to_string(),
        ));
    }
    let excludes = match compose.get("exclude") {
        None => &[][..],
        Some(value) => value.as_array().ok_or_else(|| {
            invalid("inline `valueSet.compose.exclude` must be an array".to_string())
        })?,
    };

    if includes.len() == 1 && excludes.is_empty() {
        let include = inline_compose_group(&includes[0], "include", 0, &invalid)?;
        if let InlineComposeGroup::Ecl(ecl) = include {
            return Ok(InlineValueSet {
                target: InlineValueSetTarget::Ecl(Some(ecl)),
                canonical,
                definition,
            });
        }
        if matches!(include, InlineComposeGroup::All) {
            return Ok(InlineValueSet {
                target: InlineValueSetTarget::Ecl(None),
                canonical,
                definition,
            });
        }
    }

    let mut members: Vec<(String, String)> = Vec::new();
    let mut includes_all = false;
    for (index, group) in includes.iter().enumerate() {
        match inline_compose_group(group, "include", index, &invalid)? {
            InlineComposeGroup::Members(group_members) => {
                for member in group_members {
                    if !members.iter().any(|(code, _)| code == &member.0) {
                        members.push(member);
                    }
                }
            }
            InlineComposeGroup::All => includes_all = true,
            InlineComposeGroup::Ecl(_) => {
                return Err(invalid(
                    "inline ECL filters cannot be combined with other include or exclude groups"
                        .to_string(),
                ))
            }
        }
    }

    let mut excludes_all = false;
    let mut excluded = std::collections::HashSet::new();
    for (index, group) in excludes.iter().enumerate() {
        match inline_compose_group(group, "exclude", index, &invalid)? {
            InlineComposeGroup::Members(group_members) => {
                excluded.extend(group_members.into_iter().map(|(code, _)| code));
            }
            InlineComposeGroup::All => excludes_all = true,
            InlineComposeGroup::Ecl(_) => {
                return Err(invalid(
                    "inline ECL filters are not supported in exclude groups".to_string(),
                ))
            }
        }
    }
    if includes_all && !excludes_all {
        return Err(invalid(
            "an all-SNOMED include can only be combined with an all-SNOMED exclude (the exact empty set)"
                .to_string(),
        ));
    }
    if excludes_all {
        if !includes_all || !members.is_empty() || !excluded.is_empty() {
            return Err(invalid(
                "an all-SNOMED exclude is supported only with one all-SNOMED include".to_string(),
            ));
        }
        members.clear();
    } else {
        members.retain(|(code, _)| !excluded.contains(code));
    }
    Ok(InlineValueSet {
        target: InlineValueSetTarget::Members(members),
        canonical,
        definition,
    })
}

enum InlineComposeGroup {
    All,
    Ecl(String),
    Members(Vec<(String, String)>),
}

fn inline_compose_group<F>(
    value: &serde_json::Value,
    kind: &str,
    index: usize,
    invalid: &F,
) -> Result<InlineComposeGroup, FhirError>
where
    F: Fn(String) -> FhirError,
{
    let group = value.as_object().ok_or_else(|| {
        invalid(format!(
            "inline `valueSet.compose.{kind}[{index}]` must be an object"
        ))
    })?;
    if let Some(field) = group
        .keys()
        .find(|field| !matches!(field.as_str(), "system" | "concept" | "filter"))
    {
        return Err(invalid(format!(
            "inline `valueSet.compose.{kind}[{index}].{field}` is not supported"
        )));
    }
    let system = group.get("system").and_then(|v| v.as_str());
    if system != Some(fhir::SNOMED_SYSTEM) {
        return Err(invalid(format!(
            "inline `valueSet.compose.{kind}[{index}].system` must be {}",
            fhir::SNOMED_SYSTEM
        )));
    }
    let concepts = group.get("concept");
    let filters = group.get("filter");
    if concepts.is_some() && filters.is_some() {
        return Err(invalid(format!(
            "inline `valueSet.compose.{kind}[{index}]` cannot combine concept and filter"
        )));
    }
    if let Some(filters) = filters {
        let filters = filters.as_array().ok_or_else(|| {
            invalid(format!(
                "inline `valueSet.compose.{kind}[{index}].filter` must be an array"
            ))
        })?;
        if filters.len() != 1 {
            return Err(invalid(
                "inline ValueSet filters must contain exactly one SNOMED `constraint =` filter"
                    .to_string(),
            ));
        }
        let filter = filters[0]
            .as_object()
            .ok_or_else(|| invalid("inline ValueSet filter must be an object".to_string()))?;
        if let Some(field) = filter
            .keys()
            .find(|field| !matches!(field.as_str(), "property" | "op" | "value"))
        {
            return Err(invalid(format!(
                "inline ValueSet filter element `{field}` is not supported"
            )));
        }
        if filter.get("property").and_then(|v| v.as_str()) != Some("constraint")
            || filter.get("op").and_then(|v| v.as_str()) != Some("=")
        {
            return Err(invalid(
                "only the SNOMED `constraint =` inline ValueSet filter is supported".to_string(),
            ));
        }
        let ecl = filter
            .get("value")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| invalid("inline ValueSet ECL constraint is empty".to_string()))?;
        return Ok(InlineComposeGroup::Ecl(ecl.to_string()));
    }
    let Some(concepts) = concepts else {
        return Ok(InlineComposeGroup::All);
    };
    let concepts = concepts.as_array().ok_or_else(|| {
        invalid(format!(
            "inline `valueSet.compose.{kind}[{index}].concept` must be an array"
        ))
    })?;
    if concepts.is_empty() {
        return Err(invalid(format!(
            "inline `valueSet.compose.{kind}[{index}].concept` must not be empty"
        )));
    }
    let mut members = Vec::with_capacity(concepts.len());
    for (concept_index, concept) in concepts.iter().enumerate() {
        let concept = concept.as_object().ok_or_else(|| {
            invalid(format!(
                "inline `valueSet.compose.{kind}[{index}].concept[{concept_index}]` must be an object"
            ))
        })?;
        if let Some(field) = concept
            .keys()
            .find(|field| !matches!(field.as_str(), "code" | "display"))
        {
            return Err(invalid(format!(
                "inline `valueSet.compose.{kind}[{index}].concept[{concept_index}].{field}` is not supported"
            )));
        }
        let code = concept
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                invalid(format!(
                    "inline `valueSet.compose.{kind}[{index}].concept[{concept_index}].code` is required"
                ))
            })?;
        crate::sctid::validate_syntax(code).map_err(|error| {
            invalid(format!(
                "invalid SCTID in inline ValueSet concept `{code}`: {error}"
            ))
        })?;
        let display = match concept.get("display") {
            None => String::new(),
            Some(value) => value
                .as_str()
                .ok_or_else(|| {
                    invalid(format!(
                        "inline `valueSet.compose.{kind}[{index}].concept[{concept_index}].display` must be a string"
                    ))
                })
                .and_then(|display| {
                    if valid_fhir_string(display, false) {
                        Ok(display.to_string())
                    } else {
                        Err(invalid(format!(
                            "inline `valueSet.compose.{kind}[{index}].concept[{concept_index}].display` must be a valid non-empty FHIR string"
                        )))
                    }
                })?,
        };
        members.push((code.to_string(), display));
    }
    Ok(InlineComposeGroup::Members(members))
}

/// Parse a raw query string into key/value pairs, percent-decoding both sides
/// (and `+` → space). Handles repeated keys (FHIR uses `property=` repeatedly).
fn parse_query(raw: &str) -> Result<Vec<(String, String)>, FhirError> {
    raw.split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let mut it = pair.splitn(2, '=');
            let k = pct_decode(it.next().unwrap_or(""))?;
            let v = pct_decode(it.next().unwrap_or(""))?;
            Ok((k, v))
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

/// Maximum request body size accepted by `sct serve`. Operation routes parse a
/// FHIR `Parameters` resource and the base route parses a batch Bundle; neither
/// needs a large body. 1 MiB is generous for legitimate requests and stops a
/// malicious or buggy client from exhausting memory with an unbounded `String`
/// extractor.
const MAX_BODY_BYTES: usize = 1024 * 1024;

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
/// `spec/commands/serve.md`; it applies to implicit and inline SNOMED
/// expansions. A stored `.codelist` ValueSet's fixed member list
/// (`ops::expand_members`) deliberately does not honour it.
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

/// The versions an `$expand` request requires the SNOMED CT system to be at,
/// each tagged with the query parameter it was read from so a mismatch can
/// name the parameter the client actually sent.
///
/// `check-system-version` asserts a version outright. `system-version` supplies
/// one "if the value set does not specify which one to use" - and an implicit
/// SNOMED value set never does, so for this server the two amount to the same
/// requirement, and both must be checked rather than one silently ignored.
fn version_pins(params: &[(String, String)]) -> Vec<(&'static str, String)> {
    let mut pins: Vec<(&'static str, String)> = params_all(params, "check-system-version")
        .into_iter()
        .map(|v| ("check-system-version", v))
        .collect();
    pins.extend(
        params_all(params, "system-version")
            .into_iter()
            .map(|v| ("system-version", v)),
    );
    pins
}

/// Refuse an `$expand` request this server cannot honour, instead of quietly
/// expanding something else.
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
fn unsupported_expand_input(params: &[(String, String)]) -> Option<FhirError> {
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
/// structured `Coding` datatype. It is refused by name rather than silently
/// discarded - the same shape of bug `$expand`'s `valueSet` handling was fixed
/// for under R17.
/// `date` asks for concept information as of a historical point in time,
/// which this single-release server cannot serve, matching `$expand`'s
/// `date` handling.
fn unsupported_lookup_input(params: &[(String, String)]) -> Option<FhirError> {
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
/// server accepts bare `codeA`/`codeB` SCTIDs only, so each is refused by name
/// rather than silently discarded.
fn unsupported_subsumes_input(params: &[(String, String)]) -> Option<FhirError> {
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
/// all structured datatypes normally supplied inline or in a POST body; each
/// is refused by name rather than silently discarded. `date` matches
/// `$lookup` and `$expand`'s handling: a single-release server cannot
/// validate as of a past point in time.
fn unsupported_validate_code_input(params: &[(String, String)]) -> Option<FhirError> {
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
fn unsupported_vs_validate_code_input(params: &[(String, String)]) -> Option<FhirError> {
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

/// Refuse a `ConceptMap/$translate` request this server cannot honour, instead
/// of quietly answering with different input (roadmap `R17b-translate`).
/// `url`/`conceptMap`/`conceptMapVersion` all name or supply a distinct,
/// versioned `ConceptMap` resource; this server has no such notion - it holds
/// one integrated cross-terminology map derived from the single loaded
/// release, selected by `system`/`targetsystem` alone. `source`/`target` are
/// ValueSet-scoping URIs (not the same parameter as `targetsystem`, which
/// names a bare code system) that this server cannot resolve - previously
/// `target` was silently treated as an alias for `targetsystem`, which is
/// wrong per the R4 definition and is fixed here to a refusal instead.
/// `coding`/`codeableConcept` are structured datatypes normally supplied
/// inline or in a POST body; `dependency` is a context-dependent mapping this
/// server's flat crossmap tables cannot represent; `reverse` would require
/// swapping which side of the mapping is authoritative, which is not
/// implemented.
fn unsupported_translate_input(params: &[(String, String)]) -> Option<FhirError> {
    const UNSUPPORTED: [(&str, &str); 8] = [
        (
            "url",
            "this server holds one integrated cross-terminology map for the loaded release \
             rather than separate ConceptMap resources by canonical URL; supply `system`, \
             `code`, and `targetsystem` instead",
        ),
        (
            "conceptMap",
            "an inline ConceptMap resource cannot be honoured; this server only translates \
             through its own loaded cross-terminology maps",
        ),
        (
            "conceptMapVersion",
            "only meaningful alongside `url`, which is not supported; there is no separate \
             ConceptMap version to pin",
        ),
        (
            "source",
            "this server does not scope translation by an arbitrary source ValueSet; supply \
             `system` and `code` directly",
        ),
        (
            "coding",
            "supply separate `system` and `code` query parameters instead",
        ),
        (
            "codeableConcept",
            "supply separate `system` and `code` query parameters instead",
        ),
        (
            "target",
            "this server does not scope translation by an arbitrary target ValueSet; supply \
             `targetsystem` instead",
        ),
        (
            "dependency",
            "context-dependent mapping dependencies are not supported",
        ),
    ];
    if let Some(e) = UNSUPPORTED.iter().find_map(|(name, hint)| {
        param(params, name).map(|_| {
            FhirError::invalid(format!(
                "`{name}` is not supported by this server: {hint}. It is refused rather than \
                 ignored, because ignoring it would silently answer using different input"
            ))
        })
    }) {
        return Some(e);
    }
    if let Some(v) = param(params, "reverse") {
        if v.eq_ignore_ascii_case("true") {
            return Some(FhirError::invalid(
                "`reverse` is not supported by this server: reverse translation (finding \
                 codes that map to a given target code, rather than translating a source code) \
                 is not implemented. It is refused rather than ignored, because ignoring it \
                 would silently answer using different input",
            ));
        }
    }
    None
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

fn pct_decode(s: &str) -> Result<String, FhirError> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let Some((high, low)) = b.get(i + 1).zip(b.get(i + 2)) else {
                    return Err(FhirError::invalid(
                        "query string contains incomplete percent-encoding",
                    ));
                };
                let Some(high) = (*high as char).to_digit(16) else {
                    return Err(FhirError::invalid(
                        "query string contains malformed percent-encoding",
                    ));
                };
                let Some(low) = (*low as char).to_digit(16) else {
                    return Err(FhirError::invalid(
                        "query string contains malformed percent-encoding",
                    ));
                };
                out.push(((high << 4) | low) as u8);
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| {
        FhirError::invalid("query string contains percent-encoding that is not valid UTF-8")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repeated_and_encoded_params() {
        let params =
            parse_query("code=22298006&property=parent&property=child&filter=heart+attack")
                .unwrap();
        assert_eq!(param(&params, "code"), Some("22298006"));
        assert_eq!(params_all(&params, "property"), vec!["parent", "child"]);
        assert_eq!(param(&params, "filter"), Some("heart attack"));
    }

    #[test]
    fn parses_encoded_implicit_valueset_url_without_splitting_its_query() {
        let params = parse_query(
            "url=http%3A%2F%2Fsnomed.info%2Fsct%3Ffhir_vs%3Decl%2F22298006&includeDesignations=true",
        )
        .unwrap();
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
    /// silently *widens* the result.
    #[test]
    fn expand_refuses_inputs_it_would_otherwise_silently_ignore() {
        for name in [
            "valueSet",
            "context",
            "date",
            "exclude-system",
            "force-system-version",
        ] {
            let refused = unsupported_expand_input(&parse_query(&format!("{name}=x")).unwrap())
                .unwrap_or_else(|| panic!("`{name}` must be refused, not ignored"));
            assert_eq!(refused.status, 400);
            assert!(refused.diagnostics.contains(name));
        }

        assert!(unsupported_expand_input(
            &parse_query("url=http://snomed.info/sct?fhir_vs&count=10&activeOnly=false").unwrap(),
        )
        .is_none());
    }

    /// `system-version` supplies a version when the value set does not specify
    /// one, which an implicit SNOMED value set never does - so it carries the
    /// same force here as `check-system-version` and must be checked too.
    #[test]
    fn version_pins_cover_both_spellings() {
        let pins = version_pins(
            &parse_query(
                "check-system-version=http://snomed.info/sct|a&system-version=http://snomed.info/sct|b",
            )
            .unwrap(),
        );
        assert_eq!(
            pins,
            vec![
                (
                    "check-system-version",
                    "http://snomed.info/sct|a".to_string()
                ),
                ("system-version", "http://snomed.info/sct|b".to_string()),
            ]
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
        // The bare `refset` form - the set of reference sets - is its own kind
        // of query, not an ECL reduction.
        assert_eq!(p(&format!("{sct}?fhir_vs=refset")), Some(Refsets));

        // Malformed forms: named, not guessed.
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
        assert_eq!(
            implicit_expand_target(None).unwrap(),
            ExpandTarget::Ecl(None)
        );
        assert_eq!(
            implicit_expand_target(Some("http://snomed.info/sct?fhir_vs")).unwrap(),
            ExpandTarget::Ecl(None)
        );
        assert_eq!(
            implicit_expand_target(Some("http://snomed.info/sct?fhir_vs=isa/73211009")).unwrap(),
            ExpandTarget::Ecl(Some("<<73211009".to_string()))
        );
        assert_eq!(
            implicit_expand_target(Some("http://snomed.info/sct?fhir_vs=refset")).unwrap(),
            ExpandTarget::Refsets,
            "the bare refset form is now implemented, not refused"
        );

        let unknown = implicit_expand_target(Some("http://example.org/ValueSet/nope"))
            .expect_err("an unknown value set must not expand");
        assert_eq!(unknown.status, 404);

        let unsupported = implicit_expand_target(Some("http://snomed.info/sct?fhir_vs=nonsense/1"))
            .expect_err("an unimplemented form must not expand");
        assert_eq!(unsupported.status, 400);
        assert!(unsupported.diagnostics.contains("nonsense/1"));
    }

    #[test]
    fn implicit_identifier_slots_reject_ecl_syntax() {
        for form in ["isa", "refset"] {
            for id in [
                "",
                "73211009 OR 22298006",
                "73211009 MINUS 46635009",
                "73211009junk",
                "73211009 |Diabetes|",
                " 73211009",
                "73211009 ",
                "+73211009",
                "７３２１１００９",
                "(73211009)",
            ] {
                let url = format!("http://snomed.info/sct?fhir_vs={form}/{id}");
                assert_eq!(
                    implicit_expand_target(Some(&url)).unwrap_err().status,
                    400,
                    "{url}"
                );
            }
        }
        let expr = "<<73211009 OR 22298006";
        let url = format!("http://snomed.info/sct?fhir_vs=ecl/{expr}");
        assert_eq!(
            implicit_expand_target(Some(&url)).unwrap(),
            ExpandTarget::Ecl(Some(expr.into()))
        );
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
    fn wildcard_cors_warning_requires_a_non_loopback_bind() {
        let wildcard = vec!["*".to_string()];
        let named = vec!["https://example.org".to_string()];
        assert!(should_warn_about_wildcard_cors(false, &wildcard));
        assert!(!should_warn_about_wildcard_cors(true, &wildcard));
        assert!(!should_warn_about_wildcard_cors(false, &named));
        assert!(!should_warn_about_wildcard_cors(false, &[]));
    }

    #[test]
    fn normalises_fhir_base() {
        assert_eq!(normalise_base("/"), "");
        assert_eq!(normalise_base("/fhir"), "/fhir");
        assert_eq!(normalise_base("fhir/"), "/fhir");
    }

    #[test]
    fn advertised_url_prefers_and_validates_the_public_base() {
        let addr = "0.0.0.0:8080".parse().unwrap();
        assert_eq!(
            advertised_url(Some("https://fhir.example.org/fhir/"), addr, "/fhir").unwrap(),
            "https://fhir.example.org/fhir"
        );
        assert_eq!(
            advertised_url(None, addr, "/fhir").unwrap(),
            "http://0.0.0.0:8080/fhir"
        );
        assert_eq!(
            advertised_url(Some("http://[::1]:8080/fhir"), addr, "/fhir").unwrap(),
            "http://[::1]:8080/fhir"
        );
        for invalid in [
            "fhir.example.org/fhir",
            "ftp://fhir.example.org/fhir",
            "https://:443/fhir",
            "https://user@example.org/fhir",
            "https://fhir.example.org:0/fhir",
            "https://fhir.example.org:+80/fhir",
            "https://fhir.example.org:notaport/fhir",
            "https://fhir.example.org:99999/fhir",
            "https://[not-ip]:8080/fhir",
            "https://[]:8080/fhir",
            "https://fhir.example.org/fhir?tenant=one",
            "https://fhir.example.org/fhir#metadata",
            "https://fhir.example.org/fhir/..",
            "https://fhir.example.org/fhir/%2E%2e",
            "https://fhir.example.org/fhir/%ZZ",
        ] {
            assert!(
                advertised_url(Some(invalid), addr, "/fhir").is_err(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn pagination_defaults_caps_and_parses() {
        assert_eq!(pagination(&[]).unwrap(), (100, 0, false, true));
        assert_eq!(
            pagination(
                &parse_query("count=5000&offset=42&includeDesignations=true&activeOnly=false")
                    .unwrap(),
            )
            .unwrap(),
            (1000, 42, true, false)
        );
    }

    #[test]
    fn pagination_rejects_malformed_numbers() {
        for query in ["count=nope", "count=-1", "offset=nope", "offset=-1"] {
            assert!(
                pagination(&parse_query(query).unwrap()).is_err(),
                "accepted {query}"
            );
        }
    }

    #[test]
    fn query_parser_rejects_malformed_percent_encoding() {
        for query in ["filter=%", "filter=%2", "filter=%ZZ", "filter=%FF"] {
            let error = parse_query(query).unwrap_err();
            assert_eq!(error.status, 400);
            assert!(error.diagnostics.contains("percent-encoding"));
        }
    }

    #[test]
    fn validates_fhir_primitive_lexical_forms() {
        for valid in [
            "2026",
            "2026-10",
            "2024-02-29",
            "2026-10-14T12:34:56Z",
            "2026-10-14T12:34:60.123+14:00",
        ] {
            assert!(valid_fhir_date_time(valid), "rejected {valid}");
        }
        for invalid in [
            "0000",
            "2026-13",
            "2025-02-29",
            "2026-10-14T24:00:00Z",
            "2026-10-14T12:34:56",
            "2026-10-14T12:34:56+14:01",
        ] {
            assert!(!valid_fhir_date_time(invalid), "accepted {invalid}");
        }
        assert!(valid_fhir_code("draft status"));
        assert!(!valid_fhir_code(" draft"));
        assert!(!valid_fhir_code("draft  status"));
        assert!(valid_fhir_uri(
            "http://snomed.info/sct?fhir_vs=ecl/<<73211009"
        ));
        assert!(!valid_fhir_uri("https://example.org/bad url"));
        assert!(!valid_fhir_uri("http://["));
        assert!(valid_fhir_canonical("http://snomed.info/sct|2026-01-01"));
        assert!(valid_fhir_canonical("http://snomed.info/sct|"));
        assert!(!valid_fhir_canonical("http://[|2026-01-01"));
        for valid in ["0", "1", "-1", "+1", "2147483647", "-2147483648"] {
            assert!(valid_fhir_integer(valid), "rejected {valid}");
        }
        for invalid in ["", "01", "-0", "+0", "2147483648", "1.0"] {
            assert!(!valid_fhir_integer(invalid), "accepted {invalid}");
        }
    }
}
