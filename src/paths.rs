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

/// Stem used when no name can be derived from the input - piped stdin, or a
/// path with no usable file name. Chosen so the canonical filenames above fall
/// out of [`derived_output`] unchanged.
const FALLBACK_STEM: &str = "snomed";

/// Default output path for a build command, named after its input.
///
/// Every artefact-producing command names its output `<input stem><suffix>`, so
/// a release identity set once at the top of the pipeline survives to the end:
///
/// ```text
/// uk-monolith-42.ndjson  --->  uk-monolith-42.db
///                              uk-monolith-42.parquet
///                              uk-monolith-42-embeddings.arrow
/// ```
///
/// The canonical names are the same rule applied to a canonical input:
/// `snomed.ndjson` yields `snomed.db` and `snomed-embeddings.arrow`, which is
/// what these commands defaulted to before the stem was propagated.
///
/// The result is always a bare filename, so output lands in the working
/// directory rather than beside an input that may live somewhere read-only.
/// `-` (stdin) and unusable names fall back to [`FALLBACK_STEM`].
pub fn derived_output(input: &Path, suffix: &str) -> PathBuf {
    let stem = if input.as_os_str() == "-" {
        None
    } else {
        input.file_stem().and_then(|s| s.to_str())
    }
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .unwrap_or(FALLBACK_STEM);

    PathBuf::from(format!("{stem}{suffix}"))
}

/// Locate an FST index when `--index` is not supplied.
///
/// `sct fst build` names its output after its input, so the index next door is
/// usually `<release>.fst` rather than the canonical `snomed.fst`. Prefer the
/// canonical name, then fall back to the newest `*.fst` in the same directory -
/// the same shape as the `--db` chain's last two steps, minus the env var and
/// config entry an index has never had.
///
/// `dir` is the directory to search: the working directory for `sct fst search`
/// and `sct sayt`, or the database's directory for `sct serve`.
pub fn find_fst_index(dir: &Path) -> Option<PathBuf> {
    let canonical = dir.join(CANONICAL_FST);
    if canonical.exists() {
        return Some(canonical);
    }
    newest_with_extension(dir, "fst")
}

/// Canonical FST index name, and the fallback stem's index name.
const CANONICAL_FST: &str = "snomed.fst";

/// Suffix each build command appends to the stem. Kept together so the set of
/// artefact names is visible in one place.
pub mod suffix {
    /// `sct sqlite`
    pub const DB: &str = ".db";
    /// `sct parquet`
    pub const PARQUET: &str = ".parquet";
    /// `sct fst build`
    pub const FST: &str = ".fst";
    /// `sct embed` - matches the canonical `snomed-embeddings.arrow`.
    pub const EMBEDDINGS: &str = "-embeddings.arrow";
    /// `sct markdown` - a directory, not a file.
    pub const MARKDOWN_DIR: &str = "-concepts";
}

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

/// Whether a numeric argument that fails SCTID check-digit validation should
/// be a hard error (`true`) rather than a warning appended to the existing
/// not-found message (`false`, the default). See `[lookup]` in
/// `docs/path-resolution.md`.
pub fn strict_sctid_checksum() -> bool {
    load_config()
        .lookup
        .and_then(|l| l.strict_sctid_checksum)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Config file schema (single source of truth)
// ---------------------------------------------------------------------------

// An unrecognised section or key (a typo'd name, most often) must not
// silently keep every field at its lenient default - that is exactly the
// silent-degrade-to-a-broader-default failure the invariant in
// `spec/roadmap.md`'s bug-audit section forbids, and `strict_sctid_checksum`
// below exists specifically to turn lenient defaults into hard errors, so a
// typo in its own key must not silently defeat it. `deny_unknown_fields`
// routes an unrecognised section/key through `load_config_from`'s existing
// "could not parse" warning branch rather than adding a new one.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub paths: Option<PathsConfig>,
    pub trud: Option<TrudConfig>,
    pub format: Option<FormatConfig>,
    pub codelists: Option<CodelistsConfig>,
    pub lookup: Option<LookupConfig>,
}

/// `[paths]` section - default DB and embeddings overrides used when the
/// corresponding CLI flag is omitted.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct PathsConfig {
    pub db: Option<String>,
    pub embeddings: Option<String>,
}

/// `[codelists]` section - the registry directory bare-id `includes:` entries
/// (and `sct serve --codelists`) resolve against. See [`codelist_registry`].
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct CodelistsConfig {
    pub dir: Option<String>,
}

/// `[trud]` section - see `spec/commands/trud.md`.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct TrudConfig {
    pub api_key: Option<String>,
    pub download_dir: Option<String>,
    pub data_dir: Option<String>,
    pub default_edition: Option<String>,
    pub editions: Option<HashMap<String, EditionProfile>>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct EditionProfile {
    pub trud_item: u32,
}

/// `[format]` section - see `src/format.rs`.
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct FormatConfig {
    pub concept: Option<String>,
    pub concept_fsn_suffix: Option<String>,
}

/// `[lookup]` section - see [`strict_sctid_checksum`].
#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct LookupConfig {
    pub strict_sctid_checksum: Option<bool>,
}

