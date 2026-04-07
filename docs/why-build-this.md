## The Problem with Terminology Servers

Every clinical software project that uses SNOMED CT eventually ends up in the same place: standing up a terminology server. The de facto standard advice from SNOMED International themselves is to run Snowstorm, their reference implementation. The alternative is to pay for a hosted service like CSIRO's Ontoserver, or use a cloud FHIR endpoint.

This has become so normalised that the question "how do I use SNOMED CT?" has only one accepted answer: "run a server."

That answer is wrong for a large class of use cases.

---

## The Overhead of Server-Based Approaches

### Operational complexity

Running Snowstorm locally requires:

- Docker and Docker Compose
- Elasticsearch (which itself requires at minimum 4GB of RAM reserved)
- The Snowstorm JVM process on top of that
- A multi-gigabyte RF2 import that takes 30-60 minutes
- Port management, health checks, restart policies

This is a non-trivial operational burden for what is essentially a read-only lookup against a static dataset. The terminology does not change between SNOMED releases (twice a year). You are running a full distributed search cluster to serve read-only queries against data that changes twice a year.

### Network and TLS overhead

Even when running "locally," most server-based approaches communicate over HTTP. In practice this means:

- TCP connection setup for every query (or connection pool management)
- TLS handshake overhead if using HTTPS (mandatory for any production deployment)
- JSON serialisation and deserialisation on both sides of every request
- HTTP header overhead per request
- Latency measured in milliseconds for what should be a microsecond operation

