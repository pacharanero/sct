# ECL - Expression Constraint Language for `sct`

**Status:** Shipped (v0.5.0 onward). The parser and evaluator (hierarchy, refset, boolean, wildcard, and attribute refinement) power `sct ecl`, `sct codelist add --ecl`, and `sct serve` ValueSet expansion. Deferred items (cardinality, reverse/dotted attributes, group cardinality, and whole-AST SQL compilation) remain future work - see §5 and §9.
**Scope:** Parse and evaluate SNOMED CT Expression Constraint Language (ECL) against a local `sct` SQLite database, returning the set of matching concept SCTIDs. First consumer: `sct codelist add --ecl "<<73211009"`.
**Audience:** A coding agent (and Marcus) implementing this in the `sct` repo.

---

## 1. Why

ECL is the only standardised query language for SNOMED CT. Every terminology server speaks it; every serious codelist specification is written in it. `<<73211009` means "Diabetes mellitus and all its subtypes" - the single most common operation in codelist construction and phenotyping.

`sct` needs ECL for three converging reasons:

1. **Codelists.** `sct codelist add --ecl "<<73211009"` expands an ECL expression into concrete concepts and appends them - far more powerful than the existing `--include-descendants` flag, and the natural way clinicians and researchers specify intent.
2. **`sct serve`.** FHIR `ValueSet/$expand` uses this evaluator for hierarchy, refset membership, boolean composition, and attribute filters.
3. **SCT-QL.** The friendly query language in `spec/sct-ql-spec.md` is designed to *compile to ECL*. So ECL is the **intermediate representation** the whole query stack converges on: SCT-QL → ECL AST → SQL. Building ECL first is the foundation, not a detour.

This document covers the ECL **engine** - parser plus evaluator. SCT-QL sugar and the FHIR HTTP routing remain separate surfaces over that engine.

---

## 2. Substrate - what the database already provides

Evaluation needs three kinds of lookup. Two exist today; one is added by this work.

| Operator family | Backing data | Status |
|---|---|---|
| Hierarchy (`<`, `<<`, `>`, `>>`, `<!`, `>!`) | `concept_ancestors` TCT when usable; `concept_isa(child_id, parent_id)` fallback | **exists** - works on any DB from `sct sqlite` |
| Refset member (`^`) | `refset_members(refset_id, referenced_component_id)` | **exists** (needs `--refsets simple` at ingest) |
| Attribute refinement (`:`) | typed relationship triples `(source, type, destination, group)` | **added here** - see §4 |

Transitive hierarchy traversal uses `concept_ancestors` as an indexed fast path only when its transactional completion marker, schema, indexes, and source/closure invalidation triggers are valid. Missing, legacy, partial, stale, or malformed TCTs fall back to recursive CTEs on `concept_isa`, so ECL remains correct on a stock database without requiring `--transitive-closure`. CLI callers receive one canonical stderr build-or-repair instruction; adapters that cannot write diagnostics to stderr can reuse the same guidance in their own metadata channel.

---

## 3. Architecture

```
ECL text
  │  tokenizer  (src/ecl/lex.rs)
  ▼
token stream
  │  recursive-descent parser  (src/ecl/parse.rs)
  ▼
AST  (src/ecl/ast.rs)
  │  evaluator  (src/ecl/eval.rs) - walks the AST, querying SQLite
  ▼
Set<SCTID>
```

A **hand-written recursive-descent parser** rather than a grammar-generator dependency: ECL has no operator-precedence puzzles or left-recursion once factored, the project carries no parser crate today, and hand-rolling gives precise error messages (`expected '=' after attribute name at position 14`). `spec/sct-ql-spec.md` recommends `pest` for SCT-QL; that remains an option there, but ECL's surface is small enough to not warrant the build-time proc-macro.

Text input is bounded before it can create a stack-unsafe AST: parenthesised expressions and attribute groups may nest up to 200 levels; flat associative boolean/refinement chains accept up to 10,000 terms and are stored as balanced binary trees; repeated left-associative `MINUS` accepts up to 200 terms. Inputs above those limits return a parse error rather than aborting the process. These limits apply to text parsed by the engine, not to callers manually constructing the public AST types.

The evaluator computes **sets of SCTIDs** bottom-up. For v1 the set algebra runs in Rust (`BTreeSet<u64>`) with hierarchy/refset membership pulled from SQLite; transitive hierarchy terms use indexed TCT lookups when available and recursive CTEs otherwise. This is correct and fast for the fixture tests and for realistically-sized codelist queries. The eventual scale story - compiling a whole AST to a single SQL query (per `sct-ql-spec.md`) - is a later optimisation, noted but not built now.

