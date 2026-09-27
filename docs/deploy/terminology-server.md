# Build From Source

Run a FHIR R4 SNOMED CT terminology server on a clean VPS with Docker Compose - HTTPS included. This route builds the `sct` image from a git clone, so it's the one to pick if you want to patch the code, pin a specific commit, or build for a platform the [published image](docker-image.md) doesn't cover. If you just want the server running, [Docker Image](docker-image.md) is faster - no clone, no build.

The stack is two containers: `sct` (builds the database on first boot and serves FHIR), and [Caddy](https://caddyserver.com) in front of it (automatic TLS, optional basic auth, CORS). Caddy owns the public ports; `sct` is never reachable directly.

## Prerequisites

- Docker with the Compose plugin.
- An NHS TRUD account subscribed to the SNOMED CT UK Monolith Edition, and your TRUD API key. It is used once, to download the release under your own licence - `sct` does not redistribute SNOMED CT content, and the running server never contacts TRUD again once the database is built.
- For real HTTPS: a domain name with its DNS **already** pointing at this server, and ports 80 + 443 reachable from the internet (Let's Encrypt needs both to issue a certificate).

## The four-step self-host

1. **DNS** - point `fhir.example.org` at your server. Do this first; Let's Encrypt needs it live before Caddy can request a certificate.
2. **`ssh` in**, clone, and configure:

   ```bash
   git clone https://github.com/pacharanero/sct.git
   cd sct
   cp .env.example .env
   $EDITOR .env
   ```

   Set at minimum:

   ```text
   TRUD_API_KEY=your-trud-api-key
   DOMAIN=fhir.example.org
   ACME_EMAIL=you@example.org
   ```

3. **Bring the stack up:**

   ```bash
   docker compose up -d --build
   ```

4. **`curl https://fhir.example.org/fhir/metadata`** - works once the first-run build and certificate issuance complete (see [What first boot does](#what-first-boot-does)).

## Fast path (local / no domain)

To try it locally without DNS or a real certificate, just skip `DOMAIN`:

```bash
git clone https://github.com/pacharanero/sct.git
cd sct
TRUD_API_KEY=your-key-here docker compose up -d --build
```

Caddy serves plain HTTP on port 80 (no `DOMAIN` means no automatic HTTPS - there's no hostname to get a certificate for):

```bash
curl http://localhost/fhir/metadata
```

## What first boot does

If the `sct-data` volume doesn't already contain a database, the entrypoint runs:

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

Edit `.env` - see `.env.example` for the full annotated list. The essentials:

| Variable | Default | Description |
|---|---|---|
| `TRUD_API_KEY` | - | Required for first boot, unless you provide an existing database. Build-time only - the running server never uses it again. |
| `SCT_TRUD_EDITION` | `uk_monolith` | Built-in TRUD edition to download. |
| `SCT_REFSETS` | `all` | `all` enables ICD-10 / OPCS-4 maps and concept history. |
| `SCT_LOCALE` | `en-GB` | Preferred-term locale. |
| `SCT_INCLUDE_INACTIVE` | `false` | Set `true` to retain inactive concepts. |
| `DOMAIN` | *(unset)* | Your public hostname. Set it for automatic HTTPS; leave unset for plain HTTP on `:80`. |
| `SCT_PUBLIC_URL` | `https://$DOMAIN/fhir`, or `http://localhost/fhir` without `DOMAIN` | Absolute FHIR base URL advertised in metadata and search results. Set it explicitly when another proxy uses a different scheme, hostname, or path. |
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

Upgrade after pulling newer code:

```bash
git pull
docker compose up -d --build
```

Force a fresh download/build by removing the volumes (this also discards the issued TLS certificate, which Caddy will simply re-request):

```bash
docker compose down -v
docker compose up -d --build
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
