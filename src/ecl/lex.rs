// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tokeniser for the supported ECL subset. See `spec/ecl.md` §5.
//!
//! Whitespace is skipped. `|term|` annotations after a concept id are consumed
//! and discarded (they carry no semantics). Constructs outside the slice 1
//! grammar (cardinality `[`, dotted `.`, etc.) produce a clear error rather
//! than being silently mis-tokenised.

use anyhow::{bail, Result};

/// Refusal shown for any `{{ … }}` filter other than a history supplement.
/// `spec/ecl.md` promises the construct is named, not that the offending
/// character is pointed at: a user who has not met ECL 2.0 filters learns
/// nothing from "unexpected character".
pub const UNSUPPORTED_FILTER: &str = "unsupported ECL construct: only history supplements \
     (`{{ + HISTORY }}`) are implemented inside `{{ … }}`; description, member, \
     and concept filters are not yet supported";

/// A lexical token, paired with the source character offset where it starts
/// (for error messages).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    DescOrSelf, // <<
    Desc,       // <
    AncOrSelf,  // >>
    Anc,        // >
    Child,      // <!
    Parent,     // >!
    Member,     // ^
    Star,       // *
    Colon,      // :
    Comma,      // ,
    Eq,         // =
    NotEq,      // !=
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    LFilter,    // {{
    RFilter,    // }}
    Plus,       // +
    Sctid(String),
    And,
    Or,
    Minus,
    /// `HISTORY`, or `HISTORY-MIN` / `-MOD` / `-MAX`. The payload is the
    /// upper-cased profile suffix, empty for a bare `HISTORY`.
    History(String),
}

/// A token plus its starting character position.
#[derive(Debug, Clone)]
pub struct Spanned {
    pub tok: Tok,
    pub pos: usize,
}

/// Tokenise an ECL string.
pub fn lex(input: &str) -> Result<Vec<Spanned>> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    // Nesting depth of `{{ … }}`. Inside a filter, anything the supported
    // subset does not recognise is a filter construct, and saying so is more
    // useful than reporting the character that happened to stop the scan.
    let mut filter_depth = 0usize;

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let peek = |o: usize| chars.get(i + o).copied();

        macro_rules! push {
            ($t:expr, $adv:expr) => {{
                out.push(Spanned {
                    tok: $t,
                    pos: start,
                });
                i += $adv;
            }};
        }

        match c {
            '<' => match peek(1) {
                Some('<') => push!(Tok::DescOrSelf, 2),
                Some('!') => push!(Tok::Child, 2),
                _ => push!(Tok::Desc, 1),
            },
            '>' => match peek(1) {
                Some('>') => push!(Tok::AncOrSelf, 2),
                Some('!') => push!(Tok::Parent, 2),
                _ => push!(Tok::Anc, 1),
            },
            '^' => push!(Tok::Member, 1),
            '*' => push!(Tok::Star, 1),
            ':' => push!(Tok::Colon, 1),
            ',' => push!(Tok::Comma, 1),
            '=' => push!(Tok::Eq, 1),
            '!' => match peek(1) {
                Some('=') => push!(Tok::NotEq, 2),
                _ => bail!("unexpected '!' at position {start} (expected '!=')"),
            },
            '(' => push!(Tok::LParen, 1),
            ')' => push!(Tok::RParen, 1),
            '+' => push!(Tok::Plus, 1),
            // `{{` opens a filter/supplement; a single `{` opens an attribute
            // group. Longest match first, as for `<<` and `>>`.
            '{' => match peek(1) {
                Some('{') => {
                    filter_depth += 1;
                    push!(Tok::LFilter, 2)
                }
                _ => push!(Tok::LBrace, 1),
            },
            '}' => match peek(1) {
                Some('}') => {
                    filter_depth = filter_depth.saturating_sub(1);
                    push!(Tok::RFilter, 2)
                }
                _ => push!(Tok::RBrace, 1),
            },
            '|' => {
                // Consume a `|term|` annotation and discard it.
                i += 1;
                while i < chars.len() && chars[i] != '|' {
                    i += 1;
                }
                if i >= chars.len() {
                    bail!("unterminated |term| annotation starting at position {start}");
                }
                i += 1; // closing '|'
            }
            '0'..='9' => {
                let mut j = i;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                let s: String = chars[i..j].iter().collect();
                out.push(Spanned {
                    tok: Tok::Sctid(s),
                    pos: start,
                });
                i = j;
            }
            c if c.is_ascii_alphabetic() => {
                let mut j = i;
                while j < chars.len() && chars[j].is_ascii_alphabetic() {
                    j += 1;
                }
                let word: String = chars[i..j].iter().collect();
                let upper = word.to_ascii_uppercase();
                if upper == "HISTORY" {
                    // `HISTORY-MIN` is one keyword, not `HISTORY` `-` `MIN`:
                    // consuming the suffix here keeps `-` out of the token
                    // space and lets an unknown profile name be rejected by
                    // name rather than as a stray character.
                    let (profile, len) = history_profile(&chars, j)?;
                    out.push(Spanned {
                        tok: Tok::History(profile),
                        pos: start,
                    });
                    i = j + len;
                    continue;
                }
                let tok = match upper.as_str() {
                    "AND" => Tok::And,
                    "OR" => Tok::Or,
                    "MINUS" => Tok::Minus,
                    other if filter_depth > 0 => {
                        bail!("{UNSUPPORTED_FILTER} (found {other:?} at position {start})")
                    }
                    other => bail!(
                        "unsupported ECL keyword {other:?} at position {start} \
                         (supported: AND, OR, MINUS; reverse/dotted attributes are not yet implemented)"
                    ),
                };
                out.push(Spanned { tok, pos: start });
                i = j;
            }
            '[' | ']' => {
                bail!("attribute cardinality ('[..]') is not yet supported (position {start})")
            }
            '.' => bail!("dotted attributes ('.') are not yet supported (position {start})"),
            other if filter_depth > 0 => {
                bail!("{UNSUPPORTED_FILTER} (found {other:?} at position {start})")
            }
            other => bail!("unexpected character {other:?} at position {start}"),
        }
    }

    Ok(out)
}