For interactive use - a clinician coding a record, a developer testing a query, an LLM reasoning about a concept - this overhead is tolerable. For batch processing - validating a dataset of 100,000 coded records, running analytics over a research cohort, building a search index - it becomes a genuine bottleneck. As Mark Wardle [puts it in the Hermes README](https://github.com/wardle/hermes#how-is-this-different-to-a-national-terminology-service): "when I do analytics, I can't see me making server round-trips for every check of subsumption! That would be silly."

### The LLM use case makes this worse

Language models have no persistent connection state. Every tool call is a fresh request. If an LLM is using SNOMED CT as part of a reasoning chain - "is this diagnosis a subtype of this hierarchy? what are the preferred terms for these codes? what attributes does this concept have?" - it may make dozens of concept lookups in a single conversation turn. Each one goes over HTTP to a terminology server.

Worse, most LLM-accessible SNOMED tools today go over the public internet to a remote server. This introduces:

- Dependency on external availability (what happens when SNOMED International's public Snowstorm instance is down?)
- Data governance questions (you are sending patient-adjacent coded data to a third-party endpoint)
- Unpredictable latency
- Rate limiting

---

## Reproducibility as a Clinical Safety Property

Terminology servers are stateful infrastructure. When a terminology server answers a query, the answer depends on which release is loaded, whether the server has been updated since the last deployment, and whether the instance in production matches the instance in staging or development. None of this is typically captured in source control.

A `snomed.db` file checked into a repository alongside application code changes this. The exact terminology version used by a system becomes:

- **Explicit** — the file's content hash identifies the release unambiguously
- **Versioned** — git history tracks when the terminology was updated and who approved it
- **Reproducible** — checking out a commit gives you both the code and the terminology it was written against
- **Auditable** — a clinical decision made at a point in time can be re-evaluated against the exact terminology snapshot that was in use

This matters for clinical safety. A subsumption hierarchy that influenced a clinical decision rule, an inclusion criterion that defined a research cohort, a validation check that flagged an anomalous code — these are not abstract software artefacts. They have clinical consequences, and the question "what did the system believe about this concept on this date?" should have a deterministic answer.

Terminology server deployments rarely provide this. Snowstorm is updated out-of-band from the application that depends on it. There is no convention for pinning a terminology server to a specific release in the way you would pin a library dependency. The result is that the terminology version is implicit, often unknown, and effectively unauditable after the fact.

The file-as-artefact approach treats terminology as a dependency, not as infrastructure. This is the same shift that containerisation brought to runtime environments and that lockfiles brought to package management. It is an obvious property to want for any safety-relevant data dependency.

---

## What Exists - and Why It Doesn't Fit

### Snowstorm (IHTSDO/snowstorm)

[https://github.com/IHTSDO/snowstorm](https://github.com/IHTSDO/snowstorm)

The official SNOMED International terminology server. It is open source, well-maintained, and genuinely capable - it is the server behind the international SNOMED browser and is used in production by national release centres worldwide.

But it is built on Elasticsearch, requires a JVM, needs gigabytes of RAM, and takes an hour to import a release. It is an enterprise server built for enterprise workloads. It is the right tool for a national terminology service serving thousands of concurrent users. It is the wrong tool for a developer's laptop or a local AI tool.

### Hermes (wardle/hermes)

[https://github.com/wardle/hermes](https://github.com/wardle/hermes)

Mark Wardle's Clojure implementation uses LMDB and Apache Lucene to produce a much more lightweight tool than Snowstorm — it imports in under 5 minutes, requires no Elasticsearch, and runs from a single JAR or directly from source code. It can be installed via Homebrew (`brew install wardle/tools/hermes`).

Hermes is designed as a **library first** — it can be embedded directly into JVM applications with in-process function calls and no network overhead. It also provides optional HTTP and MCP servers when needed. It does not "fundamentally run as a server" — the HTTP and MCP interfaces are thin optional wrappers around a library API.

Performance is a core design goal: sub-microsecond concept lookups, 82,000+ req/s for concurrent operations on a modest laptop, driven by LMDB's zero-copy memory-mapped reads and Lucene's optimised full-text search. It provides a comprehensive terminology API including transitive closure, subsumption testing, ECL evaluation, compositional grammar support, cross-mapping, OWL reasoning, and a HL7 FHIR terminology server via [hades](https://github.com/wardle/hades).

For example, to download, install, and index the complete UK monolith edition:

```shell
brew install wardle/tools/hermes
hermes uk.nhs/sct-monolith --db snomed.db --progress --api-key trud/api-key.txt --cache-dir trud/cache install index compact status
```

This downloads the distribution, imports the RF2 files, builds indices, compacts the database, and prints status — all in a single command that completes in under 5 minutes.

Hermes uses LMDB rather than SQLite for storage — a deliberate choice that trades ad-hoc queryability for zero-copy memory-mapped reads and the performance characteristics above. LMDB is not proprietary; it is a BSD-licensed, widely-used embedded key-value store, inspectable with standard tools (`mdb_stat`, `mdb_dump`, etc.). Hermes runs on the JVM, so startup time is slower than a compiled binary, but runtime performance is excellent as the [benchmarks](https://github.com/wardle/hermes#indicative-benchmarks) demonstrate. It is written in Clojure, a relatively niche language — this does not affect users of the HTTP, MCP, or Java APIs, but may be a barrier to direct source contributions or building custom command-line tooling on top of the library.

### eigenbau/mcp-snomed-ct

[https://github.com/eigenbau/mcp-snomed-ct](https://github.com/eigenbau/mcp-snomed-ct)

A Python MCP server for SNOMED CT. The MCP interface design is sensible - it exposes tools like `snomed_lookup`, `snomed_get_by_code`, `snomed_get_related`. But it is a thin wrapper over a FHIR R4 terminology server. The local backend option still requires running Snowstorm locally (with all the Docker/Elasticsearch overhead that implies). The remote backend sends queries to Ontoserver or another cloud endpoint.

It solves the "SNOMED in an LLM tool" problem at the interface layer while leaving the underlying infrastructure problem entirely unsolved.

### SidneyBissoli/medical-terminologies-mcp

[https://github.com/SidneyBissoli/medical-terminologies-mcp](https://github.com/SidneyBissoli/medical-terminologies-mcp)

A broader multi-terminology MCP server covering ICD-11, SNOMED CT, LOINC, RxNorm and MeSH. The breadth is useful but the implementation is entirely API-call-based - every query goes to an external web service. This is the opposite of what is described here: it adds an MCP layer on top of the existing server-dependency problem rather than solving it.

### IHTSDO/rf2-to-json-conversion

[https://github.com/IHTSDO/rf2-to-json-conversion](https://github.com/IHTSDO/rf2-to-json-conversion)

SNOMED International's own RF2 to JSON conversion tool. This is actually close in spirit to Layer 1 of the approach described here - it produces JSON files from RF2. But it then loads them into MongoDB, reintroducing a server dependency. The project is also several years old, Java-based, and not maintained as an active tool.

### IHTSDO/snomed-database-loader

[https://github.com/IHTSDO/snomed-database-loader](https://github.com/IHTSDO/snomed-database-loader)

Includes some SQLite tooling as part of a broader Neo4j-oriented workflow. The SQLite component exists only as an intermediate step toward a graph database import, not as a first-class output. There is no FTS5 support, no denormalised schema, and no notion of the file as a portable artefact.

---

## The Gap

None of the existing tools provide:

- A **deterministic RF2 transform** that produces a standard, portable intermediate artefact
- A **single SQLite file** with FTS5 that can be queried with the standard `sqlite3` binary
- A **Rust binary** with no runtime dependencies that serves SNOMED via MCP over stdio
- **Flat files on disk** (NDJSON or markdown) designed for direct LLM consumption and RAG indexing
- Any path that does not eventually require a running server process

The "data over services" insight is not novel - it is how every other dataset is handled. You do not run a terminology server to look up a postcode. You do not spin up Elasticsearch to search a CSV. The clinical informatics community has internalised the terminology-server pattern so deeply that the obvious alternatives have not been built.

---

## Who This Is For

This toolchain is designed for:

- **Developers** building clinical applications who want SNOMED lookup without operational overhead
- **Data engineers** running batch validation or analytics pipelines over coded datasets
- **Researchers** who need SNOMED concept relationships for cohort definition or phenotyping, locally
- **AI/LLM tool builders** who need SNOMED accessible via MCP without a server dependency
- **Clinical informaticians** who want to introspect and explore SNOMED content from the command line

It is not designed to replace a national terminology server. It is designed for all the use cases where a national terminology server is the wrong answer.

---

## On SNOMED Licensing

SNOMED CT is licensed. Use requires either national membership (covered in the UK by NHS England's national licence, which covers NHS organisations and their suppliers) or an affiliate licence from SNOMED International.

This toolchain does not distribute SNOMED CT data. It provides tooling to transform and consume a licensed copy of the RF2 files that the user has obtained themselves. The output artefacts (SQLite file, NDJSON, Parquet) contain SNOMED CT content and are therefore subject to the same licence terms as the source RF2 data.
