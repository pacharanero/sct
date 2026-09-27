# Deployment: self-hosting `sct serve` with Docker Compose

Status: **shipped**. All four deliverables below are built: the `Caddyfile` + `docker/caddy-entrypoint.sh`, the `caddy` service in `compose.yaml`, the multi-arch Docker Hub publish job in `.github/workflows/release.yml`, and the user-facing walkthrough at [`docs/deploy/`](../docs/deploy/index.md). The Caddy layer was verified against a live stack (not just `caddy validate` syntax-checking) - proxying, CORS, the unhealthy-upstream 503, and basic auth (401/401/200 across unauthenticated/wrong/correct credentials) all confirmed working; two real bugs surfaced only by live testing are noted inline where they were fixed. A second, no-clone run path (`compose.hub.yaml`, pulling the published image instead of building) was added after this spec's original "Realistic run" below and is verified the same way - see that section. This document now records the *design record*, not outstanding work - see `spec/roadmap.md` for anything still open (e.g. GHCR as a second registry).

## Goal: the four-step self-host

The north-star UX for standing up a public FHIR terminology server:

1. **DNS** - point `fhir.example.org` at your server.
2. **`ssh` in.**
3. **Bring up the Compose stack** with a TRUD API key and a domain in env.
4. **`curl https://fhir.example.org/fhir/metadata`** works.

