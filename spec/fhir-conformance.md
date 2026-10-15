# FHIR and ECL conformance evidence

Status: Active assurance programme. Roadmap programme: `R17`; `R17a` and `R17e` are shipped, with `R17f` remaining.

## Decision summary

- `sct serve` is a local, read-only, SNOMED CT-focused subset of the FHIR R4 terminology service. It is not currently an approved HL7 Terminology Ecosystem server or a drop-in general terminology server.
- Four different evidence layers are kept separate: repository-owned semantic regressions, independent FHIR resource validation, the official HL7 terminology test runner, and direct ECL syntax/semantic evidence. Passing one layer must not be described as passing another.
- The public CI path uses only the committed synthetic RF2 fixture and licence-clean test material. Runs that require the SNOMED CT test subontology or a real release remain private and use content already available to the licensed operator.
- The immediate target is credible evidence for the shipped SNOMED-focused surface. Full HL7 ecosystem approval is a separate product decision. An unreleased upstream 1.9.5-SNAPSHOT approval-page draft says every approved server claims `general`, whose tests assume transient arbitrary `CodeSystem`, `ValueSet`, and `ConceptMap` ingestion; that proposal is useful planning evidence, not a published approval standard.

## Evidence layers

| Layer | What it proves | Current position |
|---|---|---|
| Repository regression suite | The documented `sct` GET/POST operation contract and synthetic-fixture semantics remain stable | Gating in CI through Rust tests and `benchmarks/conformance-ci.sh` |
| HL7 FHIR Validator | Captured responses satisfy the base R4 resource structures and invariants | Gating in CI through `benchmarks/fhir-validator-ci.sh` |
| HL7 Terminology Ecosystem `txTests` | The server interoperates with the official terminology operation requests and expected responses for the selected released modes | Pre-`R17e` baseline established; licensed test content, unsupported generic terminology semantics, and a post-`R17e` rerun remain |
| ECL 2.3 grammar/examples plus semantic fixtures | The documented ECL subset has explicit syntax coverage and returns exact sets on known terminology content | Official positive-example inventory established; a pinned support-profile gate remains to build |

The repository-owned suite is valuable but not independent: its requests, implementation, and expectations evolve together. The FHIR Validator is independent but checks structure, not whether an expansion contains the right concepts. `txTests` checks terminology interoperability and semantics, but a passing run is still not automatic certification. The published test registry says the FHIR Product Director reviews test outcomes before approving a server. A separate unreleased 1.9.5-SNAPSHOT draft proposes additional approval criteria: a credential-free reproducible endpoint and `general` mode for every approved server. Those draft criteria are recorded for planning but are not represented here as published requirements.

## Pinned external inputs

The baseline was established on 2026-09-25. CI must never use the mutable `current` test coordinate.

