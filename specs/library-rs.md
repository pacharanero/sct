# sct as a Rust library

Design document for evolving `sct-rs` from a CLI-only binary into a dual-purpose crate: a CLI tool *and* an importable Rust library for SNOMED CT functionality.

---

## 1. Goals

- Allow other Rust projects to `use sct_rs::SnomedDb` and query SNOMED CT directly.
- Expose a typed, idiomatic Rust API — no JSON-in/JSON-out, no `serde_json::Value` arguments.
- Keep the existing CLI behaviour exactly as-is (the CLI becomes a thin layer over the library).
- Enable `tests/` integration tests now that a library crate exists.
- Lay the ground for `cargo doc` to produce useful API documentation.

---

## 2. Proposed crate structure

```
src/
  lib.rs            — public re-exports; the library's front door
  db.rs             — SnomedDb struct and all query methods
  types.rs          — public data types (ConceptSummary, ConceptDetail, …)
  codelist/
    mod.rs          — CodelistFile, FrontMatter, ConceptLine (already mostly public)
    parse.rs        — parsing logic (extracted from commands/codelist.rs)
    export.rs       — CSV / Markdown / OpenCodelists export
  commands/         — CLI layer; private to the binary, not re-exported from lib.rs
    mcp.rs          — MCP JSON-RPC server (calls SnomedDb internally)
    codelist.rs     — codelist subcommands (calls CodelistFile internally)
    …
  main.rs           — CLI entry point; uses sct_rs:: instead of mod declarations
tests/
  db.rs             — integration tests for SnomedDb
  codelist.rs       — integration tests for CodelistFile parsing and export
```

The `commands/` subtree is **not re-exported** from `lib.rs`. It remains internal to the binary target. Library consumers only see what is explicitly `pub` in `lib.rs`.

---

## 3. Public types

### 3.1 Core query types

```rust
/// Lightweight concept record returned by search, children, ancestors, etc.
pub struct ConceptSummary {
    pub id: String,
    pub preferred_term: String,
    pub fsn: String,
}

/// Full concept detail returned by SnomedDb::concept().
pub struct ConceptDetail {
    pub id: String,
    pub fsn: String,
    pub preferred_term: String,
    pub synonyms: Vec<String>,
    pub hierarchy: String,
    pub hierarchy_path: Vec<String>,
    pub parents: Vec<ConceptSummary>,
    pub children_count: u32,
    pub active: bool,
    pub module: String,
    pub effective_time: String,
    pub ctv3_codes: Vec<String>,
    pub read2_codes: Vec<String>,
}

/// One row in an ancestor chain, with its depth from root.
pub struct AncestorEntry {
    pub concept: ConceptSummary,
    pub depth: usize,
}

/// Cross-map result for a SNOMED CT concept.
pub struct SnomedMapping {
    pub sctid: String,
    pub ctv3_codes: Vec<String>,
    pub read2_codes: Vec<String>,
}

/// Target terminology for reverse mapping (CTV3 or Read v2 → SNOMED).
pub enum Terminology {
    Ctv3,
    Read2,
}
```

### 3.2 Codelist types

`CodelistFile`, `FrontMatter`, `ConceptLine`, `Author`, and `Warning` are already defined in `commands/codelist.rs`. They move to `src/codelist/mod.rs` and are re-exported from `lib.rs` with an added `impl CodelistFile` that exposes the parse and query methods directly on the type (see §4.2).

---

## 4. Public API

### 4.1 SnomedDb

```rust
pub struct SnomedDb { /* private: Connection */ }

impl SnomedDb {
    /// Open a SQLite database produced by `sct sqlite`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self>;

    /// Open an in-memory database (useful in tests and tooling).
    pub fn open_in_memory() -> Result<Self>;

    /// FTS5 full-text search over preferred term, synonyms, and FSN.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<ConceptSummary>>;

    /// Full concept record by SCTID. Returns None if the concept is not found.
    pub fn concept(&self, id: &str) -> Result<Option<ConceptDetail>>;

    /// Immediate children of a concept in the IS-A hierarchy.
    pub fn children(&self, id: &str, limit: usize) -> Result<Vec<ConceptSummary>>;

    /// Full ancestor chain from the concept to root, ordered nearest-first.
    pub fn ancestors(&self, id: &str) -> Result<Vec<AncestorEntry>>;

    /// All concepts in a named top-level hierarchy (e.g. "clinical_finding").
    pub fn hierarchy(&self, name: &str, limit: usize) -> Result<Vec<ConceptSummary>>;

    /// Subsumption test: is `child` an IS-A descendant of `ancestor`?
    pub fn is_a(&self, child: &str, ancestor: &str) -> Result<bool>;

    /// All active descendants of a concept (recursive IS-A closure).
    pub fn descendants(&self, id: &str) -> Result<Vec<String>>;

    /// SNOMED CT concept → CTV3 and Read v2 codes.
    pub fn map_from_snomed(&self, sctid: &str) -> Result<SnomedMapping>;

    /// CTV3 or Read v2 code → SNOMED CT concept(s).
    pub fn map_to_snomed(&self, code: &str, terminology: Terminology) -> Result<Vec<ConceptSummary>>;
}
```

