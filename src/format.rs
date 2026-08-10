// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Configurable concept-listing format used by `sct refset members`,
//! `sct lexical`, and any other subcommand that prints a list of concepts.
//!
//! The goal is a single, grep-friendly, `wc -l`-accurate line per concept
//! with a format the user can tune per-invocation (CLI flag) or globally
//! (`$SCT_CONFIG_HOME/config.toml` - see [`crate::paths`]):
//!
//! ```toml
//! [format]
//! concept = "{id} | {pt} ({hierarchy})"
//! concept_fsn_suffix = " - FSN: {fsn}"
//! ```
//!
//! The default renders, for a concept whose FSN differs from its PT:
//!
//! ```text
//! 164867002 | EKG: old myocardial infarction (Clinical finding) - FSN: Electrocardiographic old myocardial infarction
//! ```
//!
//! and for one where they match:
//!
//! ```text
//! 88380005 | Acute milk alkali syndrome (Clinical finding)
//! ```
//!
//! Supported template variables (all plain-text substitution):
//!
//! | Token | Value |
//! |---|---|
//! | `{id}`             | SCTID |
//! | `{pt}`             | Preferred term |
//! | `{fsn}`            | FSN with semantic tag stripped |
//! | `{fsn_raw}`        | FSN including the semantic tag |
//! | `{tag}`            | Semantic tag alone (e.g. `disorder`) |
//! | `{hierarchy}`      | Top-level hierarchy name |
//! | `{module}`         | Module SCTID |
//! | `{effective_time}` | Effective time (YYYYMMDD) |
//! | `{count}`          | Cardinality (e.g. refset member count); empty if not set |
//! | `{score}`          | Similarity score from `sct semantic`, formatted as `{:.4}`; empty if not set |
//!
//! Tokens that aren't relevant to the current command (e.g. `{score}` outside
//! `sct semantic`) render as empty strings, so a single global template can be
//! shared across commands without breaking those that lack the field.
//!
//! Unknown `{names}` are left as literal text so typos are visible.

use crate::builder::strip_semantic_tag;

/// Prefix marking a concept SNOMED International has retired. Applied by
/// [`ConceptFormat::render`] to every command that renders concepts through the
/// shared template, so an inactive code cannot appear in a result list looking
/// exactly like a live one. Plain ASCII inside the brackets so it survives a
/// pipe into `grep`, `cut`, or a spreadsheet.
pub const INACTIVE_MARKER: &str = "⚠ [INACTIVE] ";

#[derive(Debug, Clone)]
pub struct ConceptFormat {
    /// Template rendered once per concept.
    pub line: String,
    /// Suffix appended to `line` when the concept's FSN differs from its PT.
    /// Empty string to always suppress.
    pub fsn_suffix: String,
}

impl Default for ConceptFormat {
    fn default() -> Self {
        Self {
            line: "{id} | {pt} ({hierarchy})".into(),
            fsn_suffix: " - FSN: {fsn}".into(),
        }
    }
}

impl ConceptFormat {
    /// Render a single concept line. `fsn_suffix` is appended only when the
    /// FSN (with semantic tag stripped) differs from the PT and is non-empty.
    pub fn render(&self, fields: &ConceptFields<'_>) -> String {
        let fsn_clean = strip_semantic_tag(fields.fsn);
        let tag = semantic_tag(fields.fsn);
        let count = fields.count.map(|n| n.to_string()).unwrap_or_default();
        let score = fields.score.map(|s| format!("{s:.4}")).unwrap_or_default();
        let ctx = RenderCtx {
            id: fields.id,
            pt: fields.pt,
            fsn: fsn_clean,
            fsn_raw: fields.fsn,
            tag,
            hierarchy: fields.hierarchy,
            module: fields.module,
            effective_time: fields.effective_time,
            count: &count,
            score: &score,
        };

        let mut out = render_template(&self.line, &ctx);
        if !self.fsn_suffix.is_empty() && !fields.fsn.is_empty() && fsn_clean != fields.pt {
            out.push_str(&render_template(&self.fsn_suffix, &ctx));
        }
        if fields.inactive {
            // Prefixed, not appended: in a long list the marker has to be
            // visible without reading to the end of a line that may be
            // truncated by the terminal.
            out.insert_str(0, INACTIVE_MARKER);
        }
        out
    }

