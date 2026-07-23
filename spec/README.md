# Specifications

`spec/` is for durable design records: architecture, data contracts, rationale,
and future-facing plans that would still be useful if `sct` were reimplemented in
another language.

It is not the primary user documentation. User-facing command help belongs in
`docs/commands/` and the executable truth belongs in code and tests. When a spec
only repeats those sources, trim it or replace it with a link.

## Layout

- `spec.md` - project architecture and design principles.
- `roadmap.md` - current product direction and open work.
- `commands/<name>.md` - command-specific design records and contracts.
- `cross-terminology-mapping.md` - cross-command map/history model and DMWB
  replacement rationale.
- `ecl.md`, `sct-ql-spec.md` - query language design.
- `commands/fst.md` - FST lexical index design and benchmark record.
- `path-resolution.md` - shared path/config discovery contract.
- `sdk.md` - Rust SDK, Python bindings, WebAssembly, docs, and licensing plan.
- `gui.md` - clinical knowledge atlas product direction, Playwright feedback loop,
  accessibility criteria, and staged GUI build roadmap.
- `library-rs.md` - compatibility pointer to the superseding SDK plan.
- `bench.md` - benchmark suite contract.
- `deployment.md` - self-hosting `sct serve` with Docker Compose: TRUD
  bootstrap, a Caddy TLS service, and the env-var interface.

## Maintenance Rules

- Keep command-specific implementation plans under `spec/commands/`.
- Keep cross-cutting models at the root.
- Prefer links to `docs/commands/` for usage examples once a command has shipped.
- Keep design rationale that is not obvious from the final code.
- Remove resolved open-question files when they contain no live questions.
