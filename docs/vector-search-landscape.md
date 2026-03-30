# SNOMED CT Vector / Semantic Search — Landscape Review

*Researched March 2026. Covers production tools, research models, clinical NLP, and RAG
prototypes. Updated as the ecosystem evolves.*

---

## TL;DR

No open-source, local-first, single-binary tool combines RF2 ingestion with vector
semantic search — until `sct`. The only production server with genuine vector search is
John Snow Labs', which is commercial and cloud-oriented. The research literature has rich
embedding models but none are packaged as deployable SNOMED servers.

---

## 1. Production Terminology Servers

### Snowstorm (SNOMED International)

The flagship open-source SNOMED CT server. Elasticsearch-backed, used by ~14 national
editions as their authoritative browser backend.

**Search capabilities:** Elasticsearch typeahead/prefix/fuzzy, full Expression Constraint
Language (ECL), FHIR `ValueSet/$expand`.

**Vector/semantic search:** None. The "semantic index" in the Snowstorm changelog refers
to the SNOMED CT inferred relationship index (IS-A consistency bookkeeping), not ML
embeddings. No movement in this direction as of v10.5.0.

**Verdict:** Best-in-class for lexical/ECL; zero embedding-based similarity.

---

### Hermes (wardle/hermes — Clojure)

Lightweight SNOMED library and microservice. LMDB storage, Apache Lucene full-text
search, no JVM heap tuning required. Imports UK + International in ~5 minutes. Used in
NHS production deployments. FHIR facade: Hades.

**Vector/semantic search:** None. Community interest noted (Clojure Slack, May 2024)
but aspirational only.

**Verdict:** Best lightweight alternative to Snowstorm; no vector capability.

---

### Ontoserver (CSIRO, Australia)

FHIR-native terminology server (Postgres + Lucene). The only server with full SNOMED CT
postcoordination support. Used by the Australian Digital Health Agency. Commercial
licence required.

**Vector/semantic search:** None natively. CSIRO's own published research acknowledges
that their autocompletion performs poorly when input strings are not partial prefixes and
proposes BioBERT-based semantic search as a future extension — but this has not been
integrated into the product.

**Verdict:** Best FHIR-conformance story; no vector capability.

---

### HAPI FHIR JPA Server

Java/Spring open-source FHIR server with Lucene/Hibernate Search for terminology.
Snowstorm uses the HAPI FHIR library for its own FHIR layer.

**Vector/semantic search:** None.

---

### John Snow Labs Terminology Server ⚡ *Only production server with vector search*

Commercial product embedded in the Spark NLP / John Snow Labs ecosystem.

**What it does:**
- FAISS-backed dense vector search across SNOMED CT, ICD-10, LOINC, RxNorm
- Handles synonyms, misspellings, colloquialisms ("woozy" → adverse event code)
- Sub-second at millions of concept entries; horizontally scalable
- Cross-system semantic matching

**Verdict:** The only production-grade SNOMED server with genuine semantic search.
Closed-source, commercial, cloud/server-oriented.

---

## 2. MCP Servers for LLMs

### eigenbau/mcp-snomed-ct

Wraps any FHIR R4 server (Snowstorm, Ontoserver, HAPI FHIR) to expose SNOMED lookup
tools to LLMs via Model Context Protocol. Supports Claude Desktop, Claude Code. Domain
scoping via ECL.

**Vector search:** None — proxies the underlying server's lexical/ECL search. Semantic
reasoning is provided by the LLM, not by embeddings.

---

### SidneyBissoli/medical-terminologies-mcp

MCP server with 27 tools spanning ICD-11, SNOMED CT, LOINC, RxNorm, MeSH via public
NLM/WHO APIs. No vector search.

---

## 3. Research Embedding Models

These are model weights, not packaged servers. They require you to build your own
embedding + index pipeline.

### SapBERT (Cambridge, NAACL 2021 — dominant 2024–2026)

Self-alignment pre-training on UMLS synonyms using metric learning. Clusters synonyms
of the same concept close together in 768-dim space. The most widely adopted backbone
for SNOMED concept normalization.

**HuggingFace models:**
- `cambridgeltl/SapBERT-from-PubMedBERT-fulltext` — English, 768-dim
- `cambridgeltl/SapBERT-from-PubMedBERT-fulltext-mean-token` — used by SNOBERT
- `cambridgeltl/SapBERT-UMLS-2020AB-all-lang-from-XLMR` — multilingual (XLM-R Large)

**2024 result (Abdulnazar et al., *Digital Health*):** Unsupervised SNOMED CT annotation
in English and German using SapBERT + FAISS. English F1: 0.765 unsupervised.