---

## 4. Data pipeline change - typed relationship triples

Attribute refinement is the one operator family with no backing data today. The raw triples *are* parsed from RF2 (`rf2.rs` keeps `(type_id, dest_id, group)` as SCTIDs in `Rf2Dataset.attributes`) but are dropped in `builder.rs`, which converts `type_id` to a display label and discards the group. We persist them additively:

1. **NDJSON schema v4.** `ConceptRecord` gains a `relationships: Vec<Relationship>` field, where `Relationship { type_id, destination_id, group }` are all SCTIDs/ints. `#[serde(default)]` so older v3 NDJSON still parses (empty relationships). `SCHEMA_VERSION` → 4; the existing version-validation degrades gracefully on older DBs.
2. **`builder.rs`** populates `relationships` from the already-parsed triples - no re-parsing, the data is already in hand.
3. **`sct sqlite`** writes a `concept_relationships(source_id, type_id, destination_id, group_num)` table, indexed on `source_id` and on `(type_id, destination_id)`.

The existing label-keyed `attributes` JSON column stays untouched, for display. The new table is purely additive. **Consequence:** attribute-refinement ECL requires a database rebuilt with this version (`sct ndjson` + `sct sqlite`); hierarchy/refset ECL works on existing databases unchanged.

```sql
CREATE TABLE concept_relationships (
    source_id      TEXT NOT NULL,
    type_id        TEXT NOT NULL,   -- attribute type SCTID, e.g. 363698007 |Finding site|
    destination_id TEXT NOT NULL,
    group_num      INTEGER NOT NULL
);
CREATE INDEX idx_rel_source    ON concept_relationships(source_id);
CREATE INDEX idx_rel_type_dest ON concept_relationships(type_id, destination_id);
```

---

## 5. Supported grammar (slice 1)

Operators on a focus concept:

| ECL | Meaning | Evaluation |
|---|---|---|
| `123` | the concept itself | `{123}` |
| `*` | any concept | all active concept ids |
| `<123` | descendants | indexed TCT lookup, or recursive CTE down `concept_isa` |
| `<<123` | descendants or self | descendants ∪ `{123}` |
| `>123` | ancestors | indexed TCT lookup, or recursive CTE up `concept_isa` |
| `>>123` | ancestors or self | ancestors ∪ `{123}` |
| `<!123` | children (direct) | `concept_isa` one hop down |
| `>!123` | parents (direct) | `concept_isa` one hop up |
| `^123` | members of refset 123 | `refset_members` |

Concept references may carry an optional `\|term\|` label, which is parsed and ignored for evaluation (it is a human annotation): `73211009 |Diabetes mellitus|`.

Boolean composition, left-associative, parentheses for grouping:

```
<<73211009 OR <<840539006
<<73211009 MINUS <<46635009
(<<73211009 OR <<840539006) MINUS <<199223000
```

Refinement (attribute constraints) on a focus, comma = conjunction:

```
<<404684003 : 363698007 = <<39057004
<<373873005 : 363698007 = <<57809008 , 411116001 = <<385268001
<<404684003 : { 363698007 = <<39057004 }
```

The attribute *name* and *value* are themselves expressions (`363698007`, `<<39057004`, `*`).

History supplements (ECL 2.0), which add the inactive concepts historically associated with the result set:

```
<<195967001 {{ + HISTORY-MIN }}
<<195967001 {{ + HISTORY-MOD }}
(<<73211009 OR <<840539006) {{ + HISTORY-MAX }}
<<195967001 {{ + HISTORY (900000000000527005) }}
```

Why they matter: inactivating a concept strips its parents and attributes, so it belongs to no `<<X` set. Six-year-old coded data therefore goes *silently* unmatched - no error, just fewer rows. A supplement follows the historical association reference sets back from the active result set to the retired concepts that point at it.

The three profiles are the ECL specification's (§6.11), and differ only in which reference sets they follow:

| Profile | Reference sets | Use |
|---|---|---|
| `HISTORY-MIN` | `SAME AS` | Highest precision - one-to-one equivalence only (e.g. decision support) |
| `HISTORY-MOD` | `SAME AS`, `REPLACED BY`, `WAS A`, `PARTIALLY EQUIVALENT TO` | Balanced (e.g. research, audit) |
| `HISTORY-MAX`, `HISTORY (*)`, bare `HISTORY` | All of `<900000000000522004` | Highest recall (e.g. case-finding for manual review) |

Notes on the implementation:

