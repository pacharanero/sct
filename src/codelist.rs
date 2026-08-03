// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Typed `.codelist` files and offline include composition.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// YAML front-matter of a `.codelist` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontMatter {
    pub id: String,
    pub title: String,
    pub description: String,
    pub terminology: String,
    pub created: String,
    pub updated: String,
    pub version: u32,
    pub status: String,
    pub licence: String,
    pub copyright: String,
    pub appropriate_use: String,
    pub misuse: String,
    /// Other codelists whose members are composed into this one. Each entry is a
    /// bare id (resolved to `<registry>/<id>.codelist`), a path relative to this
    /// file, or an `http(s)://` URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub includes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snomed_release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<Author>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organisation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub methodology: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signoffs: Option<Vec<serde_yaml_ng::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<Warning>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub population: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub care_setting: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencodelists_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencodelists_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orcid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affiliation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Warning {
    pub code: String,
    pub severity: String,
    pub message: String,
}

/// A single parsed line from the concept body.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConceptLine {
    /// An active concept: `195967001    Asthma (disorder)  # optional comment`
    Active {
        id: String,
        term: String,
        comment: Option<String>,
    },
    /// An explicitly excluded concept: `# 41553006   Occupational asthma (disorder)`
    Excluded {
        id: String,
        term: String,
        comment: Option<String>,
    },
    /// Pending review: `# ? 57607007  Irritant-induced asthma (disorder)`
    PendingReview { id: String, term: String },
    /// Section header or free comment: `# -- heading --`
    Comment(String),
    /// Blank line (preserved).
    Blank,
}

impl ConceptLine {
    pub fn sctid(&self) -> Option<&str> {
        match self {
            ConceptLine::Active { id, .. } => Some(id),
            ConceptLine::Excluded { id, .. } => Some(id),
            ConceptLine::PendingReview { id, .. } => Some(id),
            _ => None,
        }
    }

    #[cfg(feature = "cli")]
    pub(crate) fn is_active(&self) -> bool {
        matches!(self, ConceptLine::Active { .. })
    }
}

/// A fully parsed `.codelist` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodelistFile {
    pub front_matter: FrontMatter,
    /// All lines of the body section, in order (preserves comments/blanks).
    pub body: Vec<ConceptLine>,
}

pub fn read_codelist(path: &Path) -> Result<CodelistFile> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_codelist(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn parse_codelist(text: &str) -> Result<CodelistFile> {
    let text = text.trim_start_matches('\u{feff}');
    let after_first = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .context("codelist file must start with '---'")?;
    let (yaml_part, body_part) = after_first
        .split_once("\n---")
        .context("codelist file missing closing '---' after front-matter")?;
    let body_part = body_part.trim_start_matches(['\n', '\r']);

    let front_matter = serde_yaml_ng::from_str(yaml_part).context("parsing YAML front-matter")?;
    let body = body_part.lines().map(parse_body_line).collect();
    Ok(CodelistFile { front_matter, body })
}

pub(crate) fn parse_body_line(line: &str) -> ConceptLine {
    let trimmed = line.trim();

    if trimmed.is_empty() {
        return ConceptLine::Blank;
    }

    if let Some(rest) = trimmed.strip_prefix('#') {
        let rest = rest.trim();

        if let Some(rest) = rest.strip_prefix('?') {
            let rest = rest.trim();
            if let Some((id, term)) = split_id_term(rest) {
                return ConceptLine::PendingReview { id, term };
            }
        }

        if rest
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
        {
            if let Some((id, rest_of_line)) = rest.split_once(|c: char| c.is_whitespace()) {
                let (term, comment) = split_term_comment(rest_of_line.trim());
                return ConceptLine::Excluded {
                    id: id.to_string(),
                    term,
                    comment,
                };
            }
        }

        return ConceptLine::Comment(trimmed.to_string());
    }

    if trimmed
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        if let Some((id, rest_of_line)) = trimmed.split_once(|c: char| c.is_whitespace()) {
            let (term, comment) = split_term_comment(rest_of_line.trim());
            return ConceptLine::Active {
                id: id.to_string(),
                term,
                comment,
            };
        }
    }

    ConceptLine::Comment(trimmed.to_string())
}

/// Split `"preferred term [# inline comment]"` into `(term, Option<comment>)`.
pub(crate) fn split_term_comment(s: &str) -> (String, Option<String>) {
    if let Some(idx) = s.find(" #") {
        let term = s[..idx].trim().to_string();
        let comment = s[idx + 2..].trim().to_string();
        (
            term,
            if comment.is_empty() {
                None
            } else {
                Some(comment)
            },
        )
    } else {
        (s.trim().to_string(), None)
    }
}

fn split_id_term(s: &str) -> Option<(String, String)> {
    let (id, rest) = s.split_once(|c: char| c.is_whitespace())?;
    if id.chars().all(|c| c.is_ascii_digit()) {
        Some((id.to_string(), rest.trim().to_string()))
    } else {
        None
    }
}

