# Rust SDK

Add `sct-rs` without the CLI's default server and TUI features:

```toml
[dependencies]
sct-rs = { version = "0.19", default-features = false }
```

Create a database from terminology content you are entitled to use:

```sh
sct ndjson --rf2 /path/to/RF2-Snapshot.zip
sct sqlite --ndjson release.ndjson --output snomed.db
```

Open and query it synchronously:

```rust
use sct_rs::sdk::{SearchOptions, Snomed, Terminology};

# fn example() -> Result<(), Box<dyn std::error::Error>> {
let snomed = Snomed::open("snomed.db")?;

if let Some(provenance) = snomed.provenance() {
    println!("{} {}", provenance.edition_label, provenance.release_date);
}

if let Some(concept) = snomed.concept("22298006")? {
    assert_eq!(concept.preferred_term, "Myocardial infarction");
}

let hits = snomed.search("heart attack", 20)?;
let clinical_hits = snomed.search_with(
    SearchOptions::new("diabetes", 20).hierarchy("Clinical finding"),
)?;

let children = snomed.children("73211009", 20)?;
let ancestors = snomed.ancestors("46635009")?;
let descendants = snomed.descendants("73211009", 100)?;
let relationship = snomed.subsumes("73211009", "46635009")?;
let expanded_ids = snomed.expand("<<73211009")?;

if !snomed.has_transitive_closure() {
    eprintln!("Build the TCT for faster transitive hierarchy queries");
}

let refsets = snomed.refsets()?;
let mappings = snomed.map(
    Terminology::Snomed,
    "22298006",
    Terminology::Icd10,
)?;
let history = snomed.history("9468002")?;
# Ok(())
# }
```

Search accepts the same optional FTS5 operators as `sct lexical`. For untrusted or natural-language input that must always be treated as one literal phrase, use `SearchOptions::new(query, limit).literal()`.

Hierarchy methods use the precomputed TCT when its completion marker, schema, indexes, and source/closure invalidation triggers are valid and otherwise return the same results through recursive CTEs. The SDK never prints policy messages; applications can inspect the live `has_transitive_closure()` value and decide how to surface performance guidance. Use `transitive_closure_usable()` when a database-probe error must be distinguished from an unusable TCT.

The repository also contains a runnable example:

```sh
cargo run --example sdk-basics --no-default-features -- /path/to/snomed.db [/path/to/snomed.fst]
```

`tests/downstream-sdk/` is an isolated consumer crate used to prove that the documented lookup, search, ECL, hierarchy, subsumption, and mapping API compiles without default features.

## FST autocomplete

Attach an FST built from the same canonical NDJSON release. New artefacts carry a deterministic concept-record fingerprint, so the SDK rejects indexes built with a different locale, inactive-content choice, refset mode, extension composition, or concept content even when the release identifier is the same. Older artefacts without fingerprints retain release-identifier validation for compatibility.

```rust
# use sct_rs::sdk::Snomed;
# fn example() -> Result<(), Box<dyn std::error::Error>> {
let snomed = Snomed::open("snomed.db")?.with_fst("snomed.fst")?;
let exact = snomed.fst_exact("heart attack")?;
let prefix = snomed.fst_prefix("myoc", 20)?;
let fuzzy = snomed.fst_fuzzy("myocardial infarcton", 1, 20)?;
let words = snomed.fst_words(&["heart", "attack"], 20)?;
let typeahead = snomed.autocomplete("myoc", 20, true)?;
# Ok(())
# }
```

## Codelists

Codelist values are independent of a database where terminology validation is not required.

```rust
# use sct_rs::sdk::{parse_codelist, render_codelist};
# fn example(text: &str) -> Result<(), Box<dyn std::error::Error>> {
let codelist = parse_codelist(text)?;
let rendered = render_codelist(&codelist)?;
# Ok(())
# }
```

## Errors and missing concepts

SDK functions return `SctError`. A missing concept is `Ok(None)`; database access, unsupported terminology/schema versions, malformed stored JSON, mismatched FST provenance, and codelist failures are typed errors. SDK methods never print messages or hints, leaving presentation and logging policy to the application.

`schema_compatibility()` reports current, older, or unknown database schemas. Opening fails for a newer schema because the crate cannot safely infer forward compatibility.

## Concurrency

`Snomed` owns one synchronous SQLite connection opened with the same read-only profile as the CLI. Use one instance per worker or pool independent instances in concurrent servers; do not share one query session concurrently. The existing `sct serve` connection pool remains the model for server workloads.

## Pre-1.0 compatibility

The SDK is pre-1.0. Public breaking changes require an intentional minor-version bump and a changelog entry. Result structs and errors are non-exhaustive so fields and variants can grow without forcing downstream exhaustive matches. Deprecations will be preferred where a practical migration path exists, but the project does not promise 1.0-level stability before the facade and language bindings have been exercised by real consumers.
