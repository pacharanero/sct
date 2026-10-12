# Named user-visible guarantees (`R99-inventory`)

Read-only audit for `R99` (see
[`ods-inspired-assurance.md`](ods-inspired-assurance.md#r99---named-guarantees-and-hostile-environment-tests)).
No behaviour change and no new test code: it indexes five guarantees against
the spec passage that states each one and the tests that already enforce it,
and lists the **gaps** where a promise has no test across all the commands or
formats it covers. A human sequences the hostile-environment test-writing from
the gap lists. Citations are `file:line` as of the commit that added this file;
test names are the stable reference.

Judgement labels: **enforced** (a test fails on drift for every promised
surface), **partial** (enforced for some surfaces), **gap** (no enforcing test).

## 1. Read-only queries

Promise ([`AGENTS.md`](../AGENTS.md) Core Invariants): read commands open the
database read-only; only `sqlite`, `tct` and `size` open it read-write.

Enforcement point: `sdk::open_db_readonly` (`src/sdk/mod.rs:842`, flags
`SQLITE_OPEN_READ_ONLY`), reached through `commands::open_db_readonly`
(`src/commands/mod.rs:80`). Direct read-only openers outside it: `src/ecl/mod.rs:72`,
`src/commands/tui.rs:45`, `src/commands/gui.rs:456`.

Tests:
- `readonly_open_does_not_create_missing_file`, `readonly_open_rejects_writes`
  (`src/commands/mod.rs:251-276`) and the SDK twin (`src/sdk/mod.rs:1729`).
- `open_is_read_only_and_missing_paths_are_not_created` (`tests/sdk.rs:69`).

Judgement: **partial**.
- **Gap - the rule list in `AGENTS.md` is already wrong.** `sct read2 import`
  opens the user's database read-write (`src/commands/read2.rs:84`,
  `Connection::open`, via `import_archive`), a fourth writer after `sqlite`,
  `tct` (`src/commands/tct.rs:34`) and `size` (`src/commands/size.rs:425`).
  It is a legitimate importer, but the invariant text and any guarantee test
  must name it, or the invariant should be narrowed to "query commands".
- **Gap - the opener tests prove the helper, not the commands.** Nothing runs
  each query command (`lookup`, `lexical`, `ecl`, `refset`, `map`, `diagram`,
  `history`, `codelist`, `info`, `tui`/`gui` data paths, `serve`, `mcp`) against
  the fixture and asserts the database file is byte-identical afterwards
  (hash before/after, including `-wal`/`-shm` siblings). A new command that
  calls `Connection::open` directly would pass every current test.
- **Gap - `sct serve` write refusal.** `ServeOptions.read_only`
  (`src/commands/serve/mod.rs:83`) is documented "always true"; the only test
  hit is batch entries being GET-only (`serve/mod.rs:1143`). No test sends
  POST/PUT/DELETE to a resource route and asserts refusal plus an unchanged DB.
- **Gap - MCP.** `tests/mcp-protocol.rs:129` checks `read_only_hint`
  annotations, but the codelist tools legitimately write files under the
  codelist root; no test ties "database never written" to the MCP surface.

## 2. Offline operation

Promise ([`README.md`](../README.md), `AGENTS.md`): no network calls at
runtime; everything runs on the user's machine.

Actual network sites (all `ureq`, optional dependency, `Cargo.toml:134`):
- `sct trud` (`src/commands/trud.rs:933,1169,1408,1491,1517`) - explicit
  download tool; network is its purpose.
- `sct codelist import <https URL>` (`src/commands/codelist.rs:930`) and
  codelist `includes:` URL fetch (`codelist.rs:348-363`, cached by URL hash).
- `sct embed` / `sct semantic` post to an Ollama endpoint, default
  `http://localhost:11434` (`src/commands/embed.rs:63`,
  `src/commands/semantic.rs:51`, `embed.rs:267`, `semantic.rs:597`);
  `src/commands/ollama.rs:39` probes the same endpoint.

Tests:
- `codelist_composition_rejects_url_includes_offline` (`tests/sdk.rs:493`):
  the SDK refuses URL includes.
- `tests/trud_api.rs` runs TRUD against a local mock, not the internet.

Judgement: **gap** as a guarantee, although the code looks right by
inspection.
- Nothing proves the *query surface* makes no connection. There is no test that
  runs the fixture suite with outbound networking unavailable (unroutable proxy
  env, or a sandbox with no network namespace) and asserts success.
- The network-capable commands are not enumerated anywhere a test can read, so
  a new `ureq` call outside the three groups above would not be noticed. The
  cheapest drift check is a source-grep test over `ureq::`/`reqwest`/
  `TcpStream` allow-listed to the files above.
- Ollama's `--url`/host argument is user-supplied and can be non-loopback; the
  docs say "local-first" but no test or warning covers a remote embedding
  host (compare the existing `sct serve` non-loopback warning).
- URL codelist import has no test of its failure shape when the network is
  down (stdout empty, error on stderr, nonzero exit).

## 3. Strict unsupported-input handling

Promise ([`roadmap.md`](roadmap.md) `R17`'s governing invariant): unrecognised
or unsupported input - and inaccurate self-description - must never silently
mislead a client. Plus [`output-boundaries.md`](output-boundaries.md) for
data-to-syntax cases.

Tests, by surface:
- ECL: `refinement_attribute_group_is_refused_not_flattened`
  (`tests/ecl_eval.rs:231`), `unsupported_ecl_errors_clearly` (`:272`),
  `grouped_refinement_is_rejected_by_sdk_and_cli` and
  `grouped_refinement_is_a_client_error_over_fhir`
  (`tests/ecl-history-binding.rs:197,314`; `R92`), unsupported filters in
  `src/ecl/lex.rs:372,392`, malformed compress input
  (`tests/ecl-input-boundaries.rs:92`).
- Evidence gates: `simple_build_refuses_explicit_history_requests`,
  `sct_history_command_refuses_a_build_without_association_evidence`,
  `simple_build_refuses_classification_maps_but_keeps_legacy_maps`
  (`tests/history_evidence.rs:55,78,161`; `R93`, `R95`).
- Builds: `all_mode_refuses_to_silently_drop_companion_streams_on_stdout`
  (`tests/end_to_end.rs:586`), `unsupported_filters_and_profiles_are_refused_by_name`
  (`:741`).
- FHIR: the refusal matrix in `tests/fhir_conformance.rs:324-451`,
  `every_implicit_valueset_form_resolves_or_is_refused` (`:729`),
  `expand_property_unknown_name_is_refused` (`tests/serve.rs:2658`),
  `http_expand_refuses_a_body_it_cannot_read_rather_than_expanding_everything`
  (`:1046`), `unsupported_expand_input` (`src/commands/serve/mod.rs:1622`).
- Config: `unrecognised_config_key_disables_the_whole_file_and_says_so`
  (`tests/cli.rs:634`).
- CLI arguments: `unknown_subcommand_is_arg_error`, `ids_conflicts_with_explicit_structured_formats`,
  `bench_semantic_rejects_inapplicable_parent_options` (`tests/cli.rs:130,148,1704`).
- Boundaries: `tests/output-boundaries.rs`, `tests/codelist-boundaries.rs`,
  `tests/line-output-boundaries.rs`, `tests/markdown-rebuild.rs`.

Judgement: **partial** - strong per-feature regression tests, no cross-surface
matrix.
- **Gap - MCP.** The refusal cases above are asserted over CLI, SDK and FHIR;
  `tests/mcp-protocol.rs` has no equivalent assertions that MCP ECL/history/
  classification-map tools refuse rather than degrade. `AGENTS.md` names MCP as
  a caller to trace.
- **Gap - unsupported-argument discovery.** Whether every command rejects an
  unknown `--format` value (rather than ignoring it) is tested only for
  specific commands. A table-driven test over the command tree, reporting the
  offending command, is what `R99` asks for.
- **Gap - GUI/TUI.** `tests/gui-boundaries.js` covers output encoding only;
  unsupported-input behaviour there is untested.

## 4. Provenance identity

Promise ([`cite.md`](cite.md), `src/provenance.rs`): derived artefacts carry the
identity of the canonical artefact they came from; evidence is represented,
not discarded.

Tests:
- NDJSON header round trip: `ndjson_line_roundtrip`, `ndjson_line_rejects_concept_record`
  (`src/provenance.rs:735,760`); content fingerprint verified on SQLite import
  (`tests/cli.rs:432`, `verify_or_set_content_fingerprint`,
  `src/provenance.rs:235`).
- Companions and fingerprints across NDJSON -> sidecar -> SQLite
  (`tests/end_to_end.rs:174-249`, `:881`), SQLite typed read
  (`provenance_and_known_concept_are_typed`, `tests/sdk.rs:94`).
- FST index (`provenance_round_trips`, `tests/fst_index.rs:123`).
- Marker gates: `src/provenance.rs:629-650`.
- Suppression: `bench_no_provenance_withholds_release_identity`
  (`tests/cli.rs:1906`), `tests/embed_semantic.rs:228`; footer neutralisation
  (`human_footer_neutralises_controls_without_changing_json`, `:677`).

Judgement: **partial**.
- **Gap (known defect, queued as `R97`).** Arrow metadata drops `companions`:
  `to_arrow_metadata` (`src/provenance.rs:170`) never writes it and
  `from_arrow_metadata` (`:189`) hardcodes an empty list. No Arrow round-trip
  test exists for any provenance field.
- **Gap - Parquet and Markdown.** No test shows release identity surviving
  `sct parquet` or Markdown output, the other derived artefacts the `AGENTS.md`
  core invariant lists.
- **Gap - `--provenance` coverage.** `ProvenanceFlags` is wired into only
  `history`, `lexical`, `lookup`, `refset`, `search`, `semantic`. Other query
  commands (`ecl`, `codelist`, `map`, `diagram`, `info`) either show nothing or
  do so ad hoc; no test enumerates which commands honour `--provenance` /
  `--no-provenance`, so "every query command" cannot currently be asserted.
- **Gap - default-mode matrix.** `should_show` (`src/provenance.rs:290`) has
  three behaviours (TTY text, piped text, JSON) but no unit test over the
  `ProvenanceFlags` x `OutputMode` table; the TTY branch cannot be hit from
  `assert_cmd` and is untested.
- Serve/MCP: no test that FHIR `meta`/MCP responses report the loaded release
  identity consistently with `sct lookup`.

## 5. stdout / stderr separation

Promise (`AGENTS.md`; `R59`): machine-readable output on stdout, hints,
progress, warnings and "not found" on stderr; exit 0 success / 1 unresolved
single-item lookup / 2 usage error.

Tests:
- `cli_stdout_stderr_exit_code_discipline` (`tests/cli.rs:2005`) - the
  umbrella test, but five cases over `lookup`/`lexical` only.
- Empty-stdout checks on failure/empty paths: `tests/cli.rs:587,605,624,682,
  715,730,743,764,853,1039,1096,1106,1137,1185`
  (e.g. `lexical_empty_search_keeps_stdout_clean`,
  `fst_empty_search_keeps_stdout_clean`,
  `lookup_stdin_batch_is_structured_ordered_and_fail_closed`,
  `missing_tct_guidance_preserves_structured_stdout_and_stops_after_build`).
- `ndjson_stdout_is_a_valid_canonical_artefact` (`tests/cli.rs:418`),
  `lookup_ctv3_structured_output_is_one_valid_document` (`:690`),
  `lexical_stdin_batch_emits_one_structured_document` (`:1111`).
- MCP keeps stdout for JSON-RPC: `malformed_envelopes_notifications_and_eof_have_jsonrpc_semantics`
  (`tests/mcp-protocol.rs:498`), `unread_stdout_applies_end_to_end_backpressure`
  (`:576`).
- `tests/line-output-boundaries.rs` for line-oriented formats.

Judgement: **partial** - strongest of the five, still command-by-command.
- **Gap - structured output with a warning present.** Only a few cases assert
  that `--format json|yaml|tsv` stdout parses as exactly one document while a
  warning is emitted (`:1340`, `:1420`). There is no per-command, per-format
  check that stdout is clean when stderr carries a hint.
- **Gap - commands never run in the umbrella test.** `ecl`, `refset`,
  `codelist`, `map`, `diagram`, `history`, `info`, `size`, `tct`, `trud`, `read2`
  are not exercised for the exit-code table (1 for an unresolved single item,
  2 for usage).
- **Gap - exit code 1 vs 2 by command.** Documented in `AGENTS.md` but asserted
  only for `lookup`; `refset info`/`profile` missing-refset tests
  (`tests/cli.rs:705,720`) check failure, not the specific code.
- **Gap - build-time progress.** Progress bars (indicatif) go to stderr; no test
  asserts `sct ndjson -o -` / `sct sqlite` leave stdout untouched when
  progress is drawn (only `ndjson_stdout_is_a_valid_canonical_artefact`).
- **Gap - non-terminal and narrow-terminal output.** No test varies `COLUMNS`,
  `TERM`, `NO_COLOR` or a missing TTY, which `R99` lists as hostile conditions.

## 6. Isolation harness (what hostile-environment work builds on)

- `s/version++` runs both test suites with `env -u SCT_DB SCT_DATA_HOME="$gate_home"`
  (`s/version++:135-136`).
- About a dozen integration tests set `SCT_DATA_HOME` per child
  (`tests/ecl-history-binding.rs`, `tests/rf2-quoting.rs`, `tests/transcode.rs`,
  `tests/markdown-rebuild.rs`, `tests/codelist-boundaries.rs`, ...), but
  `tests/cli.rs`'s `sct()` helper (`:16`) does not, so the main CLI suite relies
  on the ambient environment (the release gate and CI isolate it from outside).
- No test sets `HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME` or `SCT_DB` to
  misleading values, drops a decoy `snomed.db` in the working directory, or
  checks for unexpected files after a run.

## Suggested sequence for `R99`

Ordered by value over cost, for a human to confirm:
1. Per-child hermetic environment in `tests/cli.rs`'s `sct()` helper, plus a
   decoy `snomed.db` in CWD (proves reads cannot succeed by accident).
2. Table-driven "database unchanged after every query command" test (section 1).
3. Source-grep allow-list for network crates (section 2).
4. Table-driven stdout-clean test across commands x structured formats
   (section 5), reporting the offending command/format.
5. Land `R97`, then add the Arrow/Parquet/Markdown provenance round trips
   (section 4).
6. Fix the `AGENTS.md` writer list (section 1) once the `read2 import` decision
   is made.
