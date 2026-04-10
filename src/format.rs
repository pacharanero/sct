//! Configurable concept-listing format used by `sct refset members`,
//! `sct lexical`, and any other subcommand that prints a list of concepts.
//!
//! The goal is a single, grep-friendly, `wc -l`-accurate line per concept
//! with a format the user can tune per-invocation (CLI flag) or globally
//! (`~/.config/sct/config.toml`):
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
//!
//! Unknown `{names}` are left as literal text so typos are visible.

use serde::Deserialize;
use std::path::PathBuf;

use crate::builder::strip_semantic_tag;

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
        let ctx = RenderCtx {
            id: fields.id,
            pt: fields.pt,
            fsn: fsn_clean,
            fsn_raw: fields.fsn,
            tag,
            hierarchy: fields.hierarchy,
            module: fields.module,
            effective_time: fields.effective_time,
        };

        let mut out = render_template(&self.line, &ctx);
        if !self.fsn_suffix.is_empty() && !fields.fsn.is_empty() && fsn_clean != fields.pt {
            out.push_str(&render_template(&self.fsn_suffix, &ctx));
        }
        out
    }

    /// Load the format from `~/.config/sct/config.toml`, falling back to
    /// [`Default::default`] on any error (missing file, parse error, etc.).
    pub fn load() -> Self {
        Self::load_from_home().unwrap_or_default()
    }

    fn load_from_home() -> Option<Self> {
        let home = std::env::var("HOME").ok()?;
        let path = PathBuf::from(home)
            .join(".config")
            .join("sct")
            .join("config.toml");
        if !path.exists() {
            return None;
        }
        let contents = std::fs::read_to_string(&path).ok()?;
        let root: RootConfig = toml::from_str(&contents).ok()?;
        let f = root.format?;
        let d = Self::default();
        Some(Self {
            line: f.concept.unwrap_or(d.line),
            fsn_suffix: f.concept_fsn_suffix.unwrap_or(d.fsn_suffix),
        })
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

/// The fields required to render one concept line.
pub struct ConceptFields<'a> {
    pub id: &'a str,
    pub pt: &'a str,
    pub fsn: &'a str,
    pub hierarchy: &'a str,
    pub module: &'a str,
    pub effective_time: &'a str,
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
// Config file
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RootConfig {
    format: Option<FormatSection>,
}

#[derive(Deserialize)]
struct FormatSection {
    concept: Option<String>,
    concept_fsn_suffix: Option<String>,
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
            module: "",
            effective_time: "",
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
}