**Note for `sct`:** SapBERT would likely produce significantly better SNOMED embeddings
than a general model like `nomic-embed-text`, since it was specifically trained on
biomedical concept synonymy. Worth supporting as an `--model` option in `sct embed`.

---

### CODER / CODER++ (GanjinZero, JBI + ACL-BioNLP 2022)

Contrastive learning over UMLS knowledge graph (terms + relation triplets). Cross-lingual
support. Hard negative sampling in CODER++.

**HuggingFace:** `GanjinZero/coder_eng`, `GanjinZero/coder_eng_pp`

**Licence:** MIT

---

### HiT-MiniLM-L12-SnomedCT (Oxford, NeurIPS 2024)

Fine-tuned from `all-MiniLM-L12-v2` on SNOMED CT's is-a hierarchy using hyperbolic
geometry losses (Hyperbolic Clustering + Hyperbolic Centripetal). Optimised for
predicting subsumption (is-a), not for free-text → concept similarity.

**HuggingFace:** `Hierarchy-Transformers/HiT-MiniLM-L12-SnomedCT`

**Use case:** Ontology alignment / hierarchy transfer, not clinical NLP search.

---

### SNOBERT (Kulyabin et al., arXiv 2405.16115, 2024)

Benchmark + pipeline for clinical note entity linking to SNOMED CT. Two-stage: (1) BERT
NER, (2) SapBERT-embedded FAISS index (~200k concept IDs) for candidate matching.

**GitHub:** `MikhailKulyabin/SNOBERT` (MIT)

---

### Snomed2Vec (arXiv 1907.08650)

Random walk + Poincaré embeddings on the SNOMED CT knowledge graph. 5–6× improvement on
concept similarity vs prior embeddings. Older codebase, not actively maintained.

**GitHub:** `NachusS/Snomed2Vec`

---

### BioWordVec (NCBI)

Word2Vec trained on PubMed + MIMIC-III. MeSH-trained, not SNOMED-specific, but widely
used as a biomedical embedding baseline. 200-dim, 13 GB binary.

**Download:** NCBI FTP (`BioWordVec_PubMed_MIMICIII_d200.vec.bin`)

---

## 4. Clinical NLP Tools

These are *annotation* tools (free text → SNOMED concept ID), not *search* tools
(query → ranked SNOMED concepts). Different problem, but relevant context.

### MedCAT v2 (CogStack — Apache 2.0)

Self-supervised NER + linking. Learns contextual concept embeddings from clinical text;
resolves ambiguous mentions via cosine similarity to learnt concept centroids.

**SNOMED models available (2025):**
- UK Clinical Edition 39.0 (Oct 2024) + UK Drug Extension, trained on MIMIC-IV
- UK Clinical Edition 40.2 (Jun 2025) + UK Drug Extension 40.3

Deployed at UCLH and multiple NHS trusts. Models require NIH/UMLS licence.

**v2.0.0 released August 2025:** Modular architecture, optional spaCy, reduced core
dependencies. Repo: `CogStack/cogstack-nlp`.

---

### scispaCy (Allen AI)

Biomedical NLP with spaCy models. Entity linking to UMLS via TF-IDF approximate nearest
neighbour (not dense vectors). SNOMED codes obtained via UMLS crosswalk.

---

### DrivenData SNOMED CT Entity Linking Challenge (2024)

Open challenge with open-source winning solutions:
- BERT NER + FAISS vector search against SapBERT-embedded SNOMED index
- LoRA fine-tuned LLM + FAISS retrieval + LLM reranking
- Embedding model fine-tuned on SNOMED synonym pairs

**GitHub:** `drivendataorg/snomed-ct-entity-linking` (MIT)

---

## 5. RAG / LLM-Native Prototypes

### OntologyRAG (IQVIA, arXiv 2502.18992, February 2025)

Three-stage pipeline: SNOMED CT + other ontologies → RDF knowledge graph → NL2SPARQL
retrieval → LLM reasoning. Demonstrates ICD-10 → ICD-11 mapping with proximity levels
and interpretable rationale. SNOMED CT support included.

**GitHub:** `iqvianlp/ontologyRAG`

**Status:** Research prototype.

---

### django-snomed-ct + OgbujiPT (Chimezie Ogbuji, 2023)

Proof-of-concept: CNL sentences extracted from SNOMED CT concepts, embedded, stored in
in-memory Qdrant, used for RAG-based clinical Q&A. Not a packaged tool.

---

### HengJay/snomed-ct-assistant (HuggingFace Space)