| Artefact | Pin | SHA-256 or commit |
|---|---|---|
| FHIR Validator CLI | `6.10.4` | `1106b9d58f9e363e47bea7c4fc065841e5fc91fe9d062775c3bfdd212bd653cc` |
| FHIR R4 core package | `hl7.fhir.r4.core#4.0.1` | `ebd7731df7d36b5b7d39d5fb6c9d77b44bb7fe5742f1a2e87f164738c3289d44` |
| FHIR cross-version extensions package | `hl7.fhir.xver-extensions#0.1.0` | `f3bb9fa2083402e88a02b41f433655274e8a1cca563211c8f7ba6fd0badf537a` |
| R4 terminology package | `hl7.terminology.r4#6.2.0` | `79404c9cc95491fc0155627cd039c401a6eb4748175328131e91b709a41300e2` |
| R4 extensions package | `hl7.fhir.uv.extensions.r4#5.2.0` | `b406e75575f05676559d0759770c5939d023ee72fb2ef38e0b3259328487720a` |
| Terminology package | `hl7.terminology#7.4.0` | `b321ad4ee1c18798abd8692f45bc86550cffac72ae97413125cfb82f0112c72d` |
| R5 extensions dependency | `hl7.fhir.uv.extensions.r5#5.2.0` | `e02fc6eff0f37c1611a35aa93ef9aaa3b55ab7371a5c96f4d2a5834106a13170` |
| R5 terminology dependency | `hl7.terminology.r5#7.1.0` | `473303108b5607aad7b910581739e8bbf3e61a625ed9e739c66e0ca597d4aefe` |
| Extensions package | `hl7.fhir.uv.extensions#5.3.0` | `6e3a3f9929e05d2b813a3f98fa1ad2f5122fb6b2819fa73e6c5520002fb2c5b0` |
| FHIR Terminology Ecosystem package | `hl7.fhir.uv.tx-ecosystem#1.9.3` | `96cf8c08be93600c8824767474d73773729d1d60c016b76af5dcc915f52d9c42` |
| Released `tests/test-cases.json` | package `1.9.3` | `74b6f00dc437d03012e4363950118ca9cff41a785d7f002272e4198aa935c887` |
| Draft approved-server criteria | upstream `1.9.5-SNAPSHOT` source | commit `f5dd4e257c5d84c5b5ea032cf153ce09d3255cc3` |
| ECL formal language release | tag `2.3` | commit `b0e07105ae395821bcc953f3d6084b57dc7bef2c` |
| ECL specification release | tag `2026-03-31` | commit `ba425091400838490a41e7477a74a2d5a43fbffd` |

The ECL formal-language artefacts are pinned separately because each proves a different boundary:

| Artefact | SHA-256 |
|---|---|
| `syntax/abnf-brief.txt` | `7482e079e10d95aac94e139a9a8d5708e51165f282ef5e5fdf6b91c55c6b5df6` |
| `syntax/abnf-ecl-core-brief.txt` | `75be01de2d63a33355cdb03486136d93a43945270038b1036952802e05104678` |
| `syntax/ECL.g4` | `461c27dfd7ffe8b642b2b6938698f384179613323bcea356773c6f03a2898f54` |

Validator 6.10.4 loads both version-specific R4 packages and the unversioned terminology and extensions package families. Those unversioned families bring the listed R5 dependencies even during an R4 validation run. The structural gate seeds exactly these eight archives into an isolated cache, verifies package identity after extraction, and disables HTTP access while the Validator runs. This prevents its latest-local fallback from drifting to a different package graph. The generated terminology-ecosystem IG banner has at times named `1.9.4` while the package registry and published history still exposed `1.9.3`; every official-suite evidence report must likewise record both the requested coordinate and the fetched package hash.

## Baseline findings

### Base R4 structure

FHIR Validator 6.10.4 initially found three errors across eight representative responses from the committed synthetic fixture:

1. `CapabilityStatement.date` was absent.
2. `Bundle.entry.fullUrl` was absent from the `CodeSystem` search result.
3. `ValueSet.expansion.timestamp` was absent.

Those three builders are now corrected and covered through their live HTTP routes. The same response classes validate without base-R4 errors. Remaining Validator warnings are retained as evidence and triaged separately; warnings such as absent narratives, a missing search self-link, and expansion audit recommendations do not become hard failures merely by being emitted.

`R17a` makes this repeatable in CI through `benchmarks/fhir-validator-ci.sh`: it builds the committed fixture, starts `sct serve`, captures the declared response matrix, runs the pinned Validator with the checksum-verified isolated package cache, fails on errors or warning-baseline drift, and retains the complete Validator output. The reviewed warning baseline keeps new warnings visible without turning every existing best-practice recommendation into a release blocker.

### Official terminology tests

The released `1.9.3` `general` baseline executed 597 tests and failed all 597, but this is not a 0% terminology-semantic score:

- Two metadata comparisons failed before operations: missing `software.releaseDate` and missing `TerminologyCapabilities.expansion.parameter` declarations.
- 593 operation requests reached the deliberate non-empty-body guard. Expected-success cases therefore returned HTTP 400; expected-error cases happened to receive a 4xx but failed on the generic `OperationOutcome` shape and issue coding.
- Two `$batch-validate` tests reached unsupported routes and returned HTTP 405.
- Three additional report actions were skipped by their mode gates and are not included in the 597 executed-test count.

