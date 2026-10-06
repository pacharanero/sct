# Ods-inspired reproducibility and query assurance

Proposals recorded 2026-10-01 after reviewing Oli Evans's [`ods`](https://github.com/olizilla/ods) at `7f95000`. Tracked by `R98`-`R100` in [the roadmap](roadmap.md). The citation command has its own [specification](cite.md). These extend existing `sct` machinery rather than replacing its canonical NDJSON pipeline.

## R98 - Reproducible identity versus build-event metadata

`ods` separates tool and dataset versions, treats encoding-dependency changes as dataset changes, and compares independently built manifest digests across platforms. `sct` already has canonical content fingerprints and companion identities, but timestamps, input paths and producer versions are also embedded as provenance. Distinguish reproducible content from the event that built it before making stronger reproducibility claims.

Proposed work:

- Document the identities already available: upstream release identity, canonical concept fingerprint, each companion fingerprint, schema/transformation version, and build-event metadata. State which are recorded, inferred or verified.
- Define the fingerprint/version contract for transformation changes, independently of tool releases. Do not add a dataset-version counter without first establishing what existing schema versions and fingerprints fail to express.
- Rebuild identical synthetic RF2 inputs in different temporary paths and at different times; compare concept and companion identities while allowing documented build-event fields to differ.
- Demonstrate failure on changed canonical content or companion content and on a mismatching companion binding. Check all relevant derived-format provenance round trips.
- Evaluate a small cross-platform CI identity check once the local contract is established. Gate canonical-content identity first, not byte-for-byte SQLite or compressed-file equality unless explicitly designed and demonstrated.

Exit when a user can tell exactly what an identifier proves, and tests distinguish changed content from a changed build timestamp/path. OCI packaging, public hosting of licensed terminology, and blanket byte-identical database promises are outside this proposal.

## R99 - Named guarantees and hostile-environment tests

[`ods/docs/tests.md`](https://github.com/olizilla/ods/blob/7f95000/docs/tests.md) organises tests around named user-visible guarantees. Its CI deliberately varies home directory, terminal width, credentials and index availability, and checks for unexpected files. Combine this with our existing spec-derived conformance work and command-tree policy checks.

Proposed work:

- Add a compact guarantee index linking to existing authoritative specs and tests rather than duplicating their requirements. Start with read-only queries, offline operation, strict unsupported-input handling, provenance identity, and stdout/stderr separation.
- Test each guarantee across its promised commands/formats. A test called "all formats" must actually assert the invariant for every advertised format; report the offending command/format, not a coverage count.
- Extend the existing isolated `SCT_DATA_HOME` setup with temporary HOME/XDG/config paths, misleading discoverable files, narrow/non-terminal output, and deliberately unavailable external endpoints where a tested path could otherwise use them.
- Prefer per-child environment configuration and temporary working directories over mutating process-global state. Keep fixtures independent of real licensed local databases.
- Detect unexpected writes to the fixture/repository and prove read commands cannot succeed by accidentally discovering the developer's installed data. Preserve intentional build/cache outputs through an explicit allowance rather than an indiscriminate no-files rule.
- Deliberately violate representative guarantees during test development and observe the expected failure.

Exit when the same synthetic suite behaves consistently under clean and hostile environments, and the new checks demonstrably catch ambient-data dependence or unintended writes.

## R100 - Explain ECL evaluation with checkable semantics

`ods find --sql` gives an executable equivalent query and tests its result set against the native Rust path using DuckDB. The transferable idea is an explanation whose relationship to execution is checked, not a dump of internal debugging strings.

Proposed first surface: `sct ecl expand <expression> --explain`, with text and structured output. The exact surface remains a design decision. Reuse the parsed expression and evaluator planning logic; do not maintain a second ECL compiler solely for explanation.

The explanation should identify the selected artefact, evaluation stages, transitive-closure versus fallback choices, predicates and bound parameters, history/refset evidence requirements, and any unsupported construct. ECL can require several queries and set operations, so do not claim a single generated SQL statement is equivalent when it is not. Keep user values separate from SQL syntax.

Decide whether explanation is plan-only or can optionally include observed execution results, and label those states distinctly. Explain-mode must remain read-only. Use the real synthetic database schema to compare the explained path's result set with ordinary expansion for hierarchy, refinement, set algebra and supported history cases. Missing evidence and unsupported constructs must fail consistently with the normal command.

Exit when a user can understand why an expression selects its results, and changes cannot silently make the explanation disagree with the evaluator. General SQLite tuning diagnostics and a new query language are outside this item.
