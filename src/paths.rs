// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Path & config resolution shared across every `sct` command.
//!
//! The conventions defined here are specified in
//! [`spec/path-resolution.md`](../../spec/path-resolution.md). In short:
//!
//! * Databases (`--db`) and embeddings (`--embeddings`) are auto-discovered
//!   through a five-step chain: explicit env var → CWD → config → canonical
//!   name under `$SCT_DATA_HOME/data` → newest matching file under that dir.
//! * `$SCT_DATA_HOME` defaults to `$XDG_DATA_HOME/sct` → `~/.local/share/sct`.
//! * `$SCT_CONFIG_HOME` defaults to `$XDG_CONFIG_HOME/sct` → `~/.config/sct`.
//! * A single `config.toml` houses all sections (`[paths]`, `[trud]`,
//!   `[format]`); commands ignore sections they don't read.
//!
//! `trud.rs` and `format.rs` use the types in this module so the config file
//! has exactly one definition of its schema.

use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Test-only env-mutation lock
// ---------------------------------------------------------------------------
//
// Tests that mutate process-wide env vars (HOME, SCT_DATA_HOME, ...) must
// acquire this mutex first. `cargo test` runs `#[test]` functions in
// parallel without per-test isolation, so two tests setting the same env
// var would race; both this module's `env_and_cwd_chain_smoke` and
// `trud::tests::env_directory_resolution_smoke` lock it at entry. Recovers
// from poisoning so a panicking test does not break sibling tests.

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ---------------------------------------------------------------------------
// Base directories
// ---------------------------------------------------------------------------

/// Sub-directory under `$SCT_DATA_HOME` for downloaded RF2 release zips.
pub const RELEASES_SUBDIR: &str = "releases";
/// Sub-directory under `$SCT_DATA_HOME` for built artefacts.
pub const DATA_SUBDIR: &str = "data";

/// Canonical filenames a user (or future `sct trud --link-latest`) may place
/// inside the data dir for predictable discovery.
const CANONICAL_DB: &str = "snomed.db";
const CANONICAL_EMBEDDINGS: &str = "snomed-embeddings.arrow";

/// Resolve the data root: `$SCT_DATA_HOME` → `$XDG_DATA_HOME/sct` →
/// `~/.local/share/sct`.
pub fn data_home() -> PathBuf {
    if let Some(p) = env_path_nonempty("SCT_DATA_HOME") {
        return p;
    }
    if let Some(xdg) = env_path_nonempty("XDG_DATA_HOME") {
        return xdg.join("sct");
    }
    home_dir().join(".local").join("share").join("sct")
}

/// Resolve the config root: `$SCT_CONFIG_HOME` → `$XDG_CONFIG_HOME/sct` →
/// `~/.config/sct`.
pub fn config_home() -> PathBuf {
    if let Some(p) = env_path_nonempty("SCT_CONFIG_HOME") {
        return p;
    }
    if let Some(xdg) = env_path_nonempty("XDG_CONFIG_HOME") {
        return xdg.join("sct");
    }
    home_dir().join(".config").join("sct")
}

/// Resolve the path to the config file. Order: `$SCT_CONFIG` → `./sct.toml` →
/// `$SCT_CONFIG_HOME/config.toml`. The returned path is the first that exists,
/// or - if none exist - the global default under `$SCT_CONFIG_HOME`.
pub fn config_path() -> PathBuf {
    if let Some(p) = env_path_nonempty("SCT_CONFIG") {
        return p;
    }
    let local = PathBuf::from("./sct.toml");
    if local.exists() {
        return local;
    }
    config_home().join("config.toml")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn env_path_nonempty(key: &str) -> Option<PathBuf> {
    std::env::var(key).ok().and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(expand_tilde(trimmed))
        }
    })
}

/// Expand a leading `~/` in `path` to `$HOME`. Other paths pass through
/// untouched. We deliberately do not support `~user/foo` - every caller is
/// either an env var or a config value, never an interactive shell token.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        home_dir().join(rest)
    } else if path == "~" {
        home_dir()
    } else {
        PathBuf::from(path)
    }
}

