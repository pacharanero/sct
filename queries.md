# Open Questions

Questions that need a decision before implementation can proceed. Please add your answer inline.

---

## Q1 — `sct embed`: local model or Ollama?

**Context:** Milestone 6 (vector embeddings) needs an embedding strategy:

- **Option A — Ollama HTTP client:** Call `http://localhost:11434/api/embed` with `nomic-embed-text`. Simple to implement, no model bundling. Requires Ollama to be running.
- **Option B — `ort` (ONNX Runtime):** Bundle a quantised ONNX model. Fully offline, no external dependency. Larger binary, more complex build.
- **Option C — `candle` (Hugging Face):** Pure-Rust inference. No C dependency. Somewhat experimental for embedding models.

**Preference?**

> *Answer here*

---

## Q2 — Markdown output: one file per concept or one file per hierarchy?

**Context:** 831k concepts → 831k files in `sct markdown`. This works fine with `ripgrep`/`fzf`, but some filesystem MCP servers and RAG pipelines behave badly with very large numbers of small files.

An alternative is one file per top-level hierarchy (~20 files, ~40k concepts each), or chunked files of N concepts.

**Preference?**

> *Answer here* (current default: one file per concept)

---

## Q3 — Release target: `cargo install` only, or pre-built binaries?

**Context:** Pre-built binaries on GitHub Releases make it easier for non-Rust users. The CI workflow can produce Linux x86_64 and macOS arm64/x86_64 binaries via `cross` or matrix builds.

**Preference?**

> *Answer here*

---

## Q4 — `sct mcp` schema_version validation

**Context:** The SQLite database built by `sct sqlite` stores `schema_version` per row. When `sct mcp` opens the DB, should it refuse to start if it detects a schema_version it doesn't understand (forward compatibility guard)?

**Preference?**

> *Answer here* (current behaviour: no check, works with any schema_version)