- A supplement binds to the **nearest preceding focus**, like a refinement. Parenthesise to cover a whole expression: `(A OR B) {{ + HISTORY }}`, not `A OR B {{ + HISTORY }}`.
- `HISTORY-MAX` reads the live `<900000000000522004 |Historical association reference set|` hierarchy as well as the built-in list, so a reference set added by a future release is followed rather than silently skipped.
- Reference sets are matched by SCTID *and* by the humanised name `concept_history.association` stores, so a database built by an older `sct` still matches.
- The specification's prose mentions `MOVED FROM` under `HISTORY-MIN` while its normative table does not; we follow the table. `MOVED FROM` reverses the usual direction (the *target* is the inactive concept), which the specification's own expansion template cannot express either.
- The supplement deliberately returns **inactive** concepts, so its results are outside the active substrate the rest of ECL works over. `sct serve`'s `$expand` filters to active concepts by default; pass `activeOnly=false` to see supplemented results.
- Requires the `concept_history` table, which needs `sct ndjson --refsets all` (the default `simple` mode excludes Association reference set files). Its absence is an error, not an empty result.

**Deferred (clear "unsupported ECL construct" error, not silent mis-evaluation):** cardinality `[1..*]`, reverse attributes `R`, dotted attributes `.`, attribute-group cardinality semantics (groups parse but are treated as a flat conjunction in v1 - documented approximation), nested member-of in values beyond one level, and the other `{{ … }}` filters (description, member, and concept filters).

---

## 6. Evaluation semantics

- **Concept** `123` → `{123}`.
- **Wildcard** `*` → all active concept ids (`SELECT id FROM concepts WHERE active = 1`).
- **Hierarchy** - indexed `concept_ancestors` lookup when the TCT is usable, otherwise a recursive CTE over `concept_isa`; `<<`/`>>` add the focus itself.
- **MemberOf** `^X` → `SELECT referenced_component_id FROM refset_members WHERE refset_id = X`.
- **AND/OR/MINUS** → set intersection / union / difference.
- **Refinement** `focus : attr = value`:
  1. evaluate `focus`, `attr` (a set of type SCTIDs - usually one), `value` (a set of destination SCTIDs).
  2. `SELECT DISTINCT source_id FROM concept_relationships WHERE type_id IN (attr)` then keep rows whose `destination_id ∈ value`; the surviving `source_id`s, intersected with `focus`, are the result.
  3. multiple comma-separated constraints intersect.
  4. `!=` negates the value test. Attribute groups `{…}` are evaluated as a flat conjunction in v1 (group-cardinality is deferred).

`attr` sets are small (typically one type), keeping the `IN` list bounded; value membership is tested in Rust. This is the pragmatic v1; the scale path is whole-AST SQL compilation.

---

## 7. Integration: `sct codelist add --ecl`

```
sct codelist add my.codelist --ecl "<<73211009" [--db <db>] [--comment "..."]
```

`--ecl <expr>` is mutually exclusive with positional SCTIDs. It parses and evaluates the expression against the database, then adds each resulting active concept exactly as the SCTID path does (dedup against existing, preferred-term lookup, version bump). A summary reports how many the expression matched and how many were newly added.

The same engine backs `sct serve`'s `ValueSet/$expand` and is the planned compile target for SCT-QL.

---

## 8. Testing

- **Parser unit tests** - every operator, optional `|term|`, booleans, parens, refinement; and that deferred constructs produce a clear error.
- **Evaluator integration tests** - build a small synthetic SQLite DB (a hand-made hierarchy + a refset + a few typed relationships) and assert exact result sets for `<<`, `<`, `>>`, `^`, `AND`/`OR`/`MINUS`, and an attribute refinement.
- **Pipeline test** - an NDJSON fixture with `relationships` round-trips through `sct sqlite` into `concept_relationships`.
- CI gates unchanged: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, full test suite.

---

## 9. Sequencing

Slice 1 (this work): the data-pipeline change (schema v4 + `concept_relationships`), the `src/ecl/` engine (lex + parse + eval) for the §5 grammar, and the `codelist add --ecl` wiring, with tests.

Later: cardinality and grouped semantics; reverse/dotted attributes; whole-AST SQL compilation for scale; SCT-QL → ECL lowering; ECL *output* (compile SCT-QL or codelists *to* ECL text for interoperability).

---

## 10. References

- SNOMED International, [Expression Constraint Language - Specification and Guide](https://confluence.ihtsdotools.org/display/DOCECL).
- `spec/sct-ql-spec.md` - the friendly language that compiles to this ECL engine.
- `spec/commands/serve.md` - the FHIR surface that consumes this engine.
