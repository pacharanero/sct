# FHIR Conformance And Benchmarks

`sct` keeps four different checks separate:

1. **Repository regression checks**: do the documented query-string operations
   still return the expected results over the committed synthetic fixture?
2. **FHIR structural validation**: are representative responses valid base-R4
   resources according to the independent HL7 FHIR Validator?
3. **Official terminology interoperability tests**: does the server pass the
   selected released suites from the HL7 FHIR Terminology Ecosystem?
4. **Performance benchmarks**: once the relevant correctness profile passes,
   how fast is it compared with local SQLite and other terminology servers?

The distinction matters. A fast server that returns the wrong `$expand` result
is not useful, a structurally valid response can still contain the wrong concept
set, and a repository-owned suite is not independent conformance evidence.

## Evidence, Not Certification

The current CI runner is a repository-specific smoke/regression profile aligned
with these FHIR R4 terminology operations:

- [`/metadata`](https://hl7.org/fhir/R4/http.html#capabilities)
- [`CodeSystem/$lookup`](https://hl7.org/fhir/R4/codesystem-operation-lookup.html)
- [`CodeSystem/$validate-code`](https://hl7.org/fhir/R4/codesystem-operation-validate-code.html)
- [`CodeSystem/$subsumes`](https://hl7.org/fhir/R4/codesystem-operation-subsumes.html)
- [`ValueSet/$expand`](https://hl7.org/fhir/R4/valueset-operation-expand.html)
- [`ValueSet/$validate-code`](https://hl7.org/fhir/R4/valueset-operation-validate-code.html)
- [`ConceptMap/$translate`](https://hl7.org/fhir/R4/conceptmap-operation-translate.html)

It proves the local contract remains stable; it is not an official HL7 suite and
not an HL7 certification badge. The independent layers are:

| Layer | Tool | What it checks |
|---|---|---|
| Base R4 structure | [HL7 FHIR Validator](https://github.com/hapifhir/org.hl7.fhir.core/releases) | Resource cardinalities, datatypes and invariants |
| Terminology interoperability and semantics | [FHIR Terminology Ecosystem `txTests`](https://hl7.org/fhir/uv/tx-ecosystem/testcases.html) | Released operation requests and expected responses for selected modes |
| Proposed HL7 ecosystem approval criteria | [Unreleased 1.9.5-SNAPSHOT draft at a pinned upstream commit](https://github.com/HL7/fhir-tx-ecosystem-ig/blob/f5dd4e257c5d84c5b5ea032cf153ce09d3255cc3/input/pagecontent/approved-servers.md) | Passing released tests, a public reproducible endpoint, and FHIR Product Director review; draft guidance, not a published approval standard |

The reproducible baseline pins FHIR Validator 6.10.4 and
`hl7.fhir.uv.tx-ecosystem#1.9.3`; never use the mutable `current` package in CI.
The published test registry currently uses an unversioned page while the package
coordinate and hash pin the executable corpus. The separate approval-page source
above is explicitly an unreleased draft and must not be presented as a current
published HL7 requirement.
The official command is:

```bash
java -jar validator_cli-6.10.4.jar txTests \
  -tx http://127.0.0.1:8080/fhir \
  -test-version 1.9.3 \
  -mode general \
  -output ./tx-results \
  -fhir-settings ./fhir-settings.json
```

The Validator blocks plain HTTP and private-network targets by default. For this
loopback URL, use a narrowly scoped settings file rather than disabling checks
globally:

```json
{
  "servers": [{
    "url": "http://127.0.0.1:8080/fhir",
    "type": "fhir",
    "authenticationType": "none",
    "allowHttp": true,
    "allowPrivateNetwork": true
  }]
}
```

Retain the complete runner output directory for every evidenced R4 run,
including `test.log`, `test-results.json`, `report.json`, `actual/`, `expected/`
and `conversions/`. The conversion output distinguishes server failures from
failures while converting the test corpus between FHIR versions.

The first baseline is an interoperability diagnosis, not a semantic score. The
`general` run executed 597 tests: 593 operation requests were refused at the
standard POST `Parameters` boundary, two `$batch-validate` requests reached
unsupported routes, and two metadata comparisons failed. The `sct-ecl` suite's
103 requests were also refused before ECL evaluation because they carry inline
`ValueSet` bodies. They target a licensed SNOMED test subontology that is not the
committed 22-concept synthetic fixture, so those results must not be reported as
"0% ECL conformance".

The local runner remains useful because benchmark evidence needs a stable,
reproducible workload on developer machines, deployments and CI. The detailed
pins, licensing boundary, staged external-gate plan and exact claim language are
in the [`R17` conformance evidence record](https://github.com/pacharanero/sct/blob/main/spec/fhir-conformance.md).

## Run Conformance First

Start a terminology server:

```bash
sct serve --db snomed.db --host 127.0.0.1 --port 8080 --fhir-base /fhir
```

Then run:

```bash
benchmarks/conformance.sh --server http://localhost:8080/fhir
```

The runner checks:

| Area | What is asserted |
|---|---|
| CapabilityStatement | FHIR R4 version and advertised terminology operations |
| `$lookup` | `Parameters` shape, display text, designations and parent properties |
| `CodeSystem/$validate-code` | true/false outcomes, including display mismatch |
| `$expand` | ECL expansion, text filtering, result totals and expected members |
| `$subsumes` | `subsumes`, `subsumed-by`, `equivalent`, `not-subsumed` |
| `ValueSet/$validate-code` | membership against implicit SNOMED ECL ValueSets |
| `$translate` | SNOMED to ICD-10 and reverse mapping when advertised |
| Errors | FHIR `OperationOutcome` responses and expected HTTP status codes |

Write machine-readable output for CI or later reporting:

```bash
benchmarks/conformance.sh \
  --server http://localhost:8080/fhir \
  --output reports/sct-conformance.jsonl
```

If a target server does not advertise `ConceptMap/$translate`, the translate
checks are skipped by default. Use `--strict` when comparing only servers that
are expected to support the full `sct` surface.

## Then Benchmark

After conformance passes:

```bash
benchmarks/bench.sh \
  --db snomed.db \
  --server http://localhost:8080/fhir \
  --runs 20 \
  --warmup 5 \
  --write-benchmarks
```

The existing benchmark covers:

- concept lookup
- free-text search
- direct children
- ancestor traversal
- subsumption
- bulk lookup

The benchmark reports wall-clock medians and standard deviation. Local SQLite
timings include process startup. FHIR timings include HTTP overhead.

## Compare Against Snowstorm Or Ontoserver

To make a credible public claim:

1. Load the same SNOMED CT release into every server.
2. Run each server on the same hardware class.
3. Warm the filesystem, JVM, Elasticsearch/Lucene and SQLite caches.
4. Run `benchmarks/conformance.sh` first.
5. Only publish benchmark results for servers that pass the relevant
   conformance profile.
6. Record exact versions, heap settings, database size, disk type, CPU, RAM,
   operating system and release package.

Example:

```bash
# sct
benchmarks/conformance.sh --server http://localhost:8080/fhir
benchmarks/bench.sh --db snomed.db --server http://localhost:8080/fhir --runs 20 --warmup 5

# Snowstorm Lite
benchmarks/conformance.sh --server http://localhost:8081/fhir
benchmarks/bench.sh --db snomed.db --server http://localhost:8081/fhir --runs 20 --warmup 5

# Ontoserver or another FHIR terminology server
benchmarks/conformance.sh --server http://localhost:8082/fhir
benchmarks/bench.sh --db snomed.db --server http://localhost:8082/fhir --runs 20 --warmup 5
```

The conformance checks are deliberately fixture based so the same request
matrix can be used across implementations. The benchmark fixtures should be
expanded over time with more high-fanout hierarchies, deep concepts, inactive
concepts, refsets and cross-map workloads.

## Public Methodology

When publishing results, include:

- exact `sct` version and git commit
- SNOMED CT edition and release date
- whether refsets and crossmaps were loaded
- server base URL path, for example `/fhir`
- hardware and operating system
- Docker image tags or binary versions for comparator servers
- cache state: cold start, warm cache, or both
- number of runs, warmup runs and timeout
- full conformance pass/fail output
- raw benchmark output

The headline number should be scoped. For example, "`sct serve` is faster for
these read-only SNOMED CT terminology operations on this release and hardware"
is defensible. A general claim that one terminology server is universally
faster than another is not.
