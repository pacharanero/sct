# Open Queries

Inconsistencies and open questions noted during the specs reorganisation (2026-03-28).
Address each item and delete it (or move it to the relevant spec) once resolved.

---

## Q1 — `bench/` roadmap vs BENCHMARKS.md

**Context:** `roadmap.md` previously had the Quality item "Build and run `bench/bench.sh` and
populate real timings in `BENCHMARKS.md`" marked `[x]` done. However, `bench/bench.sh` does
not exist — BENCHMARKS.md was populated using direct `time sct ...` measurements, not the
bench script suite.

**Question:** Should the bench script suite (`bench/`) still be built, or is BENCHMARKS.md
now considered sufficient? If the scripts are to be built, the Quality checkbox should remain
`[ ]`. The roadmap has been corrected: the bench/ work remains outstanding.

---

## Q2 — `sct codelist diff` vs `sct diff`

**Context:** Two different `diff` verbs exist in the spec:
- `sct diff` ([`specs/commands/diff.md`](commands/diff.md)) — compares two NDJSON artefacts
  at the *release* level (what changed between SNOMED releases)
- `sct codelist diff` ([`specs/commands/codelist.md`](commands/codelist.md)) — compares two
  `.codelist` files (what changed between two versions of a codelist)

**Assessment:** These are distinct operations with different inputs and different purposes.
Both should exist. No action required unless naming is confusing to users — in which case
consider `sct codelist compare` as an alternative verb.

---

## Q3 — MCP tool names: spec vs implementation

**Context:** [`specs/commands/mcp.md`](commands/mcp.md) lists the MCP tool names as
`snomed_search`, `snomed_concept`, `snomed_children`, `snomed_ancestors`, `snomed_hierarchy`.

**Question:** Do these match the tool names registered in `sct/src/commands/mcp.rs`? The spec
should be the source of truth. Verify and update whichever is wrong.

---

## Q4 — `sct codelist` CLI aliases (`sct refset`, `sct valueset`)

**Context:** [`specs/commands/codelist.md`](commands/codelist.md) specifies that `sct refset`
and `sct valueset` are accepted as aliases for `sct codelist` everywhere.

**Question:** When implementing, how will Clap handle top-level subcommand aliases? Clap
`#[command(alias = "refset")]` works for subcommand aliases. Confirm the aliasing approach
before implementation and update the spec with any constraints (e.g. aliases not appearing in
`--help` output, or tab-completion behaviour).

---

## Q5 — `sct info` release date inference from filename

**Context:** [`specs/commands/info.md`](commands/info.md) says `sct info` infers the release
date from the NDJSON filename.

**Question:** Is there a formal filename convention enforced by `sct ndjson --output`? If the
user renames the file or uses `-o -` (stdout piped to a custom filename), the date inference
will fail. Consider whether release date should be stored in the NDJSON itself (e.g. as a
header comment or first-line metadata record) rather than inferred from the filename.

---

## Q6 — `sct ndjson` locale flag and multi-edition layering

**Context:** The `--locale` flag defaults to `en-GB`. When layering multiple `--rf2` sources
(e.g. International + UK Clinical), it is not specified how conflicts in preferred terms
between the base and extension language reference sets are resolved.

**Question:** Clarify and document the priority rule — does the last `--rf2` source win? Does
the locale filter apply across all supplied RF2 directories? Update
[`specs/commands/ndjson.md`](commands/ndjson.md) once confirmed.

---

## Q7 — `sct serve` and `sct codelist publish --to <url>`

**Context:** [`specs/commands/codelist.md`](commands/codelist.md) mentions that
`sct codelist publish --to <url>` will target "any `sct serve` endpoint (future)".
[`specs/roadmap.md`](roadmap.md) lists `sct serve` as a future item.

**Question:** Is `sct serve` in scope for the same release as `sct codelist`? If not, the
`--to <url>` option in `sct codelist publish` should either be deferred or spec'd to target
only OpenCodelists initially, with `sct serve` support added later.
