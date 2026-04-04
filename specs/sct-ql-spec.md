# SCT Query Language - Specification and Design Notes

*Written for someone comfortable with lexers, parsers and compilers, but new to SNOMED CT.*

---

## Background: what is SNOMED CT?

SNOMED CT (Systematised Nomenclature of Medicine - Clinical Terms) is the world's largest clinical terminology system. It contains around 350,000-850,000 active concepts (depending on edition) representing clinical ideas - diseases, procedures, body structures, drugs, organisms, and so on.

Each concept has:

- A unique numeric identifier (SCTID) - e.g. `22298006`
- A Fully Specified Name (FSN) - e.g. `Myocardial infarction (disorder)`
- A preferred term - e.g. `Myocardial infarction`
- Multiple synonyms - e.g. `Heart attack`, `Cardiac infarction`

Concepts are connected by relationships. The most important is IS-A, forming a strict hierarchy (actually a DAG - a concept can have multiple parents). Other relationships are typed attributes, e.g.:

```
Myocardial infarction
  IS-A: Ischemic heart disease
  IS-A: Myocardial necrosis
  Finding site: Myocardium structure
  Associated morphology: Infarct
```

The hierarchy is deep - typically 12-15 levels from root to leaf. The root is "SNOMED CT concept" and everything descends from it through top-level hierarchies like "Clinical finding", "Procedure", "Substance", etc.

---

## The problem: ECL

SNOMED International defined a query language called ECL - Expression Constraint Language. It allows you to express queries over the concept hierarchy and relationships.

ECL is syntactically terse to the point of hostility. Some examples:

```ecl
# All descendants of Myocardial infarction (including itself)
<<22298006

# All descendants of Myocardial infarction (excluding itself)
<22298006

# All ancestors of Myocardial infarction (including itself)
>>22298006

# Myocardial infarction with a specific finding site attribute
22298006 : 363698007 = <<80891009

# Descendants of Pharmaceutical product
# where the finding site is a descendant of Cardiovascular finding
<<373873005 : 363698007 = <<57809008

# Union
<<22298006 OR <<57054005

# Exclusion
<<22298006 MINUS <<57054005
```

The operators `<<`, `>>`, `<`, `>` mean "descendants including self", "ancestors including self", "descendants excluding self", "ancestors excluding self". The `:` separates a concept expression from its attribute refinements. The `=` constrains an attribute value.

ECL is powerful and expressive. It is also:

- Completely opaque to anyone who hasn't been trained in it
- Full of punctuation that carries dense semantic meaning
- Defined by a 100+ page specification document
- The source of endless confusion in the clinical informatics community

The analogy is assembler: precise, powerful, and written for specialists rather than humans.

---

## The proposed solution: a friendly query language

We want to design a high-level query language - call it **SCT-QL** for now - that:

1. Compiles to ECL for interoperability with existing SNOMED tooling
2. Compiles to SQL recursive CTEs for local execution against a SQLite database
3. Is readable by a clinician or researcher who has never seen ECL
4. Covers ~90% of real-world use cases without requiring ECL knowledge

The Python/assembler analogy is exact: SCT-QL is Python, ECL is assembler, SQL is the bytecode we actually execute locally.

---

## SCT-QL syntax - proposed grammar

### Concept references

Concepts can be referenced by preferred term (quoted string) or SCTID (bare integer):

```
"Myocardial infarction"
22298006
```

Both are valid anywhere a concept is expected. The compiler resolves quoted strings to SCTIDs via the local database at compile time.

### Hierarchy traversal

```
descendants of "Myocardial infarction"
descendants of "Myocardial infarction" including self
ancestors of "Myocardial infarction"
ancestors of "Myocardial infarction" including self
children of "Myocardial infarction"
parents of "Myocardial infarction"
```

These map directly to ECL operators:

| SCT-QL | ECL |
|---|---|
| `descendants of X` | `<X` |
| `descendants of X including self` | `<<X` |
| `ancestors of X` | `>X` |
| `ancestors of X including self` | `>>X` |
| `children of X` | `<!X` |
| `parents of X` | `>!X` |

### Attribute constraints

```
descendants of "Pharmaceutical product"
  where finding-site is descendant of "Cardiovascular finding"
```