/// Load the merged config file. Missing files return `Config::default()`;
/// malformed files - including an unrecognised section or key, most often a
/// typo, which `deny_unknown_fields` on every section turns into a parse
/// error rather than a silently-defaulted field - do the same, with a stderr
/// warning, so every command can assume `load_config()` succeeds.
pub fn load_config() -> Config {
    load_config_from(&config_path())
}

/// Inner loader - accepts an explicit path so tests can supply a temp file.
/// Print a config diagnostic once per path per process.
///
/// `load_config()` is called from several places while serving one command, so
/// an unparseable file would otherwise repeat the same warning at each - noise
/// that buries the message rather than reinforcing it.
fn warn_once(path: &Path, message: &str) {
    static WARNED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    let seen = WARNED.get_or_init(Default::default);
    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    if seen.insert(path.to_path_buf()) {
        eprintln!("{message}");
    }
}

pub fn load_config_from(path: &Path) -> Config {
    if !path.exists() {
        return Config::default();
    }
    match fs::read_to_string(path) {
        Err(e) => {
            warn_once(
                path,
                &format!("Warning: could not read {}: {e}", path.display()),
            );
            Config::default()
        }
        Ok(contents) => match toml::from_str::<Config>(&contents) {
            Ok(c) => c,
            Err(e) => {
                // Say what was lost, not only what was wrong. A file that
                // fails to parse is discarded *whole*, so one typo'd key takes
                // every other setting with it - including a correctly spelled
                // `strict_sctid_checksum`, whose whole job is to be less
                // forgiving than the default. A user who reads only "could not
                // parse" will reasonably assume the bad line was skipped and
                // the rest applied.
                warn_once(
                    path,
                    &format!(
                        "Warning: could not parse {}: {e}\n\
                         Warning: no settings from that file are in effect - every option, \
                         including any spelled correctly elsewhere in it, falls back to its \
                         default until the file parses.",
                        path.display()
                    ),
                );
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
    CwdNewest,
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
            Source::CwdNewest => "cwd, newest".into(),
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

    // 7. Newest *.<ext> in the working directory.
    //
    // Build commands name their output after their input (see `derived_output`),
    // so a locally built artefact is usually `<release>.db` rather than the
    // canonical `snomed.db` that step 3 looks for. This last-resort step keeps
    // the zero-flag workflow working in a directory you just built in. It runs
    // last deliberately: it can never shadow an explicit flag, env var, config
    // entry, or a canonically named file, so no resolution that succeeds today
    // changes.
    let cwd_glob = format!("./*.{}", kind.extension());
    match newest_with_extension(Path::new("."), kind.extension()) {
        Some(p) => {
            return Ok(Resolved {
                path: p,
                source: Source::CwdNewest,
            });
        }
        None => tried.push((format!("{cwd_glob} (newest)"), "no matches")),
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
        // The `~/` case below reads HOME twice - once through `tilde_pathbuf`
        // and once through `expand_tilde` - so it must not interleave with a
        // sibling test that reassigns HOME, or the two reads disagree. Hold
        // [`ENV_LOCK`] for the same reason `env_and_cwd_chain_smoke` does.
        let _guard = super::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

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
    fn load_config_discards_the_file_on_an_unrecognised_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Typo'd key: `strict_sctid_chekcsum` instead of `strict_sctid_checksum`.
        fs::write(&path, "[lookup]\nstrict_sctid_chekcsum = true\n").unwrap();
        let cfg = load_config_from(&path);
        assert_eq!(
            cfg.lookup.and_then(|l| l.strict_sctid_checksum),
            None,
            "an unrecognised key must not silently keep parsing the rest of the section"
        );
    }

    #[test]
    fn load_config_discards_the_file_on_an_unrecognised_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Typo'd section: `[looku]` instead of `[lookup]`.
        fs::write(&path, "[looku]\nstrict_sctid_checksum = true\n").unwrap();
        let cfg = load_config_from(&path);
        assert!(
            cfg.lookup.is_none(),
            "an unrecognised section must not be silently ignored"
        );
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

        // ----- cwd newest is the last resort, and never shadows the data home -
        //
        // Build commands name their output after their input, so a locally
        // built database is `<release>.db`, not the canonical `snomed.db` that
        // step 3 looks for. It must still be found - but only once every
        // earlier step has come up empty.
        let local = cwd.path().join("locally-built.db");
        touch(&local, 0);
        let r = resolve(Kind::Db, None, &Config::default()).unwrap();
        assert_eq!(
            r.source,
            Source::DataHomeNewest,
            "a populated data home must still outrank the working directory"
        );

        // Empty the data home: now the local build is the only candidate.
        fs::remove_file(&newer).unwrap();
        fs::remove_file(&older).unwrap();
        let r = resolve(Kind::Db, None, &Config::default()).unwrap();
        assert_eq!(r.source, Source::CwdNewest);
        assert_eq!(
            r.path.file_name().unwrap(),
            std::ffi::OsStr::new("locally-built.db")
        );
        fs::remove_file(&local).unwrap();

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