Between steps 3 and 4, `sct` downloads a SNOMED CT release from TRUD (under the operator's own licence), runs the full build pipeline, and starts serving, while Caddy provisions a TLS certificate - with no further operator action.

## What already exists

The repository ships a working core of this today; the remaining work is additive.

- **`Dockerfile`** - multi-stage build; the runtime layer is the static `sct` binary plus a small entrypoint. The pipeline (download, unzip, RF2 -> NDJSON -> SQLite -> TCT) is entirely in-process Rust, so the image needs no `jq` / `sqlite3` / `curl` / `unzip` at runtime.
- **`docker/entrypoint.sh`** - on start, finds a built `*.db` under `/data`; if none exists and `TRUD_API_KEY` is set, runs `sct trud download --edition … --skip-if-current --pipeline` and then serves it. Binds `0.0.0.0` (sct's CLI default of `127.0.0.1` would be invisible in a container). Passes non-`serve` arguments straight through, so `docker run sct lookup 22298006` still works as a plain CLI.
- **`compose.yaml`** - passes `TRUD_API_KEY` and the `SCT_*` config, mounts a `sct-data` named volume for persistence, and has a healthcheck with a 20-minute `start_period` to cover the first-run build.

**Gaps this spec addresses:** TLS / reverse proxy, optional auth, a *published* image (today the image builds from source), and the ergonomics of bringing the stack up.

## Design decisions

### 1. SNOMED is pulled at runtime - this is mandatory, not a convenience

SNOMED CT is licensed; it **cannot** be redistributed inside a public image. The only compliant path is for the operator to supply their own TRUD API key and let the container download under their own licence and subscription. The existing entrypoint already does this. Corollary: `TRUD_API_KEY` is a **build-time** secret only - once `/data` holds the database, the running server never contacts TRUD, so the key can be removed from a long-lived container.

### 2. TLS via Caddy, not baked into `sct serve`

`sct serve` speaks plain HTTP. Rather than teach it ACME, certificate renewal, auth, CORS, and rate-limiting, front it with [Caddy](https://caddyserver.com), whose entire configuration for this is a few lines and whose automatic-HTTPS is its headline feature. The reverse-proxy layer also gives, for free, the things a public FHIR endpoint actually needs beyond TLS: **CORS** (browser-based FHIR clients require it), request logging, gzip, and rate-limiting.

Rejected alternative: teaching `sct serve` to terminate TLS itself (e.g. `rustls-acme`). It reinvents mature Caddy functionality - ACME, renewal, auth, CORS, rate-limiting - and pulls serving concerns into the core binary. Not worth it.

### 3. Packaging: `sct` and Caddy as separate Compose services

The stack is a Docker Compose file with two services: the single-purpose `sct` image (also the "publish an image for `sct serve`" roadmap item), and Caddy's official image alongside it with a `{$DOMAIN}` Caddyfile. Each container runs one process; Caddy owns TLS termination and the public ports, and `sct` serves plain HTTP on an internal port that only Caddy reaches.

This keeps the `sct` image clean and reusable - it is still a plain CLI (`docker run sct lookup 22298006`) and composes into any stack - and it reuses Caddy's battle-tested ACME rather than reimplementing certificate handling. The cost is a `compose.yaml` on the host, so the start command is `docker compose up` rather than a bare `docker run` - a negligible delta next to the steps that dominate either way (DNS, opening ports 80/443, a TRUD subscription, and a multi-minute first-run build), and Compose is the natural shape for a multi-service stack.

### 4. Readiness during the first-run build

First start is minutes, not instant. Two mechanisms:

- The container healthcheck stays unhealthy until `/fhir/metadata` answers (already present, 20-minute `start_period`).
- Caddy should return a `503 "provisioning…"` while `sct`'s health endpoint is failing, so step 4 gives a clear signal instead of a connection refused. (Caddy `reverse_proxy` with a `handle_errors` block, or a health-gated upstream.)

## Environment-variable interface

The contract the entrypoint and Caddyfile read. Existing vars keep their current names.

### Bootstrap and data (existing)

| Variable | Default | Meaning |
|:--|:--|:--|
| `TRUD_API_KEY` | (unset) | TRUD key used only to bootstrap the database. Build-time only. |
| `SCT_TRUD_EDITION` | `uk_monolith` | TRUD edition profile to download. |
| `SCT_REFSETS` | `all` | Reference-set families to load (`none` / `simple` / `all`). |
| `SCT_LOCALE` | `en-GB` | Preferred-term locale. |
| `SCT_INCLUDE_INACTIVE` | `false` | Include inactive concepts in the build. |
| `SCT_BOOTSTRAP` | `true` | If `false`, never auto-download; require a mounted DB. |
| `SCT_DB` | (auto) | Explicit database path, overriding auto-discovery under `/data`. |
| `SCT_DATA_HOME` | `/data` | Data directory (mount a volume here). |
| `SCT_CODELISTS` | `/codelists` | Codelist registry directory. |

### Serving (existing)

| Variable | Default | Meaning |
|:--|:--|:--|
| `SCT_SERVE_HOST` | `0.0.0.0` | Bind address inside the container. |
| `SCT_SERVE_PORT` | `8080` | Internal HTTP port (Caddy fronts it). |
| `SCT_FHIR_BASE` | `/fhir` | FHIR base path. |
| `SCT_PUBLIC_URL` | `https://$DOMAIN/fhir`, or `http://localhost/fhir` for bundled local Caddy | Externally reachable absolute FHIR base URL used in metadata, derived ValueSet canonicals, and search-result `fullUrl` values. Set it explicitly when another proxy changes the scheme, host, or path. |

### TLS, proxy, auth (new - for the Caddy layer)

| Variable | Default | Meaning |
|:--|:--|:--|
| `DOMAIN` | (unset) | If set, Caddy provisions TLS for it via ACME and serves HTTPS. If unset, Caddy serves plain HTTP on `:80` (dev / behind another proxy). |
| `ACME_EMAIL` | (unset) | Let's Encrypt account email (expiry notices; recommended). |
| `BASIC_AUTH_USER` | (unset) | If set with a hash, Caddy enforces HTTP basic auth. Terminology data is non-PHI and read-only, so auth is opt-in - mainly abuse control on a public endpoint. |
| `BASIC_AUTH_HASH` | (unset) | bcrypt hash for `BASIC_AUTH_USER` (from `caddy hash-password`). Hash, not plaintext, so it is safe in `docker inspect`. |
| `CORS_ORIGINS` | `*` | Allowed CORS origins for browser FHIR clients. |

**Not built**: `SCT_AUTO_UPDATE` (re-check TRUD for a newer release on restart and rebuild) was speced but deliberately not implemented - open question 3 below was never actually resolved, and a container-restart triggering a surprise multi-GB rebuild is real, separate risk surface that deserves its own careful design rather than a bolt-on. Not part of the current env-var contract; do not reference it as if it works.

## Deliverables

All four shipped:

1. **`Caddyfile`** (+ `docker/caddy-entrypoint.sh`) driven by the env above: the site address and optional `basic_auth` block are resolved by the entrypoint wrapper rather than Caddyfile placeholders (see the file's own header comment for why - `{$VAR:default}` substitutes at parse time and cannot safely embed a colon, and `basic_auth` must be entirely absent, not present-with-dummy-credentials, when unconfigured). CORS headers, and a top-level `handle_errors 502 503 504` block for the "still starting up" message - deliberately *not* `reverse_proxy`'s nested `handle_response`, which does not fire when active health checks have marked every upstream down (verified live).
2. **A `caddy` service in `compose.yaml`**, publishing `80` + `443`, fronting the internal `sct` service (no longer directly port-published) and sharing named volumes for issued certs + Caddy config. With `DOMAIN` unset it serves plain HTTP for local/dev use.
3. **A published multi-arch image** (`linux/amd64` + `linux/arm64`) on **Docker Hub only** (`docker.io/pacharanero/sct`) - GHCR was in scope per open question 1 but not requested; trivial to add later since it reuses `GITHUB_TOKEN` with no extra secret. Built on native per-arch runners (not QEMU) and merged into one manifest via `docker buildx imagetools create`, wired into `.github/workflows/release.yml` as two jobs after the GitHub Release.
4. **Docs**: [`docs/terminology-server.md`](../docs/deploy/terminology-server.md) - an existing, better-named, already-nav-registered page turned out to already own this job; updated in place rather than duplicated into a new `docs/deployment.md`. Later split into a `docs/deploy/` section (see below) once a second run path existed to document alongside it.

## Realistic run

As actually documented in [`docs/deploy/terminology-server.md`](../docs/deploy/terminology-server.md) - `git clone` rather than piecemeal `curl -O`, since building from source needs more than a couple of files (`Dockerfile`, `docker/entrypoint.sh`, `docker/caddy-entrypoint.sh` all need to come along too):

```bash
# 1. DNS: fhir.example.org -> your server   (ACME needs this live first)
# 2. ssh your-server; clone and configure
git clone https://github.com/pacharanero/sct.git && cd sct
cp .env.example .env && $EDITOR .env
#    set TRUD_API_KEY, DOMAIN=fhir.example.org, ACME_EMAIL=you@example.org
# 3. bring the stack up
docker compose up -d --build
#    first run: downloads the UK Monolith + builds (~a few minutes); Caddy issues the cert
# 4. curl https://fhir.example.org/fhir/metadata
```

The one delta from the ideal four steps: step 4 succeeds once the first-run build and certificate issuance complete, not instantly. Fetching a compose file is the expected shape for a multi-service stack, not friction to design away.

**Revised for the published image.** The `Dockerfile` / `docker/entrypoint.sh` reasoning above only applies to building from source - once the image was actually published (deliverable 3), `Dockerfile` and `entrypoint.sh` are baked into it and never need to exist on the host. Only the `caddy` service still needs host-side config, since it bind-mounts a stock upstream image rather than a custom-built one. That leaves exactly four files to fetch (`compose.hub.yaml` as `compose.yaml`, `Caddyfile`, `docker/caddy-entrypoint.sh`, `.env.example` as `.env`) with no `git clone` at all - verified end to end against the real published `pacharanero/sct:latest` image, genuinely pulled from Docker Hub rather than built locally. Documented as the [Docker Image](../docs/deploy/docker-image.md) route, alongside the original as [Build From Source](../docs/deploy/terminology-server.md).

## Caveats to design around

- **ACME ordering.** DNS and ports 80/443 must be reachable *before* Caddy attempts issuance - hence DNS-first as step 1. In-container, order Caddy startup so it can complete the challenge.
- **Resources.** The build peaks a few GB of RAM (the TCT loads all IS-A edges into memory; FTS index build) and needs ~10 GB of disk for the UK Monolith. Document a minimum spec - and note it favourably against Snowstorm's 16 GB+.
- **Secrets.** `-e TRUD_API_KEY` is visible in `docker inspect`; fine for a personal box, but document `--env-file` / Docker secrets, and that the key is build-time-only.
- **TRUD subscription.** Download fails if the operator is not subscribed to `SCT_TRUD_EDITION`; the error must say so clearly.
- **Persistence is mandatory.** Without the `/data` volume, every restart re-downloads and rebuilds. Already handled by the `VOLUME` + `find_db` logic.

## Resolved

1. **Docker Hub, GHCR, or both?** Docker Hub only, for now - that's what was asked for. GHCR remains an easy, low-cost future addition (reuses `GITHUB_TOKEN`, no new secret) if wanted.
2. **Auth beyond basic?** Shipped basic-auth-only, matching the original lean. Nothing has surfaced a need for more.
3. **`SCT_AUTO_UPDATE`?** Deferred, not built - see the "Not built" note above. Revisit as its own piece of work if wanted, rather than resolve it implicitly by shipping an unreviewed implementation.