The `sct-ecl` baseline executed 103 tests and failed all 103 at the same POST boundary. Every request carries an inline `ValueSet`; 98 expected-success cases received HTTP 400 and the five expected-error cases failed on their `OperationOutcome` details or extensions. The test requests target `http://snomed.info/xsct/31000003106/version/20250909`, while the public CI database contains 22 synthetic concepts from a different fixture. No semantic pass rate is meaningful until standard request transport and the exact licensed test subontology are both present.

These official-suite counts are retained as the historical baseline that motivated `R17e`; they predate the shipped POST boundary and must not be presented as current interoperability results. A new run against the exact licensed test subontology belongs to `R17f`.

The released corpus also needs ordinary test-fixture scrutiny. The baseline audit found duplicate names/output collisions and malformed expected ECL ValueSet URLs. Such cases must be confirmed, reported upstream, and isolated with a reviewable allowlist. `sct` must not be changed to reproduce an incorrect expectation simply to make a score green.

### ECL language evidence

The ECL 2.3 formal repository supplies 121 positive examples but no complete executable conformance or negative-syntax suite. The current parser accepts 42 of the 121 examples. The principal unsupported families are cardinality, description/concept filters, dotted and reverse attributes, concrete values, comments, alternate identifiers, and top/bottom constraints.

This count is a support inventory, not a conformance percentage. A partial implementation may correctly refuse a valid construct it does not claim. The gate needs a checked-in support manifest that says which pinned examples must parse and which must fail with a specific unsupported-construct error. Repository-owned negative cases and exact semantic result sets over the synthetic fixture cover the boundaries the official positive examples cannot.

Grouped attribute refinements are parsed and preserved, but the shared evaluator now refuses them explicitly until exact role-group semantics exist (`R92`). CLI, SDK, codelist, and FHIR regression tests prove they cannot be flattened into an ordinary conjunction; FHIR reports the refusal as an HTTP 400 `OperationOutcome`. No conformance claim may count grouped cases as supported before group-aware evaluation ships.

## Delivery stages

### Shipped R17a: independent structural gate

The blocking CI job builds the committed RF2 fixture through the real NDJSON and SQLite pipeline, starts the built `sct serve` binary on loopback, and captures both metadata resources, code-system search, lookup, both validate-code forms, subsumption, and expansion. It then runs the checksum-pinned Validator against R4 with terminology lookup and HTTP access disabled. The job fails on process failure, Validator errors or fatal issues, and any warning-baseline change. Its retained `fhir-validator-evidence` artefact contains the eight responses, complete Validator and server logs, requested download and response manifests, run identity, errors, and expected/actual warning sets.

### Shipped R17e: standard FHIR operation transport

Every shipped terminology operation now parses standard POST `Parameters` and normalizes supported primitive values through the same operation-specific input schema as GET. The boundary checks parameter names, exact `value[x]` datatypes, cardinality, primitive JSON types and lexical forms, malformed query values, and query/body conflicts before operation semantics run. Unsupported structured datatypes remain explicit errors rather than being discarded.

System-level `$expand` and `ValueSet/$validate-code` additionally accept inline SNOMED `ValueSet` resources with mandatory R4 `status` for the existing whole-system, single ECL `constraint =`, and extensional include/exclude semantics. Unsupported resource, compose, group, filter, and concept elements fail closed; `activeOnly` applies before extensional pagination, and `includeDefinition` preserves validated identifying metadata. The server remains read-only: this transport support adds no generic persistent terminology-resource store.

### R17f: official suite evidence