The `where` clause constrains by relationship attribute. Attribute names are human-readable aliases for SCTIDs:

| Alias | Attribute SCTID | Meaning |
|---|---|---|
| `finding-site` | `363698007` | anatomical location of a finding |
| `associated-morphology` | `116676008` | structural change |
| `has-active-ingredient` | `127489000` | drug ingredient |
| `causative-agent` | `246075003` | cause of a disorder |
| `method` | `260686004` | technique used in a procedure |
| `procedure-site` | `363704007` | site of a procedure |

The attribute alias table is extensible - new aliases can be added without changing the grammar.

The right-hand side of a `where` clause is itself a concept expression:

```
where finding-site is "Myocardium structure"          -- exact match
where finding-site is descendant of "Heart structure"  -- subsumption
where finding-site is any                              -- attribute exists, any value
```

### Boolean operations

```
descendants of "Type 1 diabetes" or descendants of "Type 2 diabetes"

descendants of "Asthma" excluding descendants of "Occupational asthma"

(descendants of "Type 1 diabetes" or descendants of "Type 2 diabetes")
  where finding-site is descendant of "Endocrine structure"
```

`or`, `and`, `excluding` map to ECL `OR`, `AND`, `MINUS`.

### Full example

```
-- Drugs used in cardiovascular conditions
-- (the 'gnarly' query in terminology server circles)

descendants of "Pharmaceutical product"
  where has-active-ingredient is any
  and finding-site is descendant of "Cardiovascular finding"
  excluding descendants of "Homeopathic preparation"
```

ECL equivalent:

```ecl
<<373873005|Pharmaceutical product| :
  127489000|Has active ingredient| = *,
  363698007|Finding site| = <<57809008|Cardiovascular finding|
MINUS <<1156326007|Homeopathic preparation|
```

The SCT-QL version is unambiguously more readable. The ECL version is what gets sent to a terminology server or written into a codelist definition for interoperability.

---

## Compilation pipeline

```
SCT-QL source
     │
     ▼
  Lexer
     │  tokens
     ▼
  Parser  ◄── concept name resolver (SQLite lookup at parse time)
     │
     ▼
   AST
     │
     ├──▶ ECL emitter     →  ECL string (for interoperability)
     │
     └──▶ SQL emitter     →  recursive CTE query (for local execution)
```

### Lexer tokens

```
DESCENDANTS | ANCESTORS | CHILDREN | PARENTS
OF | INCLUDING | SELF
WHERE | IS | ANY
AND | OR | EXCLUDING
LPAREN | RPAREN
STRING    -- quoted concept name: "Myocardial infarction"
INTEGER   -- bare SCTID: 22298006
IDENT     -- attribute alias: finding-site, has-active-ingredient
```

### AST nodes

```rust
enum Expr {
    ConceptRef(ConceptRef),
    Descendants { of: Box<ConceptRef>, including_self: bool },
    Ancestors   { of: Box<ConceptRef>, including_self: bool },
    Children    { of: Box<ConceptRef> },
    Parents     { of: Box<ConceptRef> },
    Refined     { base: Box<Expr>, constraints: Vec<Constraint> },
    Union       { left: Box<Expr>, right: Box<Expr> },
    Exclusion   { left: Box<Expr>, right: Box<Expr> },
}

enum ConceptRef {
    BySctid(u64),
    ByName(String),   // resolved to SCTID during parsing
}

struct Constraint {
    attribute: AttributeRef,   // alias or SCTID
    value: ConstraintValue,
}

enum ConstraintValue {
    Any,
    Exact(ConceptRef),
    Descendant { of: ConceptRef, including_self: bool },
}
```

### SQL emitter

The SQL emitter walks the AST and produces a recursive CTE. Each `Descendants` node becomes a recursive CTE block. Multiple CTEs are composed with JOINs or UNION/EXCEPT for boolean operations.

```sql
-- descendants of "Myocardial infarction"
WITH RECURSIVE descendants(id) AS (
  SELECT DISTINCT child_id
  FROM concept_isa
  WHERE parent_id = '22298006'    -- resolved from name at compile time

  UNION

  SELECT ci.child_id
  FROM concept_isa ci
  JOIN descendants d ON ci.parent_id = d.id
)
SELECT c.id, c.preferred_term, c.fsn
FROM concepts c
JOIN descendants d ON c.id = d.id
```