/// Render a codelist to its on-disk text form (front-matter + body).
pub fn render_codelist(cl: &CodelistFile) -> Result<String> {
    let yaml =
        serde_yaml_ng::to_string(&cl.front_matter).context("serialising YAML front-matter")?;
    let mut out = format!("---\n{}---\n", yaml);
    if !cl.body.is_empty() {
        out.push('\n');
        for line in &cl.body {
            out.push_str(&render_body_line(line));
            out.push('\n');
        }
    }
    Ok(out)
}

pub fn write_codelist(cl: &CodelistFile, path: &Path) -> Result<()> {
    write_codelist_atomic(cl, path, true)
}

#[cfg(feature = "cli")]
pub(crate) fn write_new_codelist(cl: &CodelistFile, path: &Path) -> Result<()> {
    write_codelist_atomic(cl, path, false)
}

fn write_codelist_atomic(cl: &CodelistFile, path: &Path, overwrite: bool) -> Result<()> {
    let out = render_codelist(cl)?;
    let target = if overwrite
        && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        std::fs::canonicalize(path)
            .with_context(|| format!("resolving symlink {}", path.display()))?
    } else {
        path.to_path_buf()
    };
    let existing_permissions = match std::fs::metadata(&target) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file(),
                "not a regular file: {}",
                target.display()
            );
            anyhow::ensure!(
                !metadata.permissions().readonly(),
                "refusing to replace read-only codelist {}",
                target.display()
            );
            Some(metadata.permissions())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("reading metadata for {}", target.display()))
        }
    };
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut builder = tempfile::Builder::new();
    if let Some(permissions) = existing_permissions.clone() {
        builder.permissions(permissions);
    }
    #[cfg(unix)]
    if existing_permissions.is_none() {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    let mut temp = builder
        .tempfile_in(parent)
        .with_context(|| format!("creating temporary file in {}", parent.display()))?;
    temp.write_all(out.as_bytes())
        .with_context(|| format!("writing temporary codelist for {}", path.display()))?;
    temp.flush()
        .with_context(|| format!("flushing temporary codelist for {}", path.display()))?;
    if let Some(permissions) = existing_permissions {
        temp.as_file()
            .set_permissions(permissions)
            .with_context(|| format!("preserving permissions for {}", target.display()))?;
    }
    temp.as_file()
        .sync_all()
        .with_context(|| format!("syncing temporary codelist for {}", path.display()))?;

    if overwrite {
        temp.persist(&target)
            .map_err(|error| error.error)
            .with_context(|| format!("atomically replacing {}", target.display()))?;
    } else {
        temp.persist_noclobber(&target)
            .map_err(|error| error.error)
            .with_context(|| format!("atomically creating {}", target.display()))?;
    }

    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("syncing directory {}", parent.display()))?;

    Ok(())
}

fn render_body_line(line: &ConceptLine) -> String {
    match line {
        ConceptLine::Active { id, term, comment } => {
            let base = format!("{id:<14} {term}");
            match comment {
                Some(c) => format!("{base}  # {c}"),
                None => base,
            }
        }
        ConceptLine::Excluded { id, term, comment } => {
            let base = format!("# {id:<13} {term}");
            match comment {
                Some(c) => format!("{base}  # {c}"),
                None => base,
            }
        }
        ConceptLine::PendingReview { id, term } => format!("# ? {id}  {term}"),
        ConceptLine::Comment(s) => s.clone(),
        ConceptLine::Blank => String::new(),
    }
}

/// How an `includes:` entry addresses another codelist.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IncludeRef {
    /// Bare token, e.g. `type-1-diabetes` -> `<registry>/type-1-diabetes.codelist`.
    Id(String),
    /// A path with a `/`, a `.codelist` suffix, or a `.`/`~`/`/` prefix,
    /// resolved relative to the including file's directory.
    Path(String),
    /// An `http(s)://` URL fetched as codelist text by an optional host resolver.
    Url(String),
}

/// Classify an `includes:` entry as a URL, path, or bare id.
pub fn parse_include_ref(raw: &str) -> IncludeRef {
    let r = raw.trim();
    if r.starts_with("http://") || r.starts_with("https://") {
        IncludeRef::Url(r.to_string())
    } else if r.contains('/')
        || r.ends_with(".codelist")
        || r.starts_with('.')
        || r.starts_with('~')
    {
        IncludeRef::Path(r.to_string())
    } else {
        IncludeRef::Id(r.to_string())
    }
}

/// Resolve an id or path include reference to a concrete `.codelist` path.
pub fn resolve_include_path(
    reference: &IncludeRef,
    including_file_dir: &Path,
    registry: &Path,
) -> Result<PathBuf> {
    match reference {
        IncludeRef::Id(id) => Ok(registry.join(format!("{id}.codelist"))),
        IncludeRef::Path(path) => {
            let expanded = expand_tilde(path);
            if expanded.is_absolute() {
                Ok(expanded)
            } else {
                Ok(including_file_dir.join(expanded))
            }
        }
        IncludeRef::Url(url) => bail!("URL includes are not yet supported: {url}"),
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    let home = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    if let Some(rest) = path.strip_prefix("~/") {
        home().join(rest)
    } else if path == "~" {
        home()
    } else {
        PathBuf::from(path)
    }
}

/// Where a resolved member came from.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemberSource {
    /// An `Active` line in this file.
    Direct,
    /// Contributed by an included codelist (carries the `includes:` ref label).
    Included(String),
}