Pre-built ChromaDB vector database of 634k SNOMED CT concepts with a simple chat
interface. Embeddings + ChromaDB index files included.

**Status:** Demo/prototype. Requires SNOMED CT licence.

---

### Biomedical Text Normalization (medRxiv 2024)

Benchmarked four LLM strategies for SNOMED term normalization. RAG achieved **88.31%**
accuracy (domain-specific) and **79.97%** (broad medical). No released tool.

---

## 6. Pre-Computed Embeddings (Downloadable)

All datasets derived from SNOMED CT require a valid SNOMED CT licence (free for most
countries via MLDS or national licence).

| Resource | Type | Dimensions | Licence |
|---|---|---|---|
| `cambridgeltl/SapBERT-from-PubMedBERT-fulltext-mean-token` | Model (generate yourself) | 768 | Apache 2.0 |
| `GanjinZero/coder_eng_pp` | Model (generate yourself) | 768 | MIT |
| `Hierarchy-Transformers/HiT-MiniLM-L12-SnomedCT` | Hierarchy-aware model | 384 | Apache 2.0 |
| `xlreator/biosyn-biobert-snomed` | BioBERT sentence embedding model | 768 | — |
| `FremyCompany/AGCT-Dataset` | Pre-computed ADAv2 embeddings (Parquet) | 1536 | + SNOMED licence |
| `HengJay/snomed-ct-assistant` | 634k concepts, ChromaDB index | — | + SNOMED licence |
| `dchang56/snomed_kge` | 5 KGE checkpoints (TransE, RotatE, etc.) | 200 | Research |
| `justin13601/loinc_snomed_embeddings` | SNOMED + LOINC, ADAv2, Parquet | 1536 | + SNOMED + LOINC licences |
| BioWordVec (NCBI FTP) | PubMed + MIMIC-III word vectors | 200 | Open (MeSH-trained) |

---

## 7. Capability Comparison

| Capability | Snowstorm | Hermes | Ontoserver | MedCAT | JSL Server | `sct` |
|---|---|---|---|---|---|---|
| Local / offline, no server | No | No | No | Partial | No | Yes |
| Single binary, zero deps | No | No (JVM) | No | No | No | Yes |
| RF2 → queryable artefact | Hours | ~5 min | No | No | No | ~30 s |
| Lexical / FTS search | Yes | Yes | Yes | No | Yes | Yes (SQLite FTS5) |
| Vector / semantic search | **No** | **No** | **No** | Partial | Yes (commercial) | Yes (Ollama) |
| MCP server for LLMs | No | No | No | No | No | Yes |
| Parquet / DuckDB export | No | No | No | No | No | Yes |
| Per-concept Markdown (RAG) | No | No | No | No | No | Yes |
| Open source + permissive | Yes | Yes | No | Yes | No | Yes |
| Production terminology server | Yes | Yes | Yes | No | Yes | Not goal |

---

## 8. Implications for `sct`

1. **The gap is real.** No open-source tool currently combines offline RF2 ingestion with
   vector semantic search end-to-end. `sct` fills this gap.

2. **Consider SapBERT as a recommended embedding model.** `nomic-embed-text` is a good
   general-purpose baseline, but `SapBERT-from-PubMedBERT-fulltext-mean-token` (Apache
   2.0, HuggingFace) was specifically trained on biomedical concept synonymy and would
   likely produce substantially better results for clinical queries. It can be run via
   Ollama once a compatible GGUF is available, or called directly via the HuggingFace
   Transformers API.

3. **Pre-built embedding artefacts are a distribution opportunity.** Several datasets
   on HuggingFace provide pre-computed SNOMED embeddings. `sct` could publish an
   official pre-built `.arrow` embeddings file alongside each NDJSON release (for
   distributions where SNOMED CT licensing permits), saving users the Ollama step
   entirely. See `specs/roadmap.md` — IPS Free Set bundling is already flagged as a
   future item.

4. **MedCAT v2 is complementary, not competing.** MedCAT maps free clinical text → SNOMED
   concept IDs. `sct mcp` maps a clinical question → ranked SNOMED concepts. These are
   different stages of a clinical NLP pipeline and could be used together.

---

*Sources: Snowstorm changelog, Hermes README, Ontoserver PMC 8861757, SapBERT NAACL 2021,
CODER JBI 2022, HiT NeurIPS 2024, SNOBERT arXiv 2405.16115, MedCAT CogStack Discourse,
DrivenData 2024, OntologyRAG arXiv 2502.18992, JMIR scoping review PMC 11494256,
John Snow Labs product docs, medRxiv 2024 normalization benchmark.*