/// A clap `value_parser` that runs [`expand_tilde`] as a path argument is
/// parsed, so path flags accept `~` in *every* shell form - `--flag ~/x`,
/// `--flag=~/x`, and quoted `"~/x"` - not just the unquoted, space-separated
/// form the shell expands for us. Wire it onto every `PathBuf` CLI argument.
pub fn tilde_pathbuf(s: &str) -> Result<PathBuf, std::convert::Infallible> {
    Ok(expand_tilde(s))
}

/// Resolve the codelist registry directory that bare-id `includes:` entries -
/// and `sct serve --codelists` - look in. Resolution order: explicit `flag` →
/// `$SCT_CODELISTS` → `[codelists] dir` config → `./codelists`.
pub fn codelist_registry(flag: Option<&Path>) -> PathBuf {
    if let Some(p) = flag {
        return p.to_path_buf();
    }
    if let Some(p) = env_path_nonempty("SCT_CODELISTS") {
        return p;
    }
    if let Some(dir) = load_config()
        .codelists
        .and_then(|c| c.dir)
        .filter(|s| !s.trim().is_empty())
    {
        return expand_tilde(&dir);
    }
    PathBuf::from("codelists")
}

// ---------------------------------------------------------------------------
// Config file schema (single source of truth)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub paths: Option<PathsConfig>,
    pub trud: Option<TrudConfig>,
    pub format: Option<FormatConfig>,
    pub codelists: Option<CodelistsConfig>,
}

/// `[paths]` section - default DB and embeddings overrides used when the
/// corresponding CLI flag is omitted.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct PathsConfig {
    pub db: Option<String>,
    pub embeddings: Option<String>,
}

/// `[codelists]` section - the registry directory bare-id `includes:` entries
/// (and `sct serve --codelists`) resolve against. See [`codelist_registry`].
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct CodelistsConfig {
    pub dir: Option<String>,
}

/// `[trud]` section - see `spec/commands/trud.md`.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct TrudConfig {
    pub api_key: Option<String>,
    pub download_dir: Option<String>,
    pub data_dir: Option<String>,
    pub default_edition: Option<String>,
    pub editions: Option<HashMap<String, EditionProfile>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct EditionProfile {
    pub trud_item: u32,
}

/// `[format]` section - see `src/format.rs`.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct FormatConfig {
    pub concept: Option<String>,
    pub concept_fsn_suffix: Option<String>,
}

/// Load the merged config file. Missing or malformed files return
/// `Config::default()` (with a stderr warning in the malformed case), so every
/// command can assume `load_config()` succeeds.
pub fn load_config() -> Config {
    load_config_from(&config_path())
}

/// Inner loader - accepts an explicit path so tests can supply a temp file.
pub fn load_config_from(path: &Path) -> Config {
    if !path.exists() {
        return Config::default();
    }
    match fs::read_to_string(path) {
        Err(e) => {
            eprintln!("Warning: could not read {}: {e}", path.display());
            Config::default()
        }
        Ok(contents) => match toml::from_str::<Config>(&contents) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Warning: could not parse {}: {e}", path.display());
                Config::default()
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Resolved file discovery
// ---------------------------------------------------------------------------

/// The source of a resolved path - used by `sct paths` and embedded in the
/// "not found" error message so users can see exactly which rule won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env(&'static str),
    Cwd,
    Config,
    DataHomeCanonical,
    DataHomeNewest,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Flag => "--flag".into(),
            Source::Env(name) => format!("${name}"),
            Source::Cwd => "cwd".into(),
            Source::Config => "config [paths]".into(),
            Source::DataHomeCanonical => "data home, canonical name".into(),
            Source::DataHomeNewest => "data home, newest".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub path: PathBuf,
    pub source: Source,
}

/// Resolution kind. Each kind defines its env var name, CWD filename, config
/// field, and the glob extension to scan inside `$SCT_DATA_HOME/data`.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Db,
    Embeddings,
}