/// Read the optional `-MIN` / `-MOD` / `-MAX` suffix of a `HISTORY` keyword
/// starting at `at` (the character just past `HISTORY`). Returns the
/// upper-cased profile - empty for a bare `HISTORY` - and how many characters
/// it consumed.
fn history_profile(chars: &[char], at: usize) -> Result<(String, usize)> {
    if chars.get(at) != Some(&'-') {
        return Ok((String::new(), 0));
    }
    let mut end = at + 1;
    while end < chars.len() && chars[end].is_ascii_alphabetic() {
        end += 1;
    }
    let profile: String = chars[at + 1..end].iter().collect::<String>().to_uppercase();
    if matches!(profile.as_str(), "MIN" | "MOD" | "MAX") {
        Ok((profile, end - at))
    } else {
        bail!(
            "unknown history supplement profile {profile:?} at position {at} \
             (expected HISTORY, HISTORY-MIN, HISTORY-MOD or HISTORY-MAX)"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).unwrap().into_iter().map(|s| s.tok).collect()
    }

    #[test]
    fn operators_longest_match() {
        assert_eq!(
            toks("<<73211009"),
            vec![Tok::DescOrSelf, Tok::Sctid("73211009".into())]
        );
        assert_eq!(toks("<! 1"), vec![Tok::Child, Tok::Sctid("1".into())]);
        assert_eq!(toks(">>2"), vec![Tok::AncOrSelf, Tok::Sctid("2".into())]);
        assert_eq!(toks(">!2"), vec![Tok::Parent, Tok::Sctid("2".into())]);
        assert_eq!(
            toks("^447562003"),
            vec![Tok::Member, Tok::Sctid("447562003".into())]
        );
    }

    #[test]
    fn discards_term_annotation() {
        assert_eq!(
            toks("73211009 |Diabetes mellitus|"),
            vec![Tok::Sctid("73211009".into())]
        );
    }

    #[test]
    fn keywords_case_insensitive() {
        assert_eq!(
            toks("1 or 2"),
            vec![Tok::Sctid("1".into()), Tok::Or, Tok::Sctid("2".into())]
        );
        assert_eq!(
            toks("1 MINUS 2"),
            vec![Tok::Sctid("1".into()), Tok::Minus, Tok::Sctid("2".into())]
        );
    }

    #[test]
    fn refinement_tokens() {
        assert_eq!(
            toks("1:2=<<3"),
            vec![
                Tok::Sctid("1".into()),
                Tok::Colon,
                Tok::Sctid("2".into()),
                Tok::Eq,
                Tok::DescOrSelf,
                Tok::Sctid("3".into()),
            ]
        );
    }

    #[test]
    fn history_supplement_tokens() {
        assert_eq!(
            toks("1 {{ + HISTORY }}"),
            vec![
                Tok::Sctid("1".into()),
                Tok::LFilter,
                Tok::Plus,
                Tok::History(String::new()),
                Tok::RFilter,
            ]
        );
        // The profile suffix is part of the keyword, and case-insensitive.
        assert_eq!(
            toks("{{+history-mod}}"),
            vec![
                Tok::LFilter,
                Tok::Plus,
                Tok::History("MOD".into()),
                Tok::RFilter,
            ]
        );
        // A single brace is still an attribute group.
        assert_eq!(
            toks("1 : { 2 = 3 }"),
            vec![
                Tok::Sctid("1".into()),
                Tok::Colon,
                Tok::LBrace,
                Tok::Sctid("2".into()),
                Tok::Eq,
                Tok::Sctid("3".into()),
                Tok::RBrace,
            ]
        );
    }

    #[test]
    fn unknown_history_profile_is_named() {
        let error = lex("1 {{ + HISTORY-EVERYTHING }}").unwrap_err().to_string();
        assert!(
            error.contains("unknown history supplement profile"),
            "{error}"
        );
        assert!(error.contains("HISTORY-MAX"), "{error}");
    }

    #[test]
    fn other_filters_are_refused_as_unsupported_constructs() {
        // Inside `{{ … }}` an unrecognised keyword or character means an ECL
        // filter we have not implemented - say so, rather than reporting the
        // character that stopped the scan.
        for expr in [
            "1 {{ term = \"asthma\" }}",
            "1 {{ M mapTarget = \"J45\" }}",
            "1 {{ D active = true }}",
        ] {
            let error = lex(expr).unwrap_err().to_string();
            assert!(
                error.contains("unsupported ECL construct"),
                "{expr}: {error}"
            );
        }
        // Outside a filter the original, more specific messages stand.
        assert!(lex("R 1").unwrap_err().to_string().contains("keyword"));
    }

    #[test]
    fn unsupported_constructs_error() {
        assert!(lex("1:2=3 [0..1]").is_err()); // cardinality
        assert!(lex("1.2").is_err()); // dotted
        assert!(lex("R 1").is_err()); // reverse keyword
        assert!(lex("73211009 |unterminated").is_err());
    }
}
