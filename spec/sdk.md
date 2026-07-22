# SDK and language bindings

Status: `R1` implementation complete locally and awaiting its crates.io release; `R2` and `R3` planned. Roadmap items: `R1` (Rust SDK), `R2` (Python), `R3` (WebAssembly).

## Decision summary

- `sct` will become a first-class SDK as well as a CLI, server, and MCP tool.
- One canonical Rust engine will drive every surface. Python and WebAssembly bindings must call the Rust implementation rather than reimplement SNOMED logic.
- The documentation site will gain a top-level **SDK** navigation section with an overview and separate Rust, Python, and WebAssembly pages.
- All SDK surfaces remain `AGPL-3.0-or-later`. No permissive linking or hosted-service exception is planned.
- SNOMED CT content is never bundled in crates, wheels, npm packages, WASM bundles, examples, or the public demo. Users provide licensed content locally; tests and the public demo use the committed synthetic fixture.
- A future AGPL/commercial dual-licensing offer remains possible but is not part of these roadmap items.

## Why now

The package already has a library crate (`sct_rs`) and exposes useful building blocks: ECL parse/evaluate/expand, the mmap-backed FST index, RF2 parsing, canonical schema types, cross-terminology transcoding, codelist types, and command entry points. What it lacks is a deliberate, stable application-facing API. General concept lookup, hierarchy, refset, lexical, and semantic operations remain coupled to CLI/MCP/FHIR adapters or private helpers; several public command functions print directly to stdout/stderr; and the standard read-only database opener is crate-private.

Stabilising that boundary unlocks three surfaces at once: idiomatic embedding in Rust applications, a thin PyO3 package for Python data and research workflows, and a browser demo compiled to WebAssembly. The work should therefore be sequenced as one SDK programme rather than three unrelated wrappers.

## Architecture

### One engine, several adapters

The dependency direction is:

```text
typed Rust query engine
    <- Rust SDK facade
    <- CLI / MCP / FHIR / GUI adapters
    <- Python PyO3 adapter
    <- WebAssembly adapter + browser storage
```

The engine owns terminology semantics and typed results. Adapters own argument parsing, JSON/Python conversion, HTTP/MCP envelopes, presentation, and runtime-specific storage. No adapter contains a second implementation of ECL, subsumption, mapping, or concept lookup.

### Crate shape

Start by adding a stable `sct_rs::sdk` facade in the existing package, because `sct-rs` is already published and downstream consumers can use it today. Do not split repositories merely to create an attractive diagram. During `R1`, measure the dependency graph of `default-features = false`; if the unconditional CLI/build dependencies make bindings unnecessarily heavy, split a leaf engine crate inside the workspace before publishing Python or WASM.

The first measurement (2026-07-21, `cargo tree --no-default-features --depth 1`) showed unconditional CLI/build/export dependencies including `clap`, `clap_complete`, Arrow, Parquet, ZIP, `ureq`, `indicatif`, CSV, TOML, and `walkdir`. R1 introduced the smaller equivalent feature boundary rather than a premature crate split: the default `cli` feature owns command, build, export, network, and presentation modules, while the no-default library exposes the SDK, codelist model, ECL, FST, mapping, refset, provenance, and schema engines. The post-change normal dependency set is `anyhow`, `chrono`, `fst`, `indexmap`, `memmap2`, `rusqlite`, `serde`, `serde_json`, `serde_yaml_ng`, `sha2`, and `unicode-normalization`; none of the previously measured CLI/build/export dependencies remain.

A likely end state is:

```text
sct-core/       typed domain records, ECL parser, storage-independent contracts
sct-engine/     native SQLite/FST implementation (if a split proves useful)
src/            existing sct-rs package: CLI and public Rust SDK facade
python/         separate PyO3/maturin cdylib crate
web/            wasm-bindgen crate and static demo shell
```

This shape is a direction, not a prerequisite. The invariant is a leaf-clean reusable engine and thin adapters; the exact number of crates should remain the smallest that achieves that.

## R1 - Rust SDK

### Public facade

The public entry point should be a domain name such as `Snomed`, not command modules or CLI `Args` structs:

```rust
use sct_rs::sdk::{Snomed, Terminology};

let snomed = Snomed::open("snomed.db")?;
let concept = snomed.concept("22298006")?;
let hits = snomed.search("heart attack", 20)?;
let descendants = snomed.expand("<<73211009")?;
let relationship = snomed.subsumes("73211009", "46635009")?;
let mappings = snomed.map(Terminology::Snomed, "22298006", Terminology::Icd10)?;
```

`SnomedDb` was proposed in the original Rust-library design, but `Snomed` is preferable: consumers should depend on terminology capability, not the current SQLite implementation. The backing store remains a private implementation detail.

### Initial API