Attribute constraints become JOINs against the relationships table:

```sql
-- where finding-site is descendant of "Cardiovascular finding"
JOIN concept_relationships r
  ON r.source_id = c.id
  AND r.type_id = '363698007'    -- finding-site SCTID
  AND r.destination_id IN (SELECT id FROM cardio_findings_cte)
```

### ECL emitter

The ECL emitter is simpler - just a tree walk producing a string:

```rust
fn emit_ecl(expr: &Expr) -> String {
    match expr {
        Expr::Descendants { of, including_self: true  } => format!("<<{}", emit_ref(of)),
        Expr::Descendants { of, including_self: false } => format!("<{}",  emit_ref(of)),
        Expr::Union { left, right } => format!("({} OR {})", emit_ecl(left), emit_ecl(right)),
        Expr::Exclusion { left, right } => format!("({} MINUS {})", emit_ecl(left), emit_ecl(right)),
        Expr::Refined { base, constraints } => format!("{} : {}",
            emit_ecl(base),
            constraints.iter().map(emit_constraint_ecl).collect::<Vec<_>>().join(", ")),
        // ...
    }
}
```

---

## The concept name resolver

The interesting compile-time step: resolving `"Myocardial infarction"` to `22298006`.

This is a SQLite FTS5 query against the local `sct` database at parse time:

```sql
SELECT id FROM concepts_fts
WHERE concepts_fts MATCH 'Myocardial infarction'
AND rank = 1   -- top result
LIMIT 1
```

If the name is ambiguous (multiple concepts match), the compiler should error with suggestions:

```
error: ambiguous concept name "diabetes"
  did you mean:
    "Diabetes mellitus" [73211009]
    "Diabetes insipidus" [15771004]
    "Gestational diabetes mellitus" [11687002]
  use the SCTID directly to disambiguate, or quote the full preferred term
```

This is a nice compiler error UX pattern - the same approach as Rust's "did you mean?" suggestions.

---

## Parser recommendation

**`pest`** (PEG parser generator for Rust) is the recommended implementation path. The grammar file is readable, the generated parser is fast, and error messages are reasonable out of the box.

The full SCT-QL grammar in PEG notation is probably 80-120 rules - a weekend project for someone already comfortable with parser combinators. `nom` would also work but the grammar would be less readable as documentation.

The grammar is intentionally simple - no operator precedence puzzles, no ambiguity, no left recursion. `and`/`or`/`excluding` are all left-associative at the same precedence level, with parentheses for grouping. This is a deliberate design choice: clinical users should not have to think about operator precedence.

---

## Scope and non-goals for v1

**In scope:**

- IS-A hierarchy traversal (descendants, ancestors, children, parents)
- Single-level attribute constraints with `where`
- Boolean composition (or, and, excluding)
- Concept reference by name or SCTID
- ECL output
- SQL output

**Out of scope for v1 (but valid future work):**

- Nested attribute groups (ECL refinement groups with `{ }`)
- Concrete domain constraints (numeric values, e.g. `> 5mg`)
- Reverse attributes (finding concepts from a site)
- Full MRCM (Machine Readable Concept Model) validation
- Any-role-group semantics

The out-of-scope items cover roughly the hardest 10% of ECL and the rarest 10% of real-world queries. A v1 that covers the grammar above handles the vast majority of codelist construction, phenotyping, and clinical query use cases.

---

## Why this is worth building

ECL is the only standardised query language for SNOMED CT. Every terminology server speaks it. Every codelist specification uses it. It is not going away.

But the barrier to writing correct ECL is high enough that most clinicians and researchers either avoid it entirely, rely on GUI tools that generate it invisibly, or write it wrong and don't notice.

A friendly syntax that compiles to correct ECL lowers that barrier to zero. Write in plain English, get correct ECL out. Use the ECL output in any SNOMED-compatible system. The compiler does the hard part.

The `--explain` flag would show both the generated ECL and the generated SQL side by side - making it a learning tool as well as a productivity tool. Write SCT-QL, see the ECL it produces, gradually learn the underlying language if you want to.

This has never been built as a standalone open source tool. It would be a genuine contribution to the clinical informatics ecosystem independent of everything else `sct` does.
