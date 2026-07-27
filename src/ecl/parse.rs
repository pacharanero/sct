// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Recursive-descent parser for the supported ECL subset. See `spec/ecl.md` §5.
//!
//! Precedence, loosest to tightest: `OR`, `AND`, `MINUS`, then a refined focus
//! (`subExpr : refinement`), then the focus atom. This is intentionally a little
//! more permissive than strict ECL (which forbids mixing `AND`/`OR`/`MINUS`
//! without parentheses); parenthesise when mixing to be explicit.

use anyhow::{bail, Result};

use crate::ecl::ast::{BoolOp, Expr, Op, Refinement};
use crate::ecl::lex::{lex, Spanned, Tok};

/// Maximum nesting depth for parenthesised sub-expressions and attribute
/// groups. Without a cap, a pathological input (e.g. thousands of nested
/// parens) drives the recursive-descent parser to a stack overflow, which
/// aborts the process - and via `sct serve`/MCP, the whole long-running
/// server. 200 comfortably covers any real ECL expression.
const MAX_DEPTH: usize = 200;

/// Parse an ECL expression into an [`Expr`].
pub fn parse(input: &str) -> Result<Expr> {
    let tokens = lex(input)?;
    if tokens.is_empty() {
        bail!("empty ECL expression");
    }
    let mut p = Parser {
        tokens,
        idx: 0,
        depth: 0,
    };
    let e = p.parse_or()?;
    if let Some(s) = p.peek() {
        bail!(
            "unexpected trailing token after expression at position {}",
            s.pos
        );
    }
    Ok(e)
}

