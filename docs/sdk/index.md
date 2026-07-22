# SDK

The Rust SDK embeds the same local terminology engine used by the `sct` CLI. It opens an `sct sqlite` database directly, performs no network calls, and returns typed serialisable records without writing to stdout or stderr.

| Surface | Status |
| --- | --- |
| [Rust](rust.md) | Read-only query facade available across terminology, refset, mapping, codelist, and FST operations |
| Python | Planned (`R2`) |
| WebAssembly | Planned (`R3`) |

The API is being extracted one vertical slice at a time so the CLI, MCP server, FHIR server, and future bindings share one implementation. See [Data and licensing](data-licensing.md) before distributing an application or terminology artefact.

## Current scope

- Open an `sct sqlite` database read-only.
- Inspect release provenance.
- Look up concepts as typed `Concept` records.
- Search preferred terms, FSNs, and synonyms through FTS5.
- Traverse direct children, ancestors, and descendants.
- Test subsumption and expand ECL expressions.
- List, inspect, compare, and profile refsets.
- Map between SNOMED CT, CTV3, Read v2, ICD-10, and OPCS-4, including history forwarding.
- Inspect typed concept history associations.
- Attach a release-matched FST index for exact, prefix, fuzzy, word, and typeahead search.
- Parse, render, read, write, and compose typed `.codelist` files without a database. SDK composition resolves local id/path includes only and rejects URL includes so it never performs implicit network or cache writes.

The default `cli` feature contains command, build, export, network, and presentation dependencies. Using `default-features = false` keeps the synchronous native SDK while excluding Clap, Arrow/Parquet, ZIP, HTTP, progress, and configuration dependencies, giving Python and other native bindings a deliberately smaller base.