- `Snomed::open(path)` opens an `sct sqlite` artefact read-only with the same safety and mmap profile as the CLI.
- `provenance()` exposes edition, release date, release ID, source and `sct` version.
- `concept(id)`, `search(query, limit)`, `children(id, limit)`, `ancestors(id)`, `descendants(id, limit)`, `subsumes(a, b)` and `expand(ecl)` return typed results.
- `refsets()`, `refset_members(id, limit)`, `refset_compare(a, b)` and `refset_profile(id)` expose the shipped refset engine.
- `map(source, code, target)` and history forwarding expose the cross-terminology engine.
- An optional FST attachment or constructor provides exact, prefix, fuzzy, word, and search-as-you-type operations through the same facade.
- Codelist parsing/composition/validation remains available as typed Rust APIs without requiring a database where the operation does not need one.

### API rules

- Return typed, `serde`-serialisable records. Do not expose `serde_json::Value` as the primary Rust contract.
- Return a typed `SctError`; preserve source errors without making `anyhow::Error` the public contract.
- Never print from SDK methods. Hints and formatting remain adapter concerns.
- Open native databases read-only by default. Any builder/write API must be separately named and impossible to invoke accidentally through `Snomed`.
- Use `#[non_exhaustive]` on public result structs and enums where fields are expected to grow.
- Keep synchronous APIs. Native SQLite operations are synchronous and fast; async hosts can use `spawn_blocking` or a pool adapter.
- Document thread-safety explicitly. A single rusqlite connection is not a concurrent server pool.
- Establish and document the pre-1.0 compatibility policy. Breaking public changes require an intentional minor-version bump and changelog entry.

### Refactor strategy

Extract one vertical slice at a time: typed result, query method, adapter migration, fixture-backed test. Move SQL out of CLI/MCP/FHIR handlers only when its new SDK method is ready, so every commit leaves the existing surfaces working. The existing synthetic RF2 fixture remains the correctness oracle, and Myocardial infarction (`22298006`) and Type 1 diabetes mellitus (`46635009`) remain known-concept cross-checks.

### Rust SDK completion criteria

- A downstream example crate uses `sct-rs` from crates.io with `default-features = false` and exercises lookup, search, ECL, hierarchy and mapping.
- CLI, MCP and FHIR surfaces call the SDK methods for those operations rather than maintaining duplicate query implementations.
- Rustdoc examples compile in CI; `cargo doc` has no broken intra-doc links.
- `default-features = false` excludes UI/server adapters and avoids unnecessary heavyweight build/export dependencies where practical.
- User documentation gains top-level **SDK** navigation containing **Overview** and **Rust** pages; the Overview states data/licensing requirements and links to Python/WebAssembly status.

`tests/downstream-sdk/` exercises the complete downstream API shape against the local package with `default-features = false`. Its path dependency is intentionally the pre-publication form; the final R1 release check replaces that path with the released crates.io version and runs the same compile.

## R2 - Python package

### Shape

Follow the proven `clincalc/python/` pattern: a separate PyO3 `cdylib` crate built by maturin, depending on the Rust SDK with `default-features = false`. Keep PyO3, maturin, Python packaging and optional pandas dependencies out of the Rust engine's dependency graph.

Provisional Python API:

```python
import sct

with sct.Snomed("snomed.db") as snomed:
    concept = snomed.concept("22298006")
    hits = snomed.search("heart attack", limit=20)
    ids = snomed.expand("<<73211009")
    result = snomed.subsumes("73211009", "46635009")
```

The public Python package wraps a private native extension (`sct._sct` or similar), exports normal Python classes/functions, carries type hints, and converts typed Rust records to Python dictionaries/dataclasses at the edge. Long-running queries release the GIL. Rust validation/query failures map to specific Python exceptions rather than undifferentiated strings.

### Initial Python scope

- `Snomed` context manager with deterministic close.
- Concept lookup, lexical search, ECL expansion, hierarchy/subsumption, refsets, mappings and provenance.
- Batch helpers (`concepts(ids)`, `map_many(codes, ...)`) designed for Python workflows so callers do not pay one FFI crossing per item.
- Optional pandas convenience can follow the first package; it is not required to prove the FFI.
- ABI3 wheels for CPython 3.9+ on supported Linux, macOS and Windows architectures, plus a source distribution where feasible.
- Hermetic tests against the synthetic database, Python type-check/example tests, and a wheel-install smoke test in CI.
- PyPI publication integrated into the existing release cascade, with package/version drift checked before publishing.

The PyPI distribution name and Python import name must be checked immediately before implementation. `sct` is desirable as an import but may already be occupied; do not choose a confusing or squatted name merely to preserve symmetry. Candidate distribution names include `sct-rs` or `snomed-sct`, with `import sct` if available and honest.

### Python completion criteria

- A clean environment can `pip install <name>`, open a user-supplied `snomed.db`, and run the documented example.
- Wheels contain no SNOMED CT content and declare `AGPL-3.0-or-later` accurately.
- Python results match Rust SDK results over shared conformance fixtures.
- The docs **SDK** section gains a Python page covering installation, examples, supported platforms, data setup, exceptions, typing and licensing.

## R3 - WebAssembly and browser demo

