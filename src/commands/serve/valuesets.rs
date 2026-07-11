// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stored/named FHIR ValueSets backed by `.codelist` files. At startup the
//! server scans a registry directory, resolves each list's effective member set
//! (composition included), and serves them as ValueSet resources. See
//! `spec/commands/serve.md`.

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

use crate::commands::codelist::{self, FrontMatter};

/// One `.codelist` exposed as a FHIR ValueSet.
pub struct RegisteredValueSet {
    pub front_matter: FrontMatter,
    pub canonical_url: String,
    /// Effective members as `(sctid, stored_term)`, composition already flattened.
    pub members: Vec<(String, String)>,
}

impl RegisteredValueSet {
    /// Full ValueSet resource (with `compose.include.concept`). Shares the
    /// builder with `sct codelist export --format fhir-json` so the served and
    /// exported forms of a list are identical.
    pub fn to_resource(&self) -> Value {
        let members: Vec<(&str, &str)> = self
            .members
            .iter()
            .map(|(id, term)| (id.as_str(), term.as_str()))
            .collect();
        codelist::fhir_valueset(
            &self.front_matter,
            &members,
            Some(&self.canonical_url),
            true,
        )
    }

    /// Metadata-only resource (no `compose`/`expansion`) for search results.
    pub fn summary_resource(&self) -> Value {
        codelist::fhir_valueset(&self.front_matter, &[], Some(&self.canonical_url), false)
    }
}

/// In-memory index of the registered ValueSets, built once at startup.
#[derive(Default)]
pub struct ValueSetRegistry {
    by_id: HashMap<String, RegisteredValueSet>,
    by_url: HashMap<String, String>,
}

impl ValueSetRegistry {
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&RegisteredValueSet> {
        self.by_id.get(id)
    }

    /// Resolve an incoming `url` parameter to a registered ValueSet: by exact
    /// canonical URL, then by the trailing path segment treated as an id.
    pub fn resolve_url(&self, url: &str) -> Option<&RegisteredValueSet> {
        if let Some(id) = self.by_url.get(url) {
            return self.by_id.get(id);
        }
        let tail = url.rsplit('/').next().unwrap_or(url);
        self.by_id.get(tail)
    }

    /// All registered ValueSets (unordered).
    pub fn iter(&self) -> impl Iterator<Item = &RegisteredValueSet> {
        self.by_id.values()
    }
}

/// Scan `dir` for `*.codelist` files and build the registry. `base_url` forms
/// the canonical URL (`{base_url}/ValueSet/{id}`) for each list. Files that fail
/// to parse or whose includes do not resolve are skipped with a stderr warning,
/// so one bad list never takes the server down. A missing directory yields an
/// empty registry (the ValueSet routes then simply 404).
pub fn load_registry(dir: &Path, base_url: &str) -> ValueSetRegistry {
    let mut reg = ValueSetRegistry::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return reg,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("codelist") {
            continue;
        }
        match build_one(&path, dir, base_url) {
            Ok(rvs) => {
                let id = rvs.front_matter.id.clone();
                if let Some(prev) = reg.by_id.get(&id) {
                    eprintln!(
                        "warning: duplicate ValueSet id {id:?} ({} and {}); keeping the first",
                        prev.canonical_url,
                        path.display()
                    );
                    continue;
                }
                reg.by_url.insert(rvs.canonical_url.clone(), id.clone());
                reg.by_id.insert(id, rvs);
            }
            Err(e) => eprintln!("warning: skipping {}: {e:#}", path.display()),
        }
    }
    reg
}

fn build_one(
    path: &Path,
    registry_dir: &Path,
    base_url: &str,
) -> anyhow::Result<RegisteredValueSet> {
    let cl = codelist::read_codelist(path)?;
    let members = codelist::effective_members_of(&cl, path, registry_dir, false)?
        .into_iter()
        .map(|m| (m.id, m.term))
        .collect();
    let canonical_url = format!("{base_url}/ValueSet/{}", cl.front_matter.id);
    Ok(RegisteredValueSet {
        front_matter: cl.front_matter,
        canonical_url,
        members,
    })
}