struct Parser {
    tokens: Vec<Spanned>,
    idx: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Spanned> {
        self.tokens.get(self.idx)
    }
    fn peek_tok(&self) -> Option<&Tok> {
        self.tokens.get(self.idx).map(|s| &s.tok)
    }
    fn next(&mut self) -> Option<Spanned> {
        let t = self.tokens.get(self.idx).cloned();
        if t.is_some() {
            self.idx += 1;
        }
        t
    }
    fn eat(&mut self, want: &Tok) -> bool {
        if self.peek_tok() == Some(want) {
            self.idx += 1;
            true
        } else {
            false
        }
    }
    fn pos_hint(&self) -> String {
        match self.peek() {
            Some(s) => format!("at position {}", s.pos),
            None => "at end of expression".to_string(),
        }
    }
    /// Enter one level of parenthesis/group nesting, rejecting expressions
    /// that would otherwise recurse deep enough to overflow the stack.
    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            bail!("ECL expression nested too deeply (max depth {MAX_DEPTH})");
        }
        Ok(())
    }
    fn exit(&mut self) {
        self.depth -= 1;
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while self.eat(&Tok::Or) {
            let right = self.parse_and()?;
            left = Expr::Bool(BoolOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_minus()?;
        while self.eat(&Tok::And) {
            let right = self.parse_minus()?;
            left = Expr::Bool(BoolOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_minus(&mut self) -> Result<Expr> {
        let mut left = self.parse_refined()?;
        while self.eat(&Tok::Minus) {
            let right = self.parse_refined()?;
            left = Expr::Bool(BoolOp::Minus, Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_refined(&mut self) -> Result<Expr> {
        let focus = self.parse_sub()?;
        if self.eat(&Tok::Colon) {
            let refinement = self.parse_refinement()?;
            Ok(Expr::Refined(Box::new(focus), refinement))
        } else {
            Ok(focus)
        }
    }
    /// A sub-expression: a parenthesised expression, or a focus.
    fn parse_sub(&mut self) -> Result<Expr> {
        if self.eat(&Tok::LParen) {
            self.enter()?;
            let e = self.parse_or();
            self.exit();
            let e = e?;
            if !self.eat(&Tok::RParen) {
                bail!("expected ')' {}", self.pos_hint());
            }
            return Ok(e);
        }
        self.parse_focus()
    }
    /// An optional focus operator applied to an atom or parenthesised expression.
    fn parse_focus(&mut self) -> Result<Expr> {
        let op = match self.peek_tok() {
            Some(Tok::DescOrSelf) => Some(Op::DescendantOrSelfOf),
            Some(Tok::Desc) => Some(Op::DescendantOf),
            Some(Tok::AncOrSelf) => Some(Op::AncestorOrSelfOf),
            Some(Tok::Anc) => Some(Op::AncestorOf),
            Some(Tok::Child) => Some(Op::ChildOf),
            Some(Tok::Parent) => Some(Op::ParentOf),
            Some(Tok::Member) => Some(Op::MemberOf),
            _ => None,
        };
        if op.is_some() {
            self.idx += 1;
        }
        let operand = if self.peek_tok() == Some(&Tok::LParen) {
            self.parse_sub()?
        } else {
            self.parse_atom()?
        };
        match op {
            Some(o) => Ok(Expr::Op(o, Box::new(operand))),
            None => Ok(operand),
        }
    }
    fn parse_atom(&mut self) -> Result<Expr> {
        match self.next() {
            Some(Spanned { tok: Tok::Star, .. }) => Ok(Expr::Wildcard),
            Some(Spanned {
                tok: Tok::Sctid(s), ..
            }) => Ok(Expr::Concept(s)),
            Some(s) => bail!("expected a concept id or '*' at position {}", s.pos),
            None => bail!("unexpected end of expression; expected a concept id or '*'"),
        }
    }

    fn parse_refinement(&mut self) -> Result<Refinement> {
        let mut left = self.parse_refine_and()?;
        while self.eat(&Tok::Or) {
            let right = self.parse_refine_and()?;
            left = Refinement::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_refine_and(&mut self) -> Result<Refinement> {
        let mut left = self.parse_refine_atom()?;
        while self.eat(&Tok::Comma) || self.eat(&Tok::And) {
            let right = self.parse_refine_atom()?;
            left = Refinement::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn parse_refine_atom(&mut self) -> Result<Refinement> {
        if self.eat(&Tok::LBrace) {
            self.enter()?;
            let inner = self.parse_refine_and();
            self.exit();
            let inner = inner?;
            if !self.eat(&Tok::RBrace) {
                bail!("expected '}}' to close attribute group {}", self.pos_hint());
            }
            return Ok(Refinement::Group(Box::new(inner)));
        }
        let attr = self.parse_sub()?;
        let negate = match self.peek_tok() {
            Some(Tok::Eq) => {
                self.idx += 1;
                false
            }
            Some(Tok::NotEq) => {
                self.idx += 1;
                true
            }
            _ => bail!(
                "expected '=' or '!=' in attribute constraint {}",
                self.pos_hint()
            ),
        };
        let value = self.parse_sub()?;
        Ok(Refinement::Attr {
            attr: Box::new(attr),
            negate,
            value: Box::new(value),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_concept() {
        assert_eq!(parse("73211009").unwrap(), Expr::Concept("73211009".into()));
        assert_eq!(
            parse("73211009 |Diabetes|").unwrap(),
            Expr::Concept("73211009".into())
        );
    }

    #[test]
    fn descendant_or_self() {
        assert_eq!(
            parse("<<73211009").unwrap(),
            Expr::Op(
                Op::DescendantOrSelfOf,
                Box::new(Expr::Concept("73211009".into()))
            )
        );
    }

    #[test]
    fn boolean_precedence_and_over_or() {
        // a OR b AND c  =>  a OR (b AND c)
        let e = parse("1 OR 2 AND 3").unwrap();
        assert_eq!(
            e,
            Expr::Bool(
                BoolOp::Or,
                Box::new(Expr::Concept("1".into())),
                Box::new(Expr::Bool(
                    BoolOp::And,
                    Box::new(Expr::Concept("2".into())),
                    Box::new(Expr::Concept("3".into())),
                )),
            )
        );
    }

    #[test]
    fn parens_group() {
        let e = parse("(1 OR 2) MINUS 3").unwrap();
        assert_eq!(
            e,
            Expr::Bool(
                BoolOp::Minus,
                Box::new(Expr::Bool(
                    BoolOp::Or,
                    Box::new(Expr::Concept("1".into())),
                    Box::new(Expr::Concept("2".into())),
                )),
                Box::new(Expr::Concept("3".into())),
            )
        );
    }

    #[test]
    fn refinement_attr() {
        let e = parse("<<404684003 : 363698007 = <<39057004").unwrap();
        match e {
            Expr::Refined(
                focus,
                Refinement::Attr {
                    attr,
                    negate,
                    value,
                },
            ) => {
                assert_eq!(
                    *focus,
                    Expr::Op(
                        Op::DescendantOrSelfOf,
                        Box::new(Expr::Concept("404684003".into()))
                    )
                );
                assert_eq!(*attr, Expr::Concept("363698007".into()));
                assert!(!negate);
                assert_eq!(
                    *value,
                    Expr::Op(
                        Op::DescendantOrSelfOf,
                        Box::new(Expr::Concept("39057004".into()))
                    )
                );
            }
            other => panic!("expected refined attr, got {other:?}"),
        }
    }

    #[test]
    fn refinement_conjunction_and_group() {
        assert!(parse("<<1 : 2 = 3, 4 = 5").is_ok());
        assert!(parse("<<1 : { 2 = 3, 4 = 5 }").is_ok());
        assert!(parse("<<1 : 2 != 3").is_ok());
    }

    #[test]
    fn refinement_binds_to_nearest_focus() {
        // `A OR B : r`  =>  `A OR (B : r)`
        let e = parse("1 OR <<2 : 3 = 4").unwrap();
        match e {
            Expr::Bool(BoolOp::Or, left, right) => {
                assert_eq!(*left, Expr::Concept("1".into()));
                assert!(matches!(*right, Expr::Refined(_, _)));
            }
            other => panic!("expected top-level OR, got {other:?}"),
        }
    }

    #[test]
    fn errors_are_clear() {
        assert!(parse("").is_err());
        assert!(parse("<<").is_err());
        assert!(parse("1 :").is_err());
        assert!(parse("1 : 2 3").is_err()); // missing '='
        assert!(parse("(1 OR 2").is_err()); // unclosed paren
    }

    #[test]
    fn deeply_nested_parens_are_rejected_not_stack_overflowed() {
        // A pathological input designed to overflow the recursive-descent
        // parser's stack; must return a clean error instead of aborting.
        let expr = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(parse(&expr).is_err());
    }

    #[test]
    fn deeply_nested_attribute_groups_are_rejected_not_stack_overflowed() {
        let expr = format!("1 : {}2 = 3{}", "{ ".repeat(100_000), " }".repeat(100_000));
        assert!(parse(&expr).is_err());
    }

    #[test]
    fn moderately_nested_parens_still_parse() {
        let expr = format!("{}1{}", "(".repeat(50), ")".repeat(50));
        assert!(parse(&expr).is_ok());
    }
}
