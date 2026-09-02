// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Abstract syntax tree for the supported subset of SNOMED CT Expression
//! Constraint Language (ECL). See `spec/ecl.md` §5.

/// A focus operator applied to a sub-expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `<` - descendants (not self)
    DescendantOf,
    /// `<<` - descendants or self
    DescendantOrSelfOf,
    /// `>` - ancestors (not self)
    AncestorOf,
    /// `>>` - ancestors or self
    AncestorOrSelfOf,
    /// `<!` - direct children
    ChildOf,
    /// `>!` - direct parents
    ParentOf,
    /// `^` - members of the refset
    MemberOf,
}

/// Boolean combination of two expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    /// `AND` - intersection
    And,
    /// `OR` - union
    Or,
    /// `MINUS` - set difference
    Minus,
}

/// An ECL expression constraint.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// `*` - any concept.
    Wildcard,
    /// A concept reference by SCTID. Any `|term|` annotation is parsed and
    /// dropped (it is a human label, not part of the semantics).
    Concept(String),
    /// A focus operator applied to a sub-expression.
    Op(Op, Box<Expr>),
    /// A boolean combination.
    Bool(BoolOp, Box<Expr>, Box<Expr>),
    /// A focus expression refined by attribute constraints (`focus : refinement`).
    Refined(Box<Expr>, Refinement),
    /// A sub-expression supplemented with the inactive concepts historically
    /// associated with its members (`expr {{ + HISTORY-MOD }}`).
    History(Box<Expr>, History),
}

/// Which historical association reference sets a `{{ + HISTORY }}` supplement
/// draws on. The three named profiles are defined by the ECL specification
/// (§6.11 History Supplements); see [`History::refsets`] for the reference set
/// each one names.
#[derive(Debug, Clone, PartialEq)]
pub enum History {
    /// `HISTORY-MIN` - `SAME AS` only: one-to-one equivalence, highest precision.
    Min,
    /// `HISTORY-MOD` - `SAME AS`, `REPLACED BY`, `WAS A`, `PARTIALLY EQUIVALENT TO`.
    Mod,
    /// `HISTORY-MAX`, `HISTORY (*)`, and a bare `HISTORY` - every historical
    /// association reference set, for maximum recall.
    Max,
    /// `HISTORY ( <expr> )` - the reference sets the inner expression selects.
    Refsets(Box<Expr>),
}

/// `900000000000527005 |SAME AS association reference set|`.
pub const ASSOC_SAME_AS: &str = "900000000000527005";
/// `900000000000526001 |REPLACED BY association reference set|`.
pub const ASSOC_REPLACED_BY: &str = "900000000000526001";
/// `900000000000528000 |WAS A association reference set|`.
pub const ASSOC_WAS_A: &str = "900000000000528000";
/// `1186924009 |PARTIALLY EQUIVALENT TO association reference set|`.
pub const ASSOC_PARTIALLY_EQUIVALENT_TO: &str = "1186924009";
/// `900000000000522004 |Historical association reference set|` - the parent
/// whose descendants are the `HISTORY-MAX` set.
pub const ASSOC_HISTORICAL_ROOT: &str = "900000000000522004";

impl History {
    /// The historical association reference sets this profile draws on, as
    /// SCTIDs. `Max` returns the reference sets known to descend from
    /// [`ASSOC_HISTORICAL_ROOT`]; the evaluator additionally reads the live
    /// hierarchy so a release that adds one is not silently missed.
    /// `Refsets` returns `None` - its reference sets come from evaluating the
    /// inner expression.
    pub fn refsets(&self) -> Option<&'static [&'static str]> {
        match self {
            Self::Min => Some(&[ASSOC_SAME_AS]),
            Self::Mod => Some(&[
                ASSOC_SAME_AS,
                ASSOC_REPLACED_BY,
                ASSOC_WAS_A,
                ASSOC_PARTIALLY_EQUIVALENT_TO,
            ]),
            // Every child of |Historical association reference set| in the
            // 2026 International release. `MOVED TO` is included for
            // completeness even though the specification notes it can be
            // ignored: its targetComponentId is a namespace concept, so it
            // never matches a concept in a result set.
            Self::Max => Some(&[
                "900000000000523009", // POSSIBLY EQUIVALENT TO
                "900000000000524003", // MOVED TO
                "900000000000525002", // MOVED FROM
                ASSOC_REPLACED_BY,
                ASSOC_SAME_AS,
                ASSOC_WAS_A,
                "900000000000529008", // SIMILAR TO
                "900000000000530003", // ALTERNATIVE
                "900000000000531004", // REFERS TO
                ASSOC_PARTIALLY_EQUIVALENT_TO,
                "1186921001", // POSSIBLY REPLACED BY
            ]),
            Self::Refsets(_) => None,
        }
    }
}

/// The attribute-constraint portion after a `:`.
#[derive(Debug, Clone, PartialEq)]
pub enum Refinement {
    /// A single attribute constraint: `attr = value` (or `!=` when `negate`).
    Attr {
        /// Attribute *type* expression (usually a concept, e.g. `363698007`).
        attr: Box<Expr>,
        /// `true` for `!=`, `false` for `=`.
        negate: bool,
        /// Attribute *value* expression (e.g. `<<80891009`, `*`).
        value: Box<Expr>,
    },
    /// Conjunction (comma or `AND`).
    And(Box<Refinement>, Box<Refinement>),
    /// Disjunction (`OR`).
    Or(Box<Refinement>, Box<Refinement>),
    /// An attribute group `{ … }`. Evaluated as a flat conjunction in v1
    /// (group cardinality is deferred - see `spec/ecl.md` §5).
    Group(Box<Refinement>),
}