The SQL behind each method already exists and is tested — it lives in `commands/mcp.rs` today. The migration moves that SQL into `db.rs` without changing it.

### 4.2 CodelistFile

```rust
impl CodelistFile {
    /// Parse a `.codelist` file from a string (e.g. from an embedded asset or network fetch).
    pub fn parse(text: &str) -> Result<Self>;

    /// Read and parse a `.codelist` file from disk.
    pub fn read(path: &Path) -> Result<Self>;

    /// Write a `.codelist` file to disk.
    pub fn write(&self, path: &Path) -> Result<()>;

    /// Iterator over active concept lines only.
    pub fn active_concepts(&self) -> impl Iterator<Item = (&str, &str)>;
    //                                                         id   term

    /// Export as a plain `sctid,preferred_term` CSV.
    pub fn export_csv(&self) -> String;

    /// Export in OpenCodelists `code,term` CSV format.
    pub fn export_opencodelists_csv(&self) -> String;

    /// Export as a Markdown table.
    pub fn export_markdown(&self) -> String;
}
```

`parse_body_line` and `split_term_comment` remain private — they are implementation details tested via the public `parse` method in `tests/codelist.rs`.

---

## 5. lib.rs surface

```rust
// src/lib.rs

pub mod codelist;
pub mod db;
pub mod types;

pub use db::SnomedDb;
pub use types::{AncestorEntry, ConceptDetail, ConceptSummary, SnomedMapping, Terminology};
pub use codelist::{CodelistFile, ConceptLine, FrontMatter};
```

`builder`, `rf2`, `schema`, and `commands` are **not** re-exported — they are implementation details. External crates only need `SnomedDb` and the codelist types.

---

## 6. What the CLI layer becomes

Each `commands/*.rs` file shrinks to a thin adapter: parse CLI args, call the library, format for humans. For example, `commands/mcp.rs` becomes:

```rust
// Before: tool_children(conn: &Connection, args: &Value) -> Result<String>
// After:
fn tool_children(db: &SnomedDb, args: &Value) -> Result<String> {
    let id = args["id"].as_str().context("requires id")?;
    let limit = args["limit"].as_u64().unwrap_or(50) as usize;
    let rows = db.children(id, limit)?;
    // ... format rows as JSON for MCP response
}
```

The SQL logic moves to `SnomedDb::children()`; the MCP tool just formats the result. This is a mechanical refactor — the SQL does not change.

---

## 7. Migration plan

The migration is designed to be done incrementally with a working CLI at every step.

**Step 1** — Add `src/types.rs` with the public structs (no functional change).

**Step 2** — Add `src/db.rs` with `SnomedDb::open()` and a first method (`search`). Wire up `src/lib.rs`. Update `main.rs` to use `sct_rs::` imports instead of `mod` declarations.

**Step 3** — Implement remaining `SnomedDb` methods one by one, each backed by the existing SQL.

**Step 4** — Refactor `commands/mcp.rs` to use `SnomedDb` (CLI output unchanged).

**Step 5** — Move `codelist` parsing/export to `src/codelist/`, expose `CodelistFile::parse()` publicly. Refactor `commands/codelist.rs` to use it.

**Step 6** — Move tests to `tests/db.rs` and `tests/codelist.rs`. Remove the existing `#[cfg(test)]` blocks added in the previous session. Keep `#[cfg(test)]` inline only for any genuinely private helpers that can't be reached through the public API.

**Step 7** — Write Rustdoc examples in `lib.rs` and the key types/methods. Run `cargo doc --open` to review.

Each step is a separate commit. The git log should read as a clear narrative of the evolution.

---

## 8. Versioning

The crate is currently `0.3.7`. Suggested milestones:

| Version | Milestone |
|---------|-----------|
| `0.4.0` | `src/lib.rs` exists; `SnomedDb` and core types are public; tests in `tests/` |
| `0.5.0` | `CodelistFile` fully public; all CLI commands refactored to use the library |
| `1.0.0` | Public API is stable; breaking changes would require a major version bump |

A `[lib]` section in `Cargo.toml` is not required — Cargo auto-detects `src/lib.rs`. The binary section already exists:

```toml
[[bin]]
name = "sct"
path = "src/main.rs"
```

No Cargo.toml changes are needed until we want to publish the library separately to crates.io (at which point `crate-type = ["cdylib", "rlib"]` may be relevant for FFI consumers).

---

## 9. Open questions

- **`attributes` field**: currently stored as a raw JSON string in the DB. Should `ConceptDetail::attributes` be `serde_json::Value`, a typed enum, or remain `String` for now? The attribute model is complex (role groups, value types) — `serde_json::Value` is the least-wrong default until a full attribute API is designed.
- **Async?** The SQLite queries are fast and synchronous. No async is needed. If an async consumer wants to use the library they can spawn it on a blocking thread via `tokio::task::spawn_blocking`.
- **`#[non_exhaustive]`**: marking `ConceptDetail`, `ConceptSummary`, and `AncestorEntry` as `#[non_exhaustive]` from the start would let us add fields without a major version bump.