    /// Load the format from the shared config file, falling back to
    /// [`Default::default`] when the file or `[format]` section is absent.
    pub fn load() -> Self {
        let cfg = crate::paths::load_config();
        Self::from_config(cfg.format.as_ref())
    }

    fn from_config(f: Option<&crate::paths::FormatConfig>) -> Self {
        let d = Self::default();
        match f {
            None => d,
            Some(f) => Self {
                line: f.concept.clone().unwrap_or(d.line),
                fsn_suffix: f.concept_fsn_suffix.clone().unwrap_or(d.fsn_suffix),
            },
        }
    }

    /// Override the line and/or suffix templates (e.g. from CLI flags).
    pub fn with_overrides(mut self, line: Option<String>, suffix: Option<String>) -> Self {
        if let Some(l) = line {
            self.line = l;
        }
        if let Some(s) = suffix {
            self.fsn_suffix = s;
        }
        self
    }
}

/// The fields required to render one concept line. Optional fields render as
/// empty strings when `None`, so the same template can be shared across
/// commands that supply different subsets.
#[derive(Default)]
pub struct ConceptFields<'a> {
    pub id: &'a str,
    pub pt: &'a str,
    pub fsn: &'a str,
    pub hierarchy: &'a str,
    pub module: &'a str,
    pub effective_time: &'a str,
    /// Cardinality, e.g. number of refset members. Renders as empty string when None.
    pub count: Option<i64>,
    /// Similarity score, e.g. from `sct semantic`. Renders as empty string when None.
    pub score: Option<f64>,
    /// Whether this concept has been retired by SNOMED International. Rendered
    /// as a fixed prefix rather than a template token: a reader must not be
    /// able to remove a safety flag by customising their line format, and an
    /// inactive code that looks exactly like a live one is the failure this
    /// exists to prevent. Defaults to `false`, so a caller that has no active
    /// status shows no flag rather than a wrong one.
    pub inactive: bool,
}

// ---------------------------------------------------------------------------
// Template rendering
// ---------------------------------------------------------------------------

struct RenderCtx<'a> {
    id: &'a str,
    pt: &'a str,
    fsn: &'a str,
    fsn_raw: &'a str,
    tag: &'a str,
    hierarchy: &'a str,
    module: &'a str,
    effective_time: &'a str,
    count: &'a str,
    score: &'a str,
}

