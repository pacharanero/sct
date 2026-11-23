# Named user-visible guarantees (`R99-inventory`)

This is the read-only inventory stage of `R99` (see [`ods-inspired-assurance.md`](ods-inspired-assurance.md#r99---named-guarantees-and-hostile-environment-tests)). It links five guarantees to their enforcement points and existing tests, records the exact scope those tests prove, and lists the remaining gaps. It does not treat implementation inspection as test evidence or describe unimplemented behaviour as a tested guarantee.

Judgement labels: **enforced** means tests fail on drift for every surface in the stated scope; **partial** means only named surfaces are enforced; **gap** means no enforcing test exists.

## 1. Read-only database queries

Promise ([`AGENTS.md`](../AGENTS.md), Core Invariants): query commands open SQLite read-only. The approved direct read-write commands are `sqlite`, `tct`, `size` after its TCT-build prompt or explicit `--build-tct`, and `read2 import`. `bench --pipeline` and `trud download --pipeline` orchestrate approved builders to create new derived databases.

The shared enforcement point is `sdk::open_db_readonly`, reached by `commands::open_db_readonly`; it uses `SQLITE_OPEN_READ_ONLY` and `PRAGMA query_only`. The TUI and GUI use equivalent explicit read-only flags. The read-write commands use separate, visible openers.

Existing tests:

- `commands::tests::readonly_open_does_not_create_missing_file` and `commands::tests::readonly_open_rejects_writes` prove the shared command helper neither creates a missing file nor permits a write.
- `sdk::tests::readonly_open_does_not_create_missing_file` proves the SDK helper does not create a missing path.
- `open_is_read_only_and_missing_paths_are_not_created` proves that opening a missing path through the public SDK fails without creating it; it does not independently test write rejection.

Current command classification, derived from the `Command` enum in `src/main.rs`:

| Class | Commands and subcommands |
|---|---|
| SQLite readers | `ecl`, `diagram`, `mcp`, DB-backed `info`, DB-backed `codelist`, `refset`, `map`, performance `bench` without `--pipeline`, `lookup`, `history`, `proximal-primitives`, `lexical`, `search lexical`, the read phase of `size`, `tui`, `gui`, `serve` |
| Approved direct SQLite writers | `sqlite`, `tct`, `size` after its prompt or with `--build-tct`, `read2 import` |
| Build orchestrators that invoke approved writers | `bench --pipeline` creates a temporary database; `trud download --pipeline` creates a derived database and `--pipeline-full` also builds its TCT |
| No SQLite access in these modes | `ndjson`, `parquet`, `markdown`, `fst`, `sayt`, `embed`, `diff`, `trud` without a pipeline, `paths`, `semantic`, `search semantic`, `search fuzzy`, `bench semantic`, `completions`, `dmwb` |

Some commands in the reader class legitimately write other artefacts: codelist mutations write codelist files under their configured boundary, and MCP can expose those codelist mutations. That does not permit them to mutate the terminology database.

Judgement: **partial**.

- **Gap - command-level database immutability.** The opener tests prove the helper, not that every reader reaches it. A table-driven integration test should run every SQLite-reader path against the synthetic fixture and compare the database plus any `-wal`/`-shm` siblings before and after. This catches a future direct `Connection::open` that bypasses the helper.
- **Gap - registry drift.** The classification above is prose derived from `src/main.rs`; no test forces a new command to declare its database access policy. Add a registry or equivalent exhaustive match only when it can be shared with dispatch rather than creating a second hand-maintained list.
- **HTTP method refusal (`R106`).** `tests/serve.rs::http_resource_routes_refuse_mutation_without_altering_terminology` runs the real `sct serve` binary over the synthetic fixture with a codelist directory. It sends `PUT`, `PATCH`, `DELETE` and resource-level `POST` (FHIR create) to `/CodeSystem`, `/CodeSystem/sct`, `/CodeSystem?url=...`, `/ValueSet`, `/ValueSet/diabetes` and `/ValueSet?url=...` and requires `405` with an `Allow` header containing exactly `GET` and `HEAD`. CodeSystem and ValueSet writes use matching resource types. It sends the same four methods as `batch` and `transaction` Bundle entries for `CodeSystem/sct`, `ValueSet/diabetes` and `ValueSet`, both without payloads (method refusal) and with resource payloads (payload refusal), and requires a per-entry `400` `OperationOutcome`. `$lookup` of 22298006 by `GET` and by standard `POST` `Parameters` is the positive control. After the server process is stopped, the database file and every codelist file must be byte-identical. `http_non_wal_resource_mutation_refusal_creates_no_sidecars` repeats the matrix after preparing the fixture in DELETE journal mode and permits no new sibling files. Production journal defaults are unchanged. Mutation attempts against operation routes (`$lookup`, `$validate-code`, `$subsumes`, `$expand`, `$translate`), `/metadata`, `/autocomplete` and a non-root `--fhir-base` are outside this matrix; only `$lookup` has GET/POST positive controls here.
- **Gap - read-only WAL sidecars (`R106a`).** The builder produces a WAL-mode database. Read-only SQLite opens can create persistent `-wal`/`-shm` siblings; this was observed for `sct serve` and a diagnostic `sct lookup` run, not exhaustively established for every reader. The WAL-mode HTTP test allows only these two new siblings and requires an empty final WAL. Together with the byte-identical database comparison this proves the checked final state, not the absence of transient writes. It does not meet the original no-new-sidecars criterion for default builds. The DELETE-mode test meets that criterion for its explicitly prepared fixture only. Changing the builder's final journal mode or adopting immutable readers requires a separate human decision about concurrent writers, live WAL visibility and read-only-directory behaviour; neither policy is changed here. Residual work remains in the roadmap rather than being treated as completed.
- **Gap - MCP database immutability.** MCP advertises read-only hints for terminology tools and separately permits bounded codelist-file mutations, but no end-to-end test proves that neither category mutates SQLite.

## 2. Offline native queries

Promise ([`README.md`](../README.md), [`AGENTS.md`](../AGENTS.md), [`spec.md`](spec.md)): native SDK and SQLite/FST-backed query paths have no external network dependency. Non-semantic MCP terminology tools share those local paths. This does not mean the entire binary is forbidden from opening sockets.

Explicit network-capable surfaces:

- `sct trud` accesses the NHS TRUD API to list and download releases.
- `sct codelist import <https URL>` and codelist URL includes fetch user-supplied resources and may populate the URL cache.
- `sct embed`, `sct semantic`, `sct search semantic`, and `sct bench semantic` call a configured Ollama HTTP endpoint, which defaults to loopback but may be remote.
- `sct serve` and `sct gui` listen locally; `sct serve` also owns the HTTP autocomplete adapter. Listeners are adapters over local data rather than outbound query dependencies.
- `sct mcp --embeddings` registers `snomed_semantic_search`, which calls the configured Ollama endpoint; MCP without that explicit option exposes only local terminology and codelist tools.
- The current experimental GUI automatically fetches CSS and JavaScript from public CDNs. This is a known exception to the intended offline GUI contract and is tracked by `R38`/`GUI-2`, not an enforced guarantee today.

Existing tests:

- `codelist_composition_rejects_url_includes_offline` proves SDK composition refuses URL includes rather than performing implicit I/O.
- `tests/trud_api.rs` exercises the deliberate TRUD client against a local mock and proves its success and failure contracts without Internet access.
- SDK tests query supplied SQLite/FST artefacts directly and contain no HTTP configuration surface.

Judgement: **partial**.

- **Gap - native-query no-egress test.** No integration test runs the SQLite/FST query matrix or non-semantic MCP tools in a network-denied sandbox or against a tripwire proxy and proves success without outbound attempts. Loopback listeners must be distinguished from outbound Internet access.
- **Gap - network-client inventory drift.** No test constrains HTTP client construction to the explicit surfaces above. A source-policy test may be useful, but it must account for listener `TcpStream` use and dependencies rather than blindly grep for socket words.
- **Gap - explicit client failure shape.** URL codelist import lacks a network-unavailable CLI test asserting empty stdout, diagnostic stderr, and nonzero exit. Ollama-backed commands need the equivalent unavailable-endpoint coverage.
- **Gap - remote Ollama disclosure.** The endpoint is user-configurable and may be non-loopback; current help does not distinguish that from the default local Ollama topology.
- **Gap - experimental GUI egress.** The CDN dependency must be removed and verified before GUI queries can join the offline guarantee.

## 3. Strict unsupported-input handling

Promise ([`roadmap.md`](roadmap.md), `R17`; [`output-boundaries.md`](output-boundaries.md)): unrecognised or unsupported input must be refused rather than silently broadened, ignored, or represented inaccurately.

Existing tests by real surface:

- ECL: `refinement_attribute_group_is_refused_not_flattened`, `unsupported_ecl_errors_clearly`, `grouped_refinement_is_rejected_by_sdk_and_cli`, `grouped_refinement_is_a_client_error_over_fhir`, lexer refusal tests for unsupported filters, and `compression_rejects_malformed_input_on_stdin_and_in_arguments`.
- Evidence gates: `simple_build_refuses_explicit_history_requests`, `sct_history_command_refuses_a_build_without_association_evidence`, and `simple_build_refuses_classification_maps_but_keeps_legacy_maps`.
- Builds: `all_mode_refuses_to_silently_drop_companion_streams_on_stdout` and `unsupported_filters_and_profiles_are_refused_by_name`.
- FHIR: the parameter-disposition tests in `tests/fhir_conformance.rs`, `every_implicit_valueset_form_resolves_or_is_refused`, `expand_property_unknown_name_is_refused`, `http_expand_refuses_a_body_it_cannot_read_rather_than_expanding_everything`, and `commands::serve::tests::expand_refuses_inputs_it_would_otherwise_silently_ignore`.
- Config and CLI: `unrecognised_config_key_disables_the_whole_file_and_says_so`, `unknown_subcommand_is_arg_error`, `ids_conflicts_with_explicit_structured_formats`, and `bench_semantic_rejects_inapplicable_parent_options`.
- Data/output boundaries: `output_boundaries_markdown_rejects_unsafe_ids_and_preserves_files`, `imports_reject_hostile_terms_without_creating_or_overwriting_destination`, `text_records_keep_delimiters_out_of_values_but_json_preserves_them`, and `nonempty_output_is_rejected_before_opening_input` are representative enforcing tests. Their source files contain additional boundary-specific cases, but a file reference alone is not evidence for an unrelated guarantee.

MCP exposes lookup/search/hierarchy/refset/map and codelist tools, not generic ECL or history tools. Its typed-contract and schema-conformance tests prove request/response shape; map evidence-refusal is covered by `map_no_mappings_names_the_rebuild_reason_for_unloaded_classification_maps`. Any future parity matrix must enumerate actual registered tools rather than assume every CLI command has an MCP twin.

Judgement: **partial**.

- **Gap - advertised argument matrix.** There is no table-driven test over every command's actual Clap surface proving that unknown arguments and invalid values, including each command's advertised format values, fail with a usage error. Not every command supports the shared text/JSON/YAML `OutputFormat`, and command-specific CSV/TSV/IDs modes must be tested only where advertised.
- **Gap - cross-surface semantic parity.** Shared capabilities need representative refusal tests through each adapter that actually exposes them: SDK, CLI, FHIR, MCP, Python, GUI, or TUI. This is capability-driven, not a Cartesian product over nonexistent operations.
- **Gap - GUI/TUI controls.** Existing GUI boundary tests cover output encoding; unsupported user-input behaviour is not yet inventoried against the planned GUI/TUI controls.

## 4. Provenance and artefact identity

Intent ([`cite.md`](cite.md), [`ods-inspired-assurance.md`](ods-inspired-assurance.md), `R98`): provenance should be represented and verifiable where an artefact format claims to carry it. The current implementation does not yet promise that every derived presentation carries the complete canonical identity.

### Carriage

| Format | Current carriage | Existing evidence | Judgement |
|---|---|---|---|
| NDJSON | Header carries `Provenance`, including source paths, release identity, content fingerprint, and companion evidence | `ndjson_line_roundtrip` serialises a fully populated fixture but asserts only edition label, release date, and companions; `ndjson_line_rejects_concept_record` proves a concept row is not accepted as a header | partial |
| SQLite | Provenance metadata and companion markers are imported from NDJSON; content fingerprint is checked before import | Companion rollback/load tests in `tests/end_to_end.rs` and `provenance_and_known_concept_are_typed` cover companion handling and imported core provenance fields; `tampered_canonical_ndjson_cannot_replace_existing_database` and `tampered_canonical_ndjson_does_not_publish_new_database` prove a concept-record edit under an unrefreshed declared fingerprint is refused, leaves an existing destination's data, metadata and companion rows unchanged, and leaves no file at a new destination | partial |
| FST | Serialises and restores the full current `Provenance` value | `full_provenance_round_trips_through_the_container` builds and reopens indexes with stored labels enabled and disabled, from a header with every current field populated (including the canonical content fingerprint and both payload/history companion descriptors), and asserts structural equality. Its legacy-header case (no fingerprint or companions) preserves the header and adds only the fingerprint calculated from the input records, independently of the FST output | enforced |
| Arrow | Schema metadata carries the provenance payload fields, including JSON-encoded source paths and companion descriptors; `_type` is reconstructed as the canonical NDJSON tag (`R97`) | Companion metadata-map checks, descriptor-presence checks on a real IPC file, and an `info` release-date check. See [Arrow evidence](#arrow-evidence); full-field carriage remains `R110` | partial |
| Parquet | The exporter consumes concept records and does not embed the NDJSON header | No carriage implementation; this is an `R98` design/implementation decision, not a missing round-trip test | gap |
| Markdown | The exporter consumes concept records and writes per-concept presentation files without the NDJSON header | No carriage implementation; this is an `R98` design/implementation decision, not a missing round-trip test | gap |

#### Arrow evidence

`provenance::to_arrow_metadata` writes the payload fields as `sct.*` strings, with `source_paths` and `companions` JSON-encoded. An absent fingerprint is omitted. `from_arrow_metadata` restores the fields and reconstructs `_type`; a missing or malformed `sct.companions` reads as an empty list. This records the existing compatibility policy, not a new validation rule.

- **Metadata map:** `arrow_metadata_round_trips_companions` asserts equality of one `history` descriptor, not the full struct. `arrow_metadata_without_companions_key_reads_empty` and `arrow_metadata_with_malformed_companions_reads_empty` pin the compatibility defaults.
- **IPC file:** `tests/embed_semantic.rs::embed_then_semantic_surfaces_myocardial_infarction` runs `sct embed` against a mocked Ollama endpoint and reopens its output with Arrow's `FileReader`. It asserts the presence of both `history` and `payload_refsets` descriptors, not their field values or equality with the NDJSON header.
- **Consumers:** the same test checks that `sct info`'s separate reader shows the fixture release date. It does not assert provenance from `sct semantic` or `sct bench semantic`; the benchmark checks cover `--no-provenance` suppression.

These tests do not assert fingerprint carriage, composite source paths, escaped/Unicode strings or a legacy IPC file without fingerprint/companion keys. Those checks remain in `R110`.

### Verification

SQLite import verifies or records the canonical content fingerprint; regression tests reject a deliberately mismatched canonical fingerprint for both an existing and a new destination, and companion sidecars are checked before publication. An NDJSON artefact without a header (or without a declared fingerprint) is still accepted and fingerprinted on import, so this check detects tampering only where the artefact declares its identity. FST carries full provenance, and a round-trip test asserts full structured equality for a fully populated header and compatibility with a legacy header. Arrow carries companion descriptors and the other provenance fields, but its evidence is a companion-only metadata-map round trip plus presence checks on a real embeddings file; full structured equality through an IPC file is `R110`. Surviving descriptors are not validation of companion files: an Arrow file carries no companion streams. There is no cross-format identity manifest or rebuild-equivalence contract yet; that is the explicit scope of `R98`.

Fresh SQLite builds reserve their destination with exclusive creation before opening it. Failed loads remove the newly reserved database and journal files after the connection closes; an existing destination is never owned by this cleanup guard. A new destination with pre-existing `-wal`, `-shm` or `-journal` files is refused without deleting those files. `malformed_fresh_build_cleans_up_but_preserves_orphan_sidecars` covers that refusal and malformed-input cleanup; guard unit tests cover reservation errors, existing files, dangling symlinks and successful disarming. Cleanup failures produce stderr warnings rather than being silently ignored. These checks do not establish safety against another process concurrently replacing files in the output directory.

### Presentation

`bench_no_provenance_withholds_release_identity` proves one suppression path, while `human_footer_neutralises_controls_without_changing_json` proves footer escaping does not alter structured data. `ProvenanceFlags` is wired into a subset of query commands, and `should_show` has TTY text, redirected text, and structured-output branches without a complete unit matrix.

Judgement: **partial**.

- **Gap - `R110` full Arrow carriage evidence.** Companion carriage shipped in `R97`; still missing is a fully populated `Provenance` round trip with structural equality through both the metadata map and a written-and-reopened IPC file, plus a legacy file without fingerprint or companion keys.
- **Gap - `R98` format contract.** Decide whether Parquet and Markdown should embed canonical identity, carry a sidecar manifest, or remain explicitly provenance-free presentations before writing tests for behaviour that does not exist.
- **Gap - presentation registry.** Enumerate which commands promise `--provenance` and `--no-provenance`, then test only those commands and modes.
- **Gap - adapter identity.** FHIR `meta` and MCP responses are not tested for release-identity agreement with CLI/SDK provenance, and their intended claims need defining before enforcement.

## 5. stdout, stderr, and exit status

Promise (`AGENTS.md`; `R59`): machine-readable data goes to stdout; hints, progress, warnings, and not-found messages go to stderr. Command-specific exit contracts distinguish success, unresolved input where documented, and Clap usage errors.

Existing tests:

- `cli_stdout_stderr_exit_code_discipline` is the umbrella regression test for five lookup/lexical cases.
- `refset_info_missing_refset_fails` and `refset_profile_missing_refset_fails` both explicitly assert exit code 1 and empty stdout.
- `ndjson_stdout_is_a_valid_canonical_artefact`, `lookup_ctv3_structured_output_is_one_valid_document`, and `lexical_stdin_batch_emits_one_structured_document` prove complete structured documents for their named paths.
- `malformed_envelopes_notifications_and_eof_have_jsonrpc_semantics` and `unread_stdout_applies_end_to_end_backpressure` protect MCP's stdout JSON-RPC transport.
- `text_records_keep_delimiters_out_of_values_but_json_preserves_them` and the other named tests in `tests/line-output-boundaries.rs` protect line-oriented output boundaries.

Judgement: **partial**.

- **Gap - per-command output contract.** `ecl`, `codelist`, `map`, `diagram`, `history`, `info`, build commands, and acquisition/import commands are not represented in one exhaustive output/exit registry. A useful matrix must use each command's advertised formats rather than assume a global JSON/YAML/TSV set.
- **Gap - warnings plus structured data.** Only selected paths prove that a stderr warning leaves stdout as exactly one parseable document.
- **Gap - progress output.** No targeted test proves that progress from stdout-producing build/export paths stays entirely on stderr.
- **Gap - terminal conditions.** No matrix varies terminal presence, width, `TERM`, and `NO_COLOR` while checking parseable stdout and bounded presentation.

## 6. Isolation harness

- `s/version++` runs both test suites with `SCT_DB` unset and an isolated `SCT_DATA_HOME`.
- CI does not set a repository-wide isolated data home; it relies on a clean hosted runner. That is useful operationally but is not an enforced hostile-environment test.
- Multiple integration tests already isolate child processes. For example, `tests/empty-valuesets.rs` sets temporary `HOME`, `XDG_CONFIG_HOME`, and `SCT_DATA_HOME`, and `tests/codelist-boundaries.rs` uses equivalent temporary homes. Other suites set only `SCT_DATA_HOME`.
- The common `tests/cli.rs` `sct()` helper does not scrub discovery variables globally. No test deliberately provides conflicting `SCT_DB`, HOME/XDG config, data-home artefacts, and a decoy database in the working directory, then verifies the documented precedence and checks for unexpected writes.

## Suggested sequence for the rest of `R99`

1. Give the common CLI child helper a hermetic baseline, then add explicit precedence cases with conflicting environment/config/CWD artefacts rather than relying on the developer or hosted runner environment.
2. Derive a command database-access policy from dispatch and test that every SQLite reader leaves the database and side files unchanged.
3. Add a capability-driven unsupported-input/output matrix over real adapter surfaces and each command's advertised formats.
4. Add a no-egress test for native SQLite/FST/SDK query paths and a maintainable inventory check for explicit network clients.
5. Land `R109` (full NDJSON header round trip) and `R110` (full Arrow carriage through an IPC file); keep Parquet/Markdown identity decisions under `R98` until their carriage contract is specified.
6. Add terminal-width/non-TTY/progress tests and unexpected-filesystem-write checks around representative command classes.
