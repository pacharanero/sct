# Get Your Own Terminology Server

Run a FHIR R4 SNOMED CT terminology server on a clean VPS with Docker Compose - HTTPS included. Either route produces the same stack: `sct` (builds the database on first boot and serves FHIR) behind [Caddy](https://caddyserver.com) (automatic TLS, optional basic auth, CORS). Caddy owns the public ports; `sct` is never reachable directly. See [`spec/deployment.md`](https://github.com/pacharanero/sct/blob/main/spec/deployment.md) for the design rationale.

For an exposed deployment, configure the gateway's concurrent-connection, header-read and idle-connection limits, authentication and per-client rate limits to suit the threat model. The bundled Caddy configuration does not set these limits, and basic authentication is optional. Keep the `sct` listener inaccessible to external clients; its request timeout does not protect against incomplete HTTP headers. CORS is not authentication.

Pick a route:

| | [Docker Image](docker-image.md) | [Build From Source](terminology-server.md) |
|---|---|---|
| Needs `git` | No | Yes |
| Needs a local build | No - pulls `pacharanero/sct` from Docker Hub | Yes - `docker compose up --build` |
| Files to fetch | 4 (compose file, Caddyfile, entrypoint script, env template) | Whole repo, via `git clone` |
| Upgrade | `docker compose pull && docker compose up -d` | `git pull && docker compose up -d --build` |
| Pick this when | You just want the server running | You want to patch the code, pin a commit, or build for an unpublished platform |

Most people want [Docker Image](docker-image.md) - it's the fastest path to a running server. Reach for [Build From Source](terminology-server.md) if you need to change or audit the code you're running.