fn render_template(tmpl: &str, ctx: &RenderCtx<'_>) -> String {
    let mut out = String::with_capacity(tmpl.len() + 64);
    let mut rest = tmpl;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match lookup(ctx, name) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn lookup<'a>(ctx: &'a RenderCtx<'_>, name: &str) -> Option<&'a str> {
    match name {
        "id" => Some(ctx.id),
        "pt" => Some(ctx.pt),
        "fsn" => Some(ctx.fsn),
        "fsn_raw" => Some(ctx.fsn_raw),
        "tag" => Some(ctx.tag),
        "hierarchy" => Some(ctx.hierarchy),
        "module" => Some(ctx.module),
        "effective_time" => Some(ctx.effective_time),
        "count" => Some(ctx.count),
        "score" => Some(ctx.score),
        _ => None,
    }
}

/// Extract just the semantic tag ("disorder", "finding", ...) from an FSN.
fn semantic_tag(fsn: &str) -> &str {
    if let Some(start) = fsn.rfind(" (") {
        if let Some(stripped) = fsn[start + 2..].strip_suffix(')') {
            return stripped;
        }
    }
    ""
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fields<'a>(id: &'a str, pt: &'a str, fsn: &'a str, hier: &'a str) -> ConceptFields<'a> {
        ConceptFields {
            id,
            pt,
            fsn,
            hierarchy: hier,
            ..Default::default()
        }
    }

    #[test]
    fn default_format_pt_matches_fsn() {
        let f = ConceptFormat::default();
        let out = f.render(&fields(
            "88380005",
            "Acute milk alkali syndrome",
            "Acute milk alkali syndrome (disorder)",
            "Clinical finding",
        ));
        assert_eq!(
            out,
            "88380005 | Acute milk alkali syndrome (Clinical finding)"
        );
    }

    #[test]
    fn default_format_fsn_differs() {
        let f = ConceptFormat::default();
        let out = f.render(&fields(
            "164867002",
            "EKG: old myocardial infarction",
            "Electrocardiographic old myocardial infarction (finding)",
            "Clinical finding",
        ));
        assert_eq!(
            out,
            "164867002 | EKG: old myocardial infarction (Clinical finding) - FSN: Electrocardiographic old myocardial infarction"
        );
    }

    #[test]
    fn empty_suffix_always_suppresses_fsn() {
        let f = ConceptFormat {
            line: "{id} {pt}".into(),
            fsn_suffix: String::new(),
        };
        let out = f.render(&fields("1", "Foo", "Bar (disorder)", "H"));
        assert_eq!(out, "1 Foo");
    }

    #[test]
    fn override_line_template() {
        let f = ConceptFormat::default().with_overrides(Some("{id}\t{pt}".into()), None);
        let out = f.render(&fields(
            "22298006",
            "MI",
            "Myocardial infarction (disorder)",
            "CF",
        ));
        assert_eq!(out, "22298006\tMI - FSN: Myocardial infarction");
    }

    #[test]
    fn unknown_token_preserved_literally() {
        let f = ConceptFormat {
            line: "{id} {nope} {pt}".into(),
            fsn_suffix: String::new(),
        };
        let out = f.render(&fields("1", "Foo", "", ""));
        assert_eq!(out, "1 {nope} Foo");
    }

    #[test]
    fn unterminated_brace_preserved() {
        let f = ConceptFormat {
            line: "{id} {pt".into(),
            fsn_suffix: String::new(),
        };
        let out = f.render(&fields("1", "Foo", "", ""));
        assert_eq!(out, "1 {pt");
    }

    #[test]
    fn semantic_tag_variable() {
        let f = ConceptFormat {
            line: "{id} [{tag}] {pt}".into(),
            fsn_suffix: String::new(),
        };
        let out = f.render(&fields("1", "Foo", "Foo (disorder)", ""));
        assert_eq!(out, "1 [disorder] Foo");
    }

    #[test]
    fn empty_fsn_does_not_trigger_suffix() {
        let f = ConceptFormat::default();
        let out = f.render(&fields("1", "Foo", "", "CF"));
        assert_eq!(out, "1 | Foo (CF)");
    }

    #[test]
    fn count_token_renders_when_set() {
        let f = ConceptFormat {
            line: "{id} | {pt} ({count} members)".into(),
            fsn_suffix: String::new(),
        };
        let out = f.render(&ConceptFields {
            id: "1129631000000105",
            pt: "Summary Care Record exclusions simple reference set",
            count: Some(231),
            ..Default::default()
        });
        assert_eq!(
            out,
            "1129631000000105 | Summary Care Record exclusions simple reference set (231 members)"
        );
    }

    #[test]
    fn count_token_empty_when_unset() {
        let f = ConceptFormat {
            line: "{id} {pt} {count}".into(),
            fsn_suffix: String::new(),
        };
        let out = f.render(&fields("1", "Foo", "", ""));
        assert_eq!(out, "1 Foo ");
    }

    #[test]
    fn score_token_formats_to_four_decimals() {
        let f = ConceptFormat {
            line: "{score} [{id}] {pt}".into(),
            fsn_suffix: String::new(),
        };
        let out = f.render(&ConceptFields {
            id: "22298006",
            pt: "Myocardial infarction",
            score: Some(0.91234567),
            ..Default::default()
        });
        assert_eq!(out, "0.9123 [22298006] Myocardial infarction");
    }

    #[test]
    fn score_token_empty_when_unset() {
        let f = ConceptFormat {
            line: "{score}{id}".into(),
            fsn_suffix: String::new(),
        };
        let out = f.render(&fields("1", "Foo", "", ""));
        assert_eq!(out, "1");
    }
}
