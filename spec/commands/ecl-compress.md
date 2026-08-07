# `sct ecl compress` - refactor a concept set into minimal ECL

**Status:** ✅ Shipped (slice 1: include roots + clean exclusions + exactness residual net + verification + `--stats` + `--codelist` input; slice 3's `sct codelist export --format ecl` wiring). Deferred: straddling-exclusion push-down (§4.2 step 4-5), `^refset` cover clauses, and the FHIR `ValueSet.compose` emitter in `sct serve` (§7 slices 2-3). Anticipated by `spec/ecl.md §9`.
**Scope:** Given an explicit set of SCTIDs (a codelist), synthesise a compact **ECL expression** that expands to *exactly* that set - the inverse of `ValueSet/$expand`. Runs against the local `sct` SQLite database.
**Audience:** A coding agent (and Marcus) implementing this in the `sct` repo.
**Provenance:** Idea harvested from the prior-art Python toolchain `cheethame2017/sct` (Apache-2.0), whose ECL/set browser can "refactor" a raw set of concepts into include/exclude constraints. Clean-room Rust reimplementation - ideas only, no code.

---

## 1. Why

A hand-curated codelist is an **extensional** artefact: a flat list of ids. It is brittle - every SNOMED release that adds a child under a concept the list "meant to include" silently drifts out of date, and a reviewer cannot see the clinical *intent* behind 400 opaque SCTIDs. An **intensional** definition (`<<73211009 MINUS <<46635009`) is the opposite: compact, self-documenting, and release-stable (it re-expands to include new children automatically).

`sct ecl expand` already goes intensional → extensional. `sct ecl compress` closes the loop:

- **Codelist maintainability.** Turn a legacy flat list back into the ECL that (approximately or exactly) produces it, so it can be reviewed, version-stabilised, and maintained as intent rather than as ids.
- **Interoperability.** Emit a FHIR `ValueSet.compose` / ECL string from any `.codelist`, feeding terminology servers and other tools that speak ECL.
- **Distinctive.** No local, offline tool does set → minimal-ECL. It pairs directly with the shipped `.codelist` and ECL engine.

Honest framing: the *minimum-size* intensional cover of an arbitrary set is a hard combinatorial problem. This command ships a good **greedy heuristic** plus an **exactness guarantee** (§4.3), not a proof of global minimality. It says so in its output.

---

## 2. CLI surface

```
sct ecl compress [<ids>...] [--codelist FILE] [--exact|--intensional-only]
                 [--max-exclusions N] [--pretty] [--no-verify] [--stats]
                 [--db PATH]
```

Input (exactly one source):
- `<ids>...` - SCTIDs as positional args, or `-` to read newline-delimited ids from stdin (so `sct ecl expand … | sct ecl compress -` round-trips).
- `--codelist FILE` - resolve a `.codelist` (composition included) and compress its members.

Behaviour:
- `--exact` (**default**) - guarantee the emitted ECL re-expands to *exactly* the input set, appending literal residual terms (`OR 12345`, `MINUS 67890`) where the subsumption heuristic cannot express the set intensionally. Always correct.
- `--intensional-only` - emit only subsumption/refset clauses; if that cannot reproduce the set exactly, **do not** add literal residuals - instead print the best-effort expression to stdout and a precise diff (missing / extra ids) to stderr, exiting non-zero. For users who want "the closest clean intent" and will hand-finish.
- `--max-exclusions N` (default e.g. 32) - complexity bound. Beyond `N` exclusion clauses for a given include root, stop refining and fall back to literal residuals (`--exact`) or report residual (`--intensional-only`). Prevents pathological blow-up on adversarial sets.
- `--pretty` - multi-line, indented formatting; default is a single line.
- `--no-verify` - skip the re-expansion check (§4.3). Default is to verify; verification is cheap and catches heuristic bugs.
- `--stats` - print a compression report to stderr: `input 412 ids → 3 include + 5 exclude clauses (+2 literal residuals); intensional coverage 99.5%`.
- `--db PATH` - database; omitted → standard discovery (`docs/path-resolution.md`).

stdout is the ECL string (clean, pipeable into `sct codelist`, a FHIR resource, etc.); all diagnostics go to stderr.

---

## 3. Data substrate

Compression is pure set/subsumption arithmetic over the IS-A graph - it needs **no** definition-status or attribute data, so it works on any current `sct` database.

| Operation | Backing data |
|---|---|
| `descendants-or-self(c)` | `concept_isa` recursive CTE; **fast path** via `concept_ancestors` (TCT) when its completion marker, schema, indexes, and invalidation triggers are valid |
| `ancestors(c)` (to find maximal/minimal elements) | same |
| membership / set algebra | Rust `BTreeSet<u64>` |
| PT for `\|term\|` annotation of emitted ids | `concepts.preferred_term` |

The TCT (`--transitive-closure`) turns the hot `descendants-or-self` call into an indexed lookup; strongly recommended for large inputs but not required. Reuse the exact traversal helpers the ECL evaluator already uses (`src/ecl/eval.rs`) so `compress` and `expand` share one definition of subsumption.

---

## 4. Algorithm

Let `S` be the input set (active concepts; inactive ids are reported and dropped, consistent with `expand`). Everything below is over the IS-A closure.

### 4.1 Include roots (cover S from above)

1. Compute the **maximal elements** of `S`: concepts in `S` that have no proper ancestor in `S`. Each becomes a candidate `<<root` clause. This is the natural, human-meaningful cover ("diabetes and all its subtypes").
2. The union of `descendants-or-self(root)` over those roots is `Cover ⊇ S`. The **over-inclusion** is `E = Cover \ S` - concepts the intensional roots pull in that the user did *not* list.

### 4.2 Exclusions (carve E back out)

3. Compute the maximal elements of `E`. For each such `x`, a `MINUS <<x` clause removes `descendants-or-self(x)` - **but only if** that subtree contains no member of `S` (i.e. `descendants-or-self(x) ∩ S = ∅`); otherwise the exclusion would wrongly delete wanted concepts.
4. Exclusions whose subtree straddles `S` and `E` are the hard case. Options, applied in order until `E` is covered or `--max-exclusions` is hit:
   a. push the exclusion *down* to the maximal elements of `E` strictly below `x` that are clean (subtree disjoint from `S`);
   b. for the residual straddling concepts, fall through to §4.3.
5. Recurse only as needed: an exclusion may itself over-remove, requiring an `OR`-back of a wanted subtree. Bound total recursion depth; this is a heuristic, not an exhaustive search.

### 4.3 Exactness guarantee (the safety net)

6. **Re-expand** the candidate expression with the real ECL evaluator (unless `--no-verify`) and diff against `S`:
   - `missing = S \ Result` → append `OR <id>` literals (or `OR <<id>` if a clean subtree exists).
   - `extra = Result \ S` → append `MINUS <id>` literals.
7. Under `--exact` (default) these literal residuals make the final expression **provably exact** - the command re-verifies once more and only then prints. Under `--intensional-only`, step 6 is a *report*, not a fixup: print the intensional expression and the diff, exit non-zero.

The residual mechanism means correctness never depends on the heuristic's cleverness - the heuristic only determines how *compact* the result is. A set with no intensional structure at all degrades gracefully to `id1 OR id2 OR …` (i.e. the input), never to a *wrong* answer.

### 4.4 Output shaping

8. Order clauses largest-cover-first; annotate roots with `|PT|` when a `--terms` style is on (default: ids only, for compactness). `--pretty` breaks one clause per line:

```
<< 73211009 |Diabetes mellitus|
  MINUS << 46635009 |Type 1 diabetes mellitus due to autoimmune process|
  MINUS 199223000
  OR 11530004
```

---

## 5. Worked example

Input: every subtype of *Diabetes mellitus* (`<<73211009`) **except** the *Type 1* subtree, plus one unrelated concept.

- Maximal element of `S` → `73211009`, giving `<<73211009`.
- `E = descendants-or-self(73211009) \ S` = the Type 1 subtree.
- Maximal clean element of `E` → `46635009`; its subtree is disjoint from `S` → `MINUS <<46635009`.
- Verify: `<<73211009 MINUS <<46635009` re-expands; diff shows the one unrelated concept `11530004` missing → append `OR 11530004`.
- Final: `<<73211009 MINUS <<46635009 OR 11530004`. `--stats`: `— input 210 ids → 1 include + 1 exclude + 1 literal; intensional coverage 99.5%`.

---

## 6. Testing

- **Unit tests over a synthetic hierarchy** with known structure:
  - a pure subtree compresses to a single `<<root` (no residuals);
  - subtree-minus-subtree compresses to `<< MINUS <<`;
  - a straddling exclusion falls back correctly and stays **exact**;
  - a structureless set degrades to `OR`-of-literals equal to the input.
- **Round-trip property test** - for random subsets of the fixture, `expand(compress(S)) == S` must hold for `--exact` (this is the core invariant; assert it always).
- **`--intensional-only`** - emits the intensional form, reports the correct diff, exits non-zero when not exact.
- **`--max-exclusions`** - bound is respected; residuals absorb the remainder.
- **Real-DB smoke** (ignored-by-default, needs a built DB) - compress a real published refset and assert exact round-trip + a sane compression ratio.
- CI gates unchanged: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, full suite.

---

## 7. Sequencing

1. **Slice 1** - includes + clean exclusions (§4.1-4.2 steps 1-3) + the exactness residual net (§4.3) + verification + `--stats`. Correct and useful from day one; compact on the common "subtree minus subtrees" shape.
2. **Slice 2** - straddling-exclusion push-down and bounded recursion (§4.2 step 4-5) for tighter expressions; `--pretty`, `|PT|` annotation.
3. **Slice 3** - `^refset` clauses in the cover (recognise when `S` *is* a refset's membership and emit `^refsetId`); the FHIR `ValueSet.compose` emitter in `sct serve`. ✅ The `sct codelist export --format ecl` half of this slice is shipped: it reuses `compress_with_tct` unchanged, so it inherits the exactness guarantee and gets the `^refset` clauses "for free" the day step 3a lands its cover recognition.
4. **Later** - MCP `snomed_compress` tool; attribute-refinement clauses in the cover (using `concept_relationships`) when a set aligns with an attribute value, e.g. `<<404684003 : 363698007 = <<39057004`.

---

## 8. References

- `spec/ecl.md` - the ECL engine (`expand`) this inverts and reuses for verification; §9 anticipates this "ECL output" work.
- `spec/roadmap.md` - "Ideas harvested from prior-art Python tooling" (source of this item) and the `.codelist` / `sct serve` consumers.
- SNOMED International, [ECL Specification and Guide](https://confluence.ihtsdotools.org/display/DOCECL).
- Prior art: `cheethame2017/sct` - <https://bitbucket.org/cheethame2017/sct/src/development/>.