/// A concept in the effective member set, with provenance.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectiveMember {
    pub id: String,
    pub term: String,
    pub source: MemberSource,
}

/// Resolve a file's effective members using local id/path includes only.
///
/// URL includes are rejected. Hosts that support URLs can call
/// [`effective_members_of_with_resolver`] with an explicit resolver.
pub fn effective_members_of(
    cl: &CodelistFile,
    file: &Path,
    registry: &Path,
) -> Result<Vec<EffectiveMember>> {
    effective_members_of_with_resolver(cl, file, registry, false, |url, _| {
        bail!("URL includes are unavailable in offline composition: {url}")
    })
}

pub(crate) fn effective_members_of_with_resolver<F>(
    cl: &CodelistFile,
    file: &Path,
    registry: &Path,
    refresh: bool,
    mut resolve_url: F,
) -> Result<Vec<EffectiveMember>>
where
    F: FnMut(&str, bool) -> Result<PathBuf>,
{
    let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut visited = HashSet::new();
    if let Ok(canonical) = std::fs::canonicalize(file) {
        visited.insert(canonical);
    }
    resolve_effective_members_with_resolver(
        cl,
        &dir,
        registry,
        refresh,
        &mut visited,
        &mut resolve_url,
    )
}

pub(crate) fn resolve_effective_members_with_resolver<F>(
    cl: &CodelistFile,
    including_file_dir: &Path,
    registry: &Path,
    refresh: bool,
    visited: &mut HashSet<PathBuf>,
    resolve_url: &mut F,
) -> Result<Vec<EffectiveMember>>
where
    F: FnMut(&str, bool) -> Result<PathBuf>,
{
    let mut members: indexmap::IndexMap<String, EffectiveMember> = indexmap::IndexMap::new();

    if let Some(includes) = &cl.front_matter.includes {
        for raw in includes {
            let reference = parse_include_ref(raw);
            let path = match &reference {
                IncludeRef::Url(url) => resolve_url(url, refresh)
                    .with_context(|| format!("fetching include {raw:?}"))?,
                _ => resolve_include_path(&reference, including_file_dir, registry)
                    .with_context(|| format!("resolving include {raw:?}"))?,
            };
            let canonical = std::fs::canonicalize(&path)
                .with_context(|| format!("include {raw:?} -> {} not found", path.display()))?;
            if !visited.insert(canonical.clone()) {
                bail!(
                    "include cycle detected at {raw:?} ({})",
                    canonical.display()
                );
            }
            let child = read_codelist(&canonical)?;
            let child_dir = canonical
                .parent()
                .unwrap_or(including_file_dir)
                .to_path_buf();
            let child_members = resolve_effective_members_with_resolver(
                &child,
                &child_dir,
                registry,
                refresh,
                visited,
                resolve_url,
            )?;
            visited.remove(&canonical);
            for member in child_members {
                members.entry(member.id.clone()).or_insert(EffectiveMember {
                    id: member.id,
                    term: member.term,
                    source: MemberSource::Included(raw.clone()),
                });
            }
        }
    }

    for line in &cl.body {
        if let ConceptLine::Active { id, term, .. } = line {
            members.insert(
                id.clone(),
                EffectiveMember {
                    id: id.clone(),
                    term: term.clone(),
                    source: MemberSource::Direct,
                },
            );
        }
    }

    for line in &cl.body {
        if let ConceptLine::Excluded { id, .. } = line {
            members.shift_remove(id);
        }
    }

    Ok(members.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(title: &str) -> CodelistFile {
        parse_codelist(&format!(
            "---\nid: test\ntitle: {title}\ndescription: Test codes\nterminology: SNOMED CT\ncreated: '2026-01-01'\nupdated: '2026-01-01'\nversion: 1\nstatus: draft\nlicence: CC-BY-4.0\ncopyright: Test\nappropriate_use: Testing\nmisuse: None\n---\n\n123456 Test concept\n"
        ))
        .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replacement_preserves_mode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.codelist");
        write_codelist(&sample("First"), &path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        write_codelist(&sample("Second"), &path).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(read_codelist(&path).unwrap().front_matter.title, "Second");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replacement_preserves_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.codelist");
        let link = directory.path().join("link.codelist");
        write_codelist(&sample("First"), &target).unwrap();
        symlink(&target, &link).unwrap();

        write_codelist(&sample("Second"), &link).unwrap();

        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(read_codelist(&target).unwrap().front_matter.title, "Second");
    }

    #[test]
    fn atomic_replacement_refuses_read_only_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.codelist");
        write_codelist(&sample("First"), &path).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions.clone()).unwrap();

        assert!(write_codelist(&sample("Second"), &path).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn atomic_creation_does_not_clobber_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.codelist");
        write_codelist_atomic(&sample("First"), &path, false).unwrap();

        assert!(write_codelist_atomic(&sample("Second"), &path, false).is_err());
        assert_eq!(read_codelist(&path).unwrap().front_matter.title, "First");
    }
}
