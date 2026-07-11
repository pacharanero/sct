# Deploy via Docker Image

Run a FHIR R4 SNOMED CT terminology server on a clean VPS using the published `pacharanero/sct` Docker Hub image - HTTPS included, no `git clone`, no local build. You fetch four small config files, fill in one `.env`, and `docker compose up`. The stack is still two containers: `sct` (builds the database on first boot and serves FHIR) and [Caddy](https://caddyserver.com) in front of it (automatic TLS, optional basic auth, CORS). Caddy owns the public ports; `sct` is never reachable directly.

If you'd rather build from source - to patch the code, pin a specific commit, or target a platform the published image doesn't cover - see [Build From Source](terminology-server.md) instead. Both routes produce the same running stack.

## Prerequisites

- Docker with the Compose plugin. No `git` needed.
- An NHS TRUD account subscribed to the SNOMED CT UK Monolith Edition, and your TRUD API key. It is used once, to download the release under your own licence - `sct` does not redistribute SNOMED CT content, and the running server never contacts TRUD again once the database is built.
- For real HTTPS: a domain name with its DNS **already** pointing at this server, and ports 80 + 443 reachable from the internet (Let's Encrypt needs both to issue a certificate).

## The compose file

This is the full `compose.yaml` for the image-based deployment - copy it as-is, or fetch it in [step 2](#the-four-step-self-host) below:

```yaml
--8<-- "compose.hub.yaml"
```

The only difference from building-from-source is the `sct` service's `image: pacharanero/sct:latest` in place of a `build:` block - the `caddy` service, volumes, and healthcheck are identical either way.

!!! tip "Also on GitHub Container Registry"
    The same multi-arch image is mirrored to `ghcr.io/pacharanero/sct` on every release. GHCR pulls are not rate-limited the way anonymous Docker Hub pulls are, so if your server sits behind a shared IP (CI, a cluster, a corporate NAT) and hits Docker Hub's `429 Too Many Requests`, just swap the `image:` line to `ghcr.io/pacharanero/sct:latest`.

## The four-step self-host

1. **DNS** - point `fhir.example.org` at your server. Do this first; Let's Encrypt needs it live before Caddy can request a certificate.

2. **`ssh` in**, fetch the four config files, and configure:

   ```bash
   mkdir sct-server && cd sct-server
   curl -o compose.yaml https://raw.githubusercontent.com/pacharanero/sct/main/compose.hub.yaml
   curl -O https://raw.githubusercontent.com/pacharanero/sct/main/Caddyfile
   curl -O https://raw.githubusercontent.com/pacharanero/sct/main/docker/caddy-entrypoint.sh
   curl -o .env https://raw.githubusercontent.com/pacharanero/sct/main/.env.example
   $EDITOR .env
   ```

   That's every file this deployment needs - `sct`'s own `Dockerfile` and entrypoint are already baked into the published image, so unlike building from source, nothing else needs to come along. Set at minimum in `.env`:

   ```text
   TRUD_API_KEY=your-trud-api-key
   DOMAIN=fhir.example.org
   ACME_EMAIL=you@example.org
   ```

3. **Bring the stack up:**

   ```bash
   docker compose up -d
   ```

   No `--build` - `sct` is pulled from Docker Hub, not compiled locally.

4. **`curl https://fhir.example.org/fhir/metadata`** - works once the first-run build and certificate issuance complete (see [What first boot does](#what-first-boot-does)).

## Fast path (local / no domain)

To try it locally without DNS or a real certificate, just skip `DOMAIN`:

```bash
mkdir sct-server && cd sct-server
curl -o compose.yaml https://raw.githubusercontent.com/pacharanero/sct/main/compose.hub.yaml
curl -O https://raw.githubusercontent.com/pacharanero/sct/main/Caddyfile
curl -O https://raw.githubusercontent.com/pacharanero/sct/main/docker/caddy-entrypoint.sh
TRUD_API_KEY=your-key-here docker compose up -d
```

Caddy serves plain HTTP on port 80 (no `DOMAIN` means no automatic HTTPS - there's no hostname to get a certificate for):

```bash
curl http://localhost/fhir/metadata
```

## What first boot does

If the `sct-data` volume doesn't already contain a database, the entrypoint baked into the image runs:

```bash
sct trud download --edition uk_monolith --skip-if-current --pipeline --refsets all --locale en-GB
```

That downloads the release, builds a SNOMED SQLite database (a few minutes for the UK Monolith), and starts `sct serve` on an internal port. Meanwhile Caddy:

- with `DOMAIN` set: requests a certificate via Let's Encrypt and starts serving HTTPS, redirecting HTTP to HTTPS;
- with `DOMAIN` unset: serves plain HTTP immediately.

Either way, **while `sct` is still building, Caddy returns a `503` with a clear message** ("sct is starting up (downloading/building the SNOMED database) - retry shortly.") instead of a bare connection error - so a `curl` during the first few minutes is expected and self-explanatory, not a sign anything is broken. It resolves to `200` as soon as the build finishes.

Subsequent starts reuse the existing database in the `sct-data` volume, so they skip the TRUD download and build step entirely - only the very first boot is slow.

## Check it works

```bash
curl 'https://fhir.example.org/fhir/metadata'
```

Look up a concept:

```bash
curl 'https://fhir.example.org/fhir/CodeSystem/$lookup?system=http://snomed.info/sct&code=22298006'
```

Expand an ECL ValueSet:

```bash
curl 'https://fhir.example.org/fhir/ValueSet/$expand?url=http://snomed.info/sct?fhir_vs=ecl/%3C%3C73211009&count=10'
```

(Substitute `http://localhost` for the fast-path / no-domain case.)

## Configuration

Edit `.env` - the fetched `.env.example` has the full annotated list. The essentials:

| Variable | Default | Description |
|---|---|---|
| `TRUD_API_KEY` | - | Required for first boot, unless you provide an existing database. Build-time only - the running server never uses it again. |
| `SCT_TRUD_EDITION` | `uk_monolith` | Built-in TRUD edition to download. |
| `SCT_REFSETS` | `all` | `all` enables ICD-10 / OPCS-4 maps and concept history. |
| `SCT_LOCALE` | `en-GB` | Preferred-term locale. |
| `SCT_INCLUDE_INACTIVE` | `false` | Set `true` to retain inactive concepts. |
| `DOMAIN` | *(unset)* | Your public hostname. Set it for automatic HTTPS; leave unset for plain HTTP on `:80`. |
| `ACME_EMAIL` | *(unset)* | Let's Encrypt account email (cert-expiry notices). Optional but recommended when `DOMAIN` is set. |
| `BASIC_AUTH_USER` / `BASIC_AUTH_HASH` | *(unset)* | Optional HTTP basic auth - see [below](#optional-basic-auth). Both must be set together. |
| `CORS_ORIGINS` | `*` | Allowed CORS origins for browser-based FHIR clients. |

### Optional basic auth

Terminology data is non-PHI and read-only, so authentication is opt-in - mainly abuse control on a publicly reachable endpoint. Generate a password hash with Caddy's own tool (no need to install Caddy locally - this runs it in a throwaway container):

```bash
docker run --rm caddy:2-alpine caddy hash-password --plaintext 'your-password'
```

Set both in `.env`:

```text
BASIC_AUTH_USER=yourusername
BASIC_AUTH_HASH=$2a$14$...output from the command above...
```

Leave both unset for no auth (the default). Setting only one has no effect - both are required together.

## Operations

Stop the server:

```bash
docker compose down
```

Upgrade to a newer release:

```bash
docker compose pull
docker compose up -d
```

There's no source to rebuild - `docker compose pull` fetches the latest `pacharanero/sct:latest` layer, and `up -d` recreates the container from it.

Force a fresh download/build by removing the volumes (this also discards the issued TLS certificate, which Caddy will simply re-request):

```bash
docker compose down -v
docker compose up -d
```

The `sct-data` volume contains licensed SNOMED CT data. Do not publish it as part of an image or commit exported database files to git.

## FHIR surface

The Docker setup runs the same server as [`sct serve`](../commands/serve.md), including:

- `CodeSystem/$lookup`
- `CodeSystem/$validate-code`
- `CodeSystem/$subsumes`
- `ValueSet/$expand`
- `ValueSet/$validate-code`
- `ConceptMap/$translate` when the database has crossmaps loaded

For the full operation reference, see [`sct serve`](../commands/serve.md). For the deployment design and rationale, see [`spec/deployment.md`](https://github.com/pacharanero/sct/blob/main/spec/deployment.md).