impl Kind {
    fn env_var(&self) -> &'static str {
        match self {
            Kind::Db => "SCT_DB",
            Kind::Embeddings => "SCT_EMBEDDINGS",
        }
    }
    fn cwd_name(&self) -> &'static str {
        match self {
            Kind::Db => CANONICAL_DB,
            Kind::Embeddings => CANONICAL_EMBEDDINGS,
        }
    }
    fn data_home_name(&self) -> &'static str {
        self.cwd_name()
    }
    fn extension(&self) -> &'static str {
        match self {
            Kind::Db => "db",
            Kind::Embeddings => "arrow",
        }
    }
    fn human_name(&self) -> &'static str {
        match self {
            Kind::Db => "SNOMED CT database",
            Kind::Embeddings => "embeddings file",
        }
    }
    fn build_hint(&self) -> &'static str {
        match self {
            Kind::Db => {
                "Build one with:\n  \
                 sct trud download --edition uk_monolith --pipeline\n  \
                 sct sqlite --ndjson snomed.ndjson"
            }
            Kind::Embeddings => {
                "Build one with:\n  \
                 sct embed --ndjson snomed.ndjson --output snomed-embeddings.arrow\n\
                 (requires Ollama; see `docs/commands/embed.md`)"
            }
        }
    }
    fn config_value<'a>(&self, cfg: &'a Config) -> Option<&'a str> {
        let p = cfg.paths.as_ref()?;
        let v = match self {
            Kind::Db => p.db.as_deref(),
            Kind::Embeddings => p.embeddings.as_deref(),
        };
        v.filter(|s| !s.trim().is_empty())
    }
}

/// Resolve a database path through the five-step chain. See
/// `spec/path-resolution.md`.
pub fn resolve_db(arg: Option<&Path>) -> Result<Resolved> {
    resolve(Kind::Db, arg, &load_config())
}

/// Resolve an embeddings file path through the five-step chain.
pub fn resolve_embeddings(arg: Option<&Path>) -> Result<Resolved> {
    resolve(Kind::Embeddings, arg, &load_config())
}