1. Run the pinned `general` suite publicly as a report-only architectural baseline until the product scope decision below is made.
2. Run `sct-ecl` privately against the exact licensed test subontology and edition URI, initially report-only and then gating the explicitly claimed supported profile.
3. Require runner exit status zero, `TestReport.status == completed`, `TestReport.result == pass`, the expected executed-test count, and no unexpected skips for a claimed mode. `status == completed`, a score, or output-file count alone is not a pass condition.
4. Upload the complete runner output directory on success or failure, including `test.log`, `test-results.json`, `report.json`, `actual/`, `expected/`, `conversions/`, and the run manifest. The R4 conversion evidence distinguishes server failures from R5-to-R4 conversion failures.
5. Record server commit/version, test package coordinate and hash, runner version and hash, modes, FHIR version, terminology edition/version, fixture provenance, and any reviewed corpus allowlist.

The command-line runner already returns zero only when all selected tests pass and one on any failure. CI should retain the report checks as defence against selecting no tests, the wrong mode, or an unexpected package even though the process exit status is meaningful.

The Validator blocks plain HTTP and private-network targets by default. Local runs use a settings file scoped to the exact loopback server URL with `allowHttp` and `allowPrivateNetwork`; do not disable those protections globally.

## Product scope decision

Passing the complete `general` mode is not a small extension of the current SNOMED server. The released suites send transient setup resources and exercise arbitrary code systems, ValueSet imports/exclusions, versions, languages, properties, supplements, ConceptMaps, and `tx-resource` parameters. Implementing that would create a generic in-memory terminology engine alongside the local SNOMED engine.

The current recommendation is not to build that engine solely to pursue an approval label. The standard POST boundary and structural gate are shipped; finish the ECL support profile and private SNOMED semantic evidence next. Reconsider complete `general` support only when a concrete consumer needs arbitrary transient terminology resources. If that decision changes, the published test registry requires FHIR Product Director review; the commit-pinned unreleased approval-page draft additionally proposes a credential-free public test endpoint loaded with the required content and `general` mode.

## Licensing boundary

- The terminology ecosystem package declares `CC0-1.0`, so its manifest, generic fixtures, and expected-response templates can be used in public CI subject to ordinary provenance.
- The SNOMED test subontology referenced by the suite is separately identified as SNOMED CT content subject to the Affiliate Licence. Do not commit it, publish it as a CI artefact, or place it in a public cache merely because the surrounding test metadata is CC0.
- The ECL formal-language repository is Apache-2.0 and is suitable for a pinned public syntax corpus. The separately published ECL specification prose carries SNOMED copyright terms; cite it rather than copying it into the repository.
- Real-release and comparator runs remain private unless the operator has separately established redistribution and publication rights for every retained artefact.

## Claim language

Use this wording until the external gates say more:

> `sct serve` implements a local, read-only, SNOMED CT-focused subset of the FHIR R4 terminology service. The documented GET and POST `Parameters` operation forms are covered by repository regression tests, and representative responses are checked with the HL7 FHIR Validator. Supported inline SNOMED `ValueSet` definitions use the existing expansion and membership semantics; arbitrary transient terminology resources and HL7 Terminology Ecosystem approval are not currently supported.

After an official-suite subset passes, report the exact evidence rather than replacing it with "FHIR compliant" or "ECL conformant":

> `sct` passed N/N selected tests from `hl7.fhir.uv.tx-ecosystem#1.9.3` in mode/suite X against terminology edition Y, with exclusions Z. This is test evidence for the named profile, not HL7 ecosystem approval.

## Sources

- [FHIR Terminology Ecosystem published test registry](https://hl7.org/fhir/uv/tx-ecosystem/testcases.html)
- [Draft approved-server criteria at pinned upstream commit](https://github.com/HL7/fhir-tx-ecosystem-ig/blob/f5dd4e257c5d84c5b5ea032cf153ce09d3255cc3/input/pagecontent/approved-servers.md)
- [FHIR Validator releases](https://github.com/hapifhir/org.hl7.fhir.core/releases)
- [ECL formal language release 2.3](https://github.com/IHTSDO/snomed-expression-constraint-language/releases/tag/2.3)
- [ECL specification release 2026-03-31](https://github.com/SNOMED-Documents/snomed-expression-constraint-language-specification/releases/tag/2026-03-31)