### Constraint: native storage does not come for free

Compiling pure Rust functions to `wasm32-unknown-unknown` is straightforward; compiling the current native query stack is not. `rusqlite`'s bundled SQLite and `memmap2` assume native files and memory mapping, while browser WASM uses sandboxed linear memory and browser file APIs. A full UK Monolith database is also several gigabytes, close to or beyond practical browser memory limits even before indexes. The project must not pretend that adding `wasm-bindgen` makes `snomed.db` browser-compatible.

### Proposed browser data path

The preferred first design is a browser-specific, read-only artefact derived locally from canonical NDJSON, for example `release.sct-web`. A native command (provisionally `sct web build`) creates the compact artefact from the user's licensed data; the browser loads it through the File System Access API or file picker and queries it locally. Nothing is uploaded, and the public site never hosts licensed SNOMED content.

The artefact should contain only what the demo needs: concept identity/display records, a compact lexical/FST index, IS-A adjacency or closure data, provenance, and optionally selected refsets. It remains regenerable from canonical NDJSON, preserving the project's primary-data invariant. Full-edition versus subset artefacts must be benchmarked before selecting a format; a codelist/ECL-limited subset may be the realistic browser-first path.

An alternative is official SQLite WASM + OPFS behind a JavaScript storage adapter. Evaluate it against the derived artefact, but do not make the Rust engine depend on a browser SQLite implementation until the trade-off is measured. Selection criteria are initial load time, memory, query latency, browser support, implementation complexity and ability to keep all licensed bytes local.

### WASM surface and demo

- Compile storage-independent logic first: normalisation, ECL parsing, codelist parsing/composition and typed result conversion.
- Add browser query methods only through the same contracts as the Rust SDK.
- Ship a tiny synthetic artefact with the public demo so it works immediately without licensed data.
- Let users select their own locally generated artefact; show release provenance prominently and state that the file remains in the browser.
- Support lookup, autocomplete, hierarchy navigation, subsumption and a clearly documented ECL subset in the first useful demo. Do not claim full parity until shared conformance cases pass.
- Host the static demo under the existing documentation site, with no runtime backend and no analytics that could observe terminology queries or filenames.
- Consider an npm package only after the browser API stabilises; check the package name at implementation time.

### WASM completion criteria

- `wasm32-unknown-unknown` builds in CI from the canonical Rust code without native SQLite/mmap dependencies leaking into the target.
- The public demo runs against the bundled synthetic fixture and a user-selected local artefact.
- Network inspection proves that supplied terminology content and queries do not leave the browser.
- Rust native and WASM implementations pass shared semantic fixtures for every operation the demo exposes.
- Browser memory/load benchmarks are published for the synthetic fixture, a representative subset and any attempted full edition.
- The docs **SDK** section gains a WebAssembly page covering supported browsers, local artefact generation, privacy, data licensing, limitations and demo usage.

## Documentation information architecture

When `R1` starts shipping user-visible APIs, add this top-level Zensical navigation section:

```yaml
- SDK:
    - Overview: sdk/index.md
    - Rust: sdk/rust.md
    - Python: sdk/python.md
    - WebAssembly: sdk/wasm.md
    - Data and licensing: sdk/data-licensing.md
```

Do not publish placeholder pages that imply unavailable packages have shipped. Add Overview + Rust with `R1`, Python with `R2`, and WebAssembly/demo with `R3`; the Overview may show an honest shipped/planned status table throughout.

## Licensing and commercial option

The Rust crate, Python package, WASM/npm package, demo source and examples remain `AGPL-3.0-or-later`. The strategic goal is to let open-source projects embed high-quality local SNOMED tooling without being structurally disadvantaged relative to closed vendors. Bindings must not weaken that reciprocity through a linking exception or permissive wrapper licence.

A future commercial licence may be offered to organisations that want to embed the engine in closed-source products, potentially bundled with paid maintenance or support. That is compatible with continuing to publish the same code under AGPL, provided the commercial licensor holds the necessary copyright/relicensing rights. Before such an offer is made, decide how external contributions are handled (for example, a contributor agreement or limiting the commercial grant to code whose rights are held by Baw Medical). Do not add a CLA or promise commercial terms as part of `R1`-`R3`; record that as a separate governance decision if the proposition becomes real.

SNOMED CT content licensing is independent of the software licence. Every SDK surface must require users to supply content they are entitled to use and must preserve release provenance. Public packages and demos contain only synthetic data.

## Sequencing

1. `R1`: stabilise the Rust SDK facade and migrate existing adapters onto it.
2. `R2`: add Python bindings over the stable facade; this can begin once the first vertical Rust slices are settled.
3. `R3`: make the pure core WASM-clean, select the browser storage/artefact design through a measured spike, then ship the local-only demo.

Python should not wait for full Rust API coverage if lookup/search/ECL/hierarchy contracts are already stable. WASM can proceed in parallel on storage-independent functions, but the browser query artefact must not be selected by assumption.