/// Inner resolver. Pure with respect to the supplied `cfg`; still touches the
/// filesystem for existence and mtime checks.
pub fn resolve(kind: Kind, arg: Option<&Path>, cfg: &Config) -> Result<Resolved> {
    // 1. Explicit flag - wins outright. We do not check existence here; the
    //    caller's open() will produce a clearer error than "file not found".
    if let Some(p) = arg {
        return Ok(Resolved {
            path: p.to_path_buf(),
            source: Source::Flag,
        });
    }

    let env_name = kind.env_var();
    let mut tried: Vec<(String, &'static str)> = Vec::with_capacity(5);

    // 2. Env var - must exist if set; do not silently fall through on a typo.
    match std::env::var(env_name) {
        Ok(v) if !v.trim().is_empty() => {
            let p = expand_tilde(v.trim());
            if p.exists() {
                return Ok(Resolved {
                    path: p,
                    source: Source::Env(env_name),
                });
            }
            anyhow::bail!(
                "${env_name} is set to {} but no file exists there.\n\
                 Unset the variable or point it at an existing {}.",
                p.display(),
                kind.human_name()
            );
        }
        _ => tried.push((format!("${env_name}"), "not set")),
    }

    // 3. CWD - preserves local-dev ergonomics.
    let cwd = PathBuf::from(format!("./{}", kind.cwd_name()));
    if cwd.exists() {
        return Ok(Resolved {
            path: cwd,
            source: Source::Cwd,
        });
    }
    tried.push((format!("./{}", kind.cwd_name()), "not present"));

    // 4. Config [paths].
    if let Some(raw) = kind.config_value(cfg) {
        let p = expand_tilde(raw);
        if p.exists() {
            return Ok(Resolved {
                path: p,
                source: Source::Config,
            });
        }
        tried.push((format!("config [paths] → {}", p.display()), "not present"));
    } else {
        tried.push(("config [paths]".into(), "unset"));
    }

    // 5. $SCT_DATA_HOME/data/<canonical name>
    let data_dir = data_home().join(DATA_SUBDIR);
    let canonical = data_dir.join(kind.data_home_name());
    if canonical.exists() {
        return Ok(Resolved {
            path: canonical,
            source: Source::DataHomeCanonical,
        });
    }
    tried.push((display_path(&canonical), "not present"));

    // 6. Newest *.<ext> in $SCT_DATA_HOME/data/
    let glob_label = display_path(&data_dir.join(format!("*.{}", kind.extension())));
    match newest_with_extension(&data_dir, kind.extension()) {
        Some(p) => {
            return Ok(Resolved {
                path: p,
                source: Source::DataHomeNewest,
            });
        }
        None => tried.push((format!("{glob_label} (newest)"), "no matches")),
    }

    anyhow::bail!(format_not_found(kind, &tried))
}

fn format_not_found(kind: Kind, tried: &[(String, &'static str)]) -> String {
    let mut out = format!("No {} found. Searched (in order):\n", kind.human_name());
    let flag = match kind {
        Kind::Db => "--db <path>",
        Kind::Embeddings => "--embeddings <path>",
    };
    out.push_str(&format!("  {flag:<48} (not supplied)\n"));
    for (label, status) in tried {
        out.push_str(&format!("  {label:<48} ({status})\n"));
    }
    out.push('\n');
    out.push_str(kind.build_hint());
    out
}

/// Return the path of the newest (by mtime, name as tie-breaker) regular file
/// in `dir` whose extension matches `ext`. Returns `None` if `dir` does not
/// exist or has no matching files.
fn newest_with_extension(dir: &Path, ext: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some(ext) {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        match &best {
            Some((bt, bp)) => {
                if mtime > *bt || (mtime == *bt && path > *bp) {
                    best = Some((mtime, path));
                }
            }
            None => best = Some((mtime, path)),
        }
    }
    best.map(|(_, p)| p)
}

/// Render a path with `~` substituted for `$HOME` for display purposes only.
/// Returned strings should not be re-opened.
pub fn display_path(p: &Path) -> String {
    let home = home_dir();
    if let Ok(rest) = p.strip_prefix(&home) {
        let s = rest.display().to_string();
        if s.is_empty() {
            "~".into()
        } else {
            format!("~/{s}")
        }
    } else {
        p.display().to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(path: &Path, age_secs: u64) {
        fs::write(path, b"").unwrap();
        let when = std::time::SystemTime::now() - Duration::from_secs(age_secs);
        let f = fs::File::options().write(true).open(path).unwrap();
        f.set_modified(when).unwrap();
    }

    /// Build a config with a `[paths]` section pointing at the given paths.
    fn cfg_with_paths(db: Option<&str>, emb: Option<&str>) -> Config {
        Config {
            paths: Some(PathsConfig {
                db: db.map(String::from),
                embeddings: emb.map(String::from),
            }),
            ..Default::default()
        }
    }

    // Pure tests (no env / cwd mutation) - safe to run in parallel.

    #[test]
    fn resolve_flag_wins() {
        let r = resolve(
            Kind::Db,
            Some(Path::new("/explicit.db")),
            &Config::default(),
        )
        .unwrap();
        assert_eq!(r.path, PathBuf::from("/explicit.db"));
        assert_eq!(r.source, Source::Flag);
    }

    #[test]
    fn tilde_pathbuf_passes_through_and_delegates() {
        // Non-`~` paths pass straight through (no HOME dependency).
        assert_eq!(
            tilde_pathbuf("/absolute/path").unwrap(),
            PathBuf::from("/absolute/path")
        );
        assert_eq!(
            tilde_pathbuf("relative/path").unwrap(),
            PathBuf::from("relative/path")
        );
        // A leading `~/` is expanded exactly as `expand_tilde` does.
        assert_eq!(tilde_pathbuf("~/x").unwrap(), expand_tilde("~/x"));
    }

    #[test]
    fn newest_picks_latest_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.db");
        let b = dir.path().join("b.db");
        let c = dir.path().join("c.arrow"); // wrong extension
        touch(&a, 100);
        touch(&b, 10);
        touch(&c, 0);
        let picked = newest_with_extension(dir.path(), "db").unwrap();
        assert_eq!(picked, b, "b.db is newer than a.db and should win");
    }

    #[test]
    fn newest_returns_none_for_missing_dir() {
        let p = newest_with_extension(Path::new("/definitely/does/not/exist"), "db");
        assert!(p.is_none());
    }

    #[test]
    fn source_labels_render() {
        assert_eq!(Source::Flag.label(), "--flag");
        assert_eq!(Source::Env("SCT_DB").label(), "$SCT_DB");
        assert_eq!(Source::Cwd.label(), "cwd");
        assert_eq!(Source::DataHomeNewest.label(), "data home, newest");
    }

    /// Env- and cwd-mutating tests, run sequentially inside a single `#[test]`
    /// (and serialised against the equivalent test in `trud::tests` via
    /// [`ENV_LOCK`]) because `cargo test` runs `#[test]` functions in
    /// parallel without per-test environment isolation.
    #[test]
    fn env_and_cwd_chain_smoke() {
        let _guard = super::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // ----- expand_tilde --------------------------------------------------
        let old_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", "/tmp/fake-home");
        }
        assert_eq!(expand_tilde("~/foo"), PathBuf::from("/tmp/fake-home/foo"));
        assert_eq!(expand_tilde("~"), PathBuf::from("/tmp/fake-home"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
        assert_eq!(expand_tilde("relative"), PathBuf::from("relative"));

        // ----- env var pointing at missing file ------------------------------
        unsafe {
            std::env::set_var("SCT_DB", "/nope/nope/missing.db");
        }
        let err = resolve(Kind::Db, None, &Config::default()).unwrap_err();
        assert!(format!("{err}").contains("but no file exists there"));
        unsafe {
            std::env::remove_var("SCT_DB");
        }

        // ----- not-found chain lists every step ------------------------------
        let tmp = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", tmp.path());
            std::env::set_var("SCT_DATA_HOME", data.path());
            std::env::remove_var("SCT_DB");
            std::env::remove_var("XDG_DATA_HOME");
        }
        let old_cwd = std::env::current_dir().ok();
        std::env::set_current_dir(cwd.path()).unwrap();
        let result = resolve(Kind::Db, None, &Config::default());
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("No SNOMED CT database found"), "{msg}");
        assert!(msg.contains("$SCT_DB"), "{msg}");
        assert!(msg.contains("./snomed.db"), "{msg}");
        assert!(msg.contains("sct trud download"), "{msg}");

        // ----- config [paths] db wins over (empty) data home ----------------
        let db = tmp.path().join("from-config.db");
        fs::write(&db, b"").unwrap();
        let cfg = cfg_with_paths(Some(db.to_str().unwrap()), None);
        let r = resolve(Kind::Db, None, &cfg).unwrap();
        assert_eq!(r.path, db);
        assert_eq!(r.source, Source::Config);

        // ----- data-home newest wins over data-home canonical-absent --------
        let newer = data.path().join("data").join("release-newer.db");
        let older = data.path().join("data").join("release-older.db");
        fs::create_dir_all(newer.parent().unwrap()).unwrap();
        touch(&older, 100);
        touch(&newer, 10);
        // Use a config without [paths] so we walk into the data dir.
        let r = resolve(Kind::Db, None, &Config::default()).unwrap();
        assert_eq!(r.path, newer);
        assert_eq!(r.source, Source::DataHomeNewest);

        // ----- restore --------------------------------------------------------
        if let Some(c) = old_cwd {
            let _ = std::env::set_current_dir(c);
        }
        unsafe {
            std::env::remove_var("SCT_DATA_HOME");
            match old_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}
