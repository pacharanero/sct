# `sct-lens` - system-wide clinical terminology lookup

Status: Proposed. Programme roadmap item: `R54`. Delivery stages: `LENS-1` through `LENS-4`.

## Decision summary

- A new sibling Cargo project, `lens/`, following the exact pattern `python/` already establishes in this repo: its own `Cargo.toml`/`Cargo.lock`, `sct-rs = { path = "..", default-features = false }`, its own CI job, its own line in `s/version++` - not a formal `[workspace]` member (this repo has none; `python/` is an independent sibling project with its own lockfile, and `lens/` should be too), not a `sct` CLI feature flag, and not a separate repository.
- [Tauri](https://tauri.app) for the desktop shell: a Rust core linking `sct-rs`'s SDK in-process, using the OS's native system webview (WebView2/WebKit) for the popover, search panel, and later the ECL workbench - no bundled browser engine, matching Tauri's (and `sct`'s) small-footprint ethos.
- Global hotkeys via the `global-hotkey` crate; reading/writing the current text selection in the foreground app (simulated copy + clipboard restore) via `enigo` or `rdev`. Full parity on macOS, Windows, and Linux/X11; Linux/Wayland is a documented limitation, not silently degraded (see Architecture).
- Local-first by default: the in-process SDK link means a lookup is a local SQLite/FTS5 read, not a network call. A pluggable FHIR R4 backend (a local `sct serve` on loopback, or a remote server) remains available as an option for anyone who wants it, mirroring `codeagogo`'s configurable endpoint - but the default is fully offline, which `codeagogo` cannot offer.
- Ideas are harvested from `aehrc/codeagogo` and `aehrc/codeagogo-win` (both Apache-2.0, inbound-compatible with this repo's AGPL-3.0-or-later) - a clean-room Rust reimplementation of the useful ideas, no code copied. That would be impractical regardless: the sources are Swift/SwiftUI and C#/WPF, two entirely separate codebases that duplicate the same SCTID validator, ECL parser, and FHIR client. This is exactly the fragmentation a single Rust core avoids.
- Never bundled into the released `sct` binary, and never a build feature flag on it. It ships as its own artefact(s) with its own install path, matching a genuinely different trust and permission posture (Accessibility grant, code signing, background/tray lifecycle) from a one-shot CLI invocation.

## Why this shape, not the other two considered

**Not a `sct` CLI feature.** This isn't a CLI-subcommand shape: it's a persistent background/tray process requesting macOS Accessibility permission, ideally macOS-notarised and Windows-Authenticode-signed, with Linux autostart `.desktop` entries - a different release and trust story than "run `sct lookup` and exit." Folding that into `s/version++`'s one-release-action promise would mean every release reasoning about tray-app signing too, and the dependency set (Tauri's `tao`/`wry`, `global-hotkey`, `enigo`) shares no code with `sct gui` (a browser-facing localhost server, categorically incapable of global hotkeys or reading another app's selection). Most `cargo install sct-rs` / `brew install sct` users want a CLI, not a background process asking for OS-level permissions - that should be a visibly separate install, not a buried `--features` flag.

**Not a separate repository.** `python/` already proves "one repo, no duplicated CI" works: a second Cargo project with its own `Cargo.toml`/`Cargo.lock`, its own job block in the same `ci.yml`, its own line in `s/version++`, contributing nothing to the main `sct` binary's dependency graph. `lens/` follows the identical pattern. This captures the isolation of a separate repo (independent dependency tree, independent build/test/release) without paying for a second repo's worth of CI/Actions/Dependabot/REUSE/branch-protection setup to stand up and keep in sync with house style.

## What to harvest from codeagogo, and what to skip

`sct`'s existing engine already covers most of the *terminology* half of codeagogo's feature set; the genuinely new work is almost entirely the OS-integration shell.

| codeagogo feature | Already in `sct`? | Plan |
|---|---|---|
| Concept lookup, active/inactive status | `sct lookup`, the SDK | Reuse directly (in-process) |
| Search & insert (typeahead) | `sct lexical`/`sct sayt`, FST/FTS5 | Reuse directly |
| Verhoeff check-digit SCTID validation | No | Small, reusable addition to `sct-rs` (also useful for `sct lookup` itself) |
| Bulk replace (`ID \| term \|`, toggle) | `commands::batch` bulk-query infra | Thin adapter over existing batch queries |
| Inactive-concept warning | Partial | Ties to `R11` (concept-history story); full detail (reason, replacement) waits on it |
| ECL parse/expand/compress | `sct ecl` | Reuse directly |
| ECL pretty-print/minify | No formatter, but the parser AST exists | Small addition: a printer over the existing `src/ecl/parse.rs` AST |
| Concept visualisation (SVG/PNG) | `sct diagram` | Reuse directly, in-process |
| Multi-code-system (LOINC, RxNorm, ...) | No | Deferred - needs the generic code-system model from `R22`; codeagogo only manages this because it queries a remote server that already indexes those systems |
| Full Monaco ECL Workbench (autocomplete, hover, live eval) | No | Deferred - a real UI project on its own, not a quick win |
| Global hotkeys, system-wide selection read/write | No | The actual new work - see Architecture |

## Architecture

### One engine, one more adapter

Same dependency direction as `sdk.md`'s "one engine, several adapters": `sct-lens` is another thin adapter over `sct_rs::sdk::Snomed`, exactly like the CLI, MCP, FHIR, and Python adapters. It consumes typed query primitives; it does not reimplement lookup, search, or ECL logic.

### Crate layout

```text
lens/
  Cargo.toml       sct-rs = { path = "..", default-features = false }
  Cargo.lock       independent lockfile, exactly like python/Cargo.lock
  src/
  tauri.conf.json
  icons/
```

### Query backend

Default: in-process `Snomed::open(db_path)` - a lookup is a local read, no serialisation, no network. Optional: an HTTP client against a configured FHIR R4 base URL (a local `sct serve` on loopback, or any remote FHIR R4 terminology server), selectable in Settings. This mirrors codeagogo's configurable endpoint, but the default differs: local and offline rather than a hardcoded remote Ontoserver.

### OS integration surfaces

- Global hotkey registration - `global-hotkey` crate (the one Tauri itself uses).
- Synthetic selection read/write - simulated copy with clipboard save/restore, via `enigo` or `rdev`.
- Cursor-anchored, always-on-top popover and a system tray icon - native Tauri window/tray features.
- Launch at login - Tauri's autostart plugin.

### The Wayland caveat

X11, macOS, and Windows all permit what codeagogo does: global key listening and synthetic input from an arbitrary background app. Wayland's security model deliberately restricts both, without mature portal-based alternatives yet. Document this honestly rather than silently degrading: `LENS-1` targets macOS/Windows/Linux(X11); a Wayland fallback (e.g. binding the desktop environment's own shortcut to a small `sct-lens --lookup-clipboard` CLI trigger) is a `LENS-4` idea, not a blocker for the MVP.

## Staged delivery

### LENS-1 - hotkey, selection-read, and a lookup popover

De-risk the hard, novel part first. Global hotkey + synthetic selection read (macOS/Windows/Linux-X11) + Verhoeff-validated SCTID detection + a cursor-anchored popover showing id/PT/FSN/active status from an in-process SDK lookup against a user-configured `snomed.db`. No search, no ECL, no bulk replace, minimal settings. If the OS-integration layer doesn't work reliably here, nothing downstream matters.

### LENS-2 - search, replace, and inactive-concept awareness

Search & Insert floating panel over the SDK's lexical/FST search. Bulk Replace (`ID | term |`, smart toggle) over the existing batch-query infrastructure. Inactive-concept warning, gated on `R11` landing so it can say *why* a concept is inactive and what replaces it, not just that it is. A Settings window: hotkey customisation, database path, launch-at-login, update checks.

### LENS-3 - ECL and visualisation

ECL pretty-print/minify (a printer over the existing parser AST) with precedence warnings on ambiguous mixed AND/OR/MINUS expressions. Concept visualisation via the existing `sct diagram` engine, SVG/PNG export, invoked in-process.

### LENS-4 - deferred, each needs its own design gate

- Full Monaco-based ECL Workbench (live autocomplete, hover info) - a real UI project; whether to use `@aehrc/ecl-editor` under Apache-2.0 attribution or build a clean-room equivalent is a real license/effort tradeoff to resolve first.
- Multi-code-system support beyond SNOMED - blocked on the generic code-system model from `R22`.
- The optional pluggable remote FHIR R4 backend from the Architecture section - technically easy once `LENS-1`'s query abstraction exists, but deliberately not the default, to keep the out-of-the-box experience local-first.
- Wayland parity, pending upstream portal support maturing.

## CI and release integration

Model `python/`'s existing wiring exactly:

- A `lens` job in `.github/workflows/ci.yml`: checkout, Rust toolchain, platform-specific Tauri prerequisites (WebKitGTK on Linux runners), `cargo clippy --manifest-path lens/Cargo.toml --all-targets --locked -- -D warnings`, build against the committed synthetic fixture database, run `lens`'s own tests.
- `s/version++` gains one more version-bump line (`cargo set-version --manifest-path lens/Cargo.toml "$version"` + `cargo check --manifest-path lens/Cargo.toml --locked`) and stages `lens/Cargo.toml lens/Cargo.lock` in the release commit, exactly as it already does for `python/Cargo.toml`/`python/Cargo.lock`.
- Signed/notarised installer builds are their own workflow, separate from the existing `sct` binary release matrix, though the version bump commit is shared.
- macOS notarisation (`R34`) and Windows Authenticode signing (`R35`) are soft prerequisites for a *polished public* release, not the MVP - but note that `codeagogo-win` already ships signed (CSIRO's DigiCert certificate), so an unsigned `sct-lens` would carry a real trust/friction disadvantage on Windows if promoted beyond an internal dev build.

## Privacy and licensing

Match or beat codeagogo's existing privacy posture, which is genuinely good practice: only read the selection on hotkey press, restore the clipboard immediately after, no persistence beyond user-set config (hotkeys, db path, launch-at-login), no telemetry. Beat it on the one point that matters most here: no network call at all in the default in-process mode, where codeagogo/-win both require internet to a remote Ontoserver for every single lookup.

`sct-lens` is AGPL-3.0-or-later, matching the rest of the repo. Ideas are harvested from Apache-2.0 `codeagogo`/`codeagogo-win` (permissively licensed, inbound-compatible) with no code copied - clean-room Rust reimplementation, the same discipline `spec/commands/ecl-compress.md` already documents for its own prior art.

## Acceptance criteria

- `LENS-1`'s hotkey-to-lookup loop works on a clean macOS, Windows, and Linux(X11) machine using only the released `sct-lens` artefact and a user-supplied `snomed.db` - no repository clone.
- No network call occurs in the default in-process mode; verifiable by packet capture during a lookup.
- The Wayland limitation is documented in the README/docs, not silently broken.
- `lens/`'s CI job and `s/version++` integration mirror `python/`'s pattern closely enough that a contributor already familiar with the Python binding's release wiring needs no new documentation to maintain it.

## Deferred decisions

- Confirm the `lens/` directory name before scaffolding (assumed here to match `python/`'s bare-word convention).
- Whether `sct-lens` gets its own Homebrew cask/tap entry or reuses `pacharanero/tap`.
- The `@aehrc/ecl-editor` licensing/effort tradeoff for `LENS-4`'s Monaco workbench.
- Distribution signing budget and timeline, tied to `R34`/`R35`.
