// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Application-facing SDK for querying a local SNOMED CT database.
//!
//! [`Snomed`] owns one synchronous, read-only SQLite connection. It is suitable
//! for command-line tools, desktop applications, and one query worker. Servers
//! should open one instance per worker or pool independent instances rather
//! than sharing a single connection concurrently.

use indexmap::IndexMap;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::provenance;
use crate::schema::ConceptRef;
use crate::schema::SCHEMA_VERSION;

pub use crate::codelist::{
    Author, CodelistFile, ConceptLine, EffectiveMember, FrontMatter, IncludeRef, MemberSource,
    Warning,
};
pub use crate::provenance::Provenance;
pub use crate::refset::{
    HierarchyCount, RefsetComparison, RefsetDiffSet, RefsetMember, RefsetSummary,
};

/// A read-only SNOMED CT query session.
///
/// Create the database first with `sct sqlite`. The SDK never downloads or
/// bundles terminology content.
///
/// ```no_run
/// use sct_rs::sdk::Snomed;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let snomed = Snomed::open("snomed.db")?;
/// let concept = snomed.concept("22298006")?.expect("concept present");
/// assert_eq!(concept.preferred_term, "Myocardial infarction");
/// # Ok(())
/// # }
/// ```
pub struct Snomed {
    conn: Connection,
    path: PathBuf,
    provenance: Option<Provenance>,
    schema_compatibility: SchemaCompatibility,
    fst: Option<crate::index::Index>,
}

impl Snomed {
    /// Open an `sct sqlite` database in read-only query mode.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SctError> {
        let path = path.as_ref().to_path_buf();
        let conn = open_db_readonly(&path, None)?;
        let provenance = provenance::read_sqlite(&conn).map_err(|source| SctError::Query {
            source: source.into_boxed_dyn_error(),
        })?;
        let schema_compatibility = query_schema_compatibility(&conn)?;
        Ok(Self {
            conn,
            path,
            provenance,
            schema_compatibility,
            fst: None,
        })
    }

    /// Return the path supplied to [`Snomed::open`].
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return release provenance when the database contains it.
    pub fn provenance(&self) -> Option<&Provenance> {
        self.provenance.as_ref()
    }

    /// Compatibility between this crate and the opened database schema.
    pub fn schema_compatibility(&self) -> SchemaCompatibility {
        self.schema_compatibility
    }

    /// Look up one concept by SCTID.
    pub fn concept(&self, id: &str) -> Result<Option<Concept>, SctError> {
        query_concept(&self.conn, id)
    }

    /// Search preferred terms, FSNs, and synonyms using SQLite FTS5.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchHit>, SctError> {
        self.search_with(SearchOptions::new(query, limit))
    }

    /// Search with an optional top-level hierarchy filter.
    pub fn search_with(&self, options: SearchOptions<'_>) -> Result<Vec<SearchHit>, SctError> {
        query_search(&self.conn, options)
    }

    #[cfg(feature = "cli")]
    pub(crate) fn search_ids_with(
        &self,
        options: SearchOptions<'_>,
    ) -> Result<Vec<String>, SctError> {
        query_search_ids(&self.conn, options)
    }

    /// Return direct children, ordered by preferred term.
    pub fn children(&self, id: &str, limit: u32) -> Result<Vec<ConceptSummary>, SctError> {
        query_direct(&self.conn, id, false, limit)
    }

    /// Return all proper ancestors, ordered from the immediate parent towards the root.
    pub fn ancestors(&self, id: &str) -> Result<Vec<ConceptSummary>, SctError> {
        query_ancestors(&self.conn, id)
    }

    /// Return proper descendants, ordered by preferred term.
    pub fn descendants(&self, id: &str, limit: u32) -> Result<Vec<ConceptSummary>, SctError> {
        query_descendants(&self.conn, id, limit)
    }

    /// Compare two concepts using reflexive SNOMED CT subsumption semantics.
    pub fn subsumes(&self, left: &str, right: &str) -> Result<Subsumption, SctError> {
        query_subsumption(&self.conn, left, right)
    }

    /// Return the proximal primitive supertypes of a concept: the most
    /// specific primitive concepts that are the concept itself or one of its
    /// ancestors. Every concept has at least one, since the root concept
    /// (138875005) is primitive and subsumes everything.
    ///
    /// Requires a database built with `sct sqlite` from schema v6 onward
    /// (the `definition_status` column); older databases return an error.
    pub fn proximal_primitive_supertypes(&self, id: &str) -> Result<Vec<ConceptSummary>, SctError> {
        query_proximal_primitive_supertypes(&self.conn, id)
    }

    /// Expand an ECL expression into sorted, deduplicated SCTIDs.
    pub fn expand(&self, expression: &str) -> Result<Vec<String>, SctError> {
        crate::ecl::expand(&self.conn, expression).map_err(|source| SctError::Query {
            source: source.into_boxed_dyn_error(),
        })
    }

    /// Whether the database currently has a usable transitive-closure table.
    ///
    /// Probe failures return `false` for compatibility. Use
    /// [`transitive_closure_usable`](Self::transitive_closure_usable) when the
    /// application needs to distinguish an unusable TCT from a database error.
    pub fn has_transitive_closure(&self) -> bool {
        self.transitive_closure_usable().unwrap_or(false)
    }

    /// Check current transitive-closure usability and preserve probe errors.
    pub fn transitive_closure_usable(&self) -> Result<bool, SctError> {
        crate::ecl::eval::has_tct(&self.conn).map_err(anyhow_query)
    }

    /// List all reference sets with loaded members.
    pub fn refsets(&self) -> Result<Vec<RefsetSummary>, SctError> {
        query_refsets(&self.conn, None)
    }

    /// Return metadata and the member count for one reference set concept.
    pub fn refset(&self, id: &str) -> Result<Option<RefsetSummary>, SctError> {
        crate::refset::refset_summary(&self.conn, id).map_err(anyhow_query)
    }

    /// List members of one reference set, ordered by preferred term.
    pub fn refset_members(
        &self,
        id: &str,
        limit: Option<u32>,
    ) -> Result<Vec<RefsetMember>, SctError> {
        query_refset_members(&self.conn, id, limit)
    }

    #[cfg(feature = "cli")]
    pub(crate) fn refset_member_ids(
        &self,
        id: &str,
        limit: Option<u32>,
    ) -> Result<Vec<String>, SctError> {
        crate::refset::list_refset_member_ids(&self.conn, id, limit.map(i64::from))
            .map_err(anyhow_query)
    }

    /// Compare the membership of two reference sets.
    pub fn refset_compare(
        &self,
        left: &str,
        right: &str,
        limit: Option<u32>,
    ) -> Result<RefsetComparison, SctError> {
        query_refset_compare(&self.conn, left, right, limit)
    }

    /// Count a reference set's members by top-level hierarchy.
    pub fn refset_profile(&self, id: &str) -> Result<Vec<HierarchyCount>, SctError> {
        query_refset_profile(&self.conn, id)
    }

    /// Map one code between supported terminology systems through SNOMED CT.
    pub fn map(
        &self,
        source: Terminology,
        code: &str,
        target: Terminology,
    ) -> Result<Vec<Mapping>, SctError> {
        query_map(&self.conn, source, code, target, false)
    }

    /// Map one code, forwarding inactive SNOMED pivots through history associations.
    pub fn map_forwarding_history(
        &self,
        source: Terminology,
        code: &str,
        target: Terminology,
    ) -> Result<Vec<Mapping>, SctError> {
        query_map(&self.conn, source, code, target, true)
    }

    /// Return recorded historical associations for one SNOMED CT concept.
    pub fn history(&self, id: &str) -> Result<Vec<HistoryAssociation>, SctError> {
        query_history(&self.conn, id)
    }

    /// Attach an FST index and return this query session.
    pub fn with_fst(mut self, path: impl AsRef<Path>) -> Result<Self, SctError> {
        self.attach_fst(path)?;
        Ok(self)
    }

    /// Attach or replace the FST index used by autocomplete methods.
    pub fn attach_fst(&mut self, path: impl AsRef<Path>) -> Result<(), SctError> {
        let path = path.as_ref();
        let index = crate::index::Index::open(path).map_err(|source| SctError::Index {
            path: path.to_path_buf(),
            source: source.into_boxed_dyn_error(),
        })?;
        let database = self
            .provenance
            .as_ref()
            .filter(|provenance| !provenance.release_id.is_empty())
            .ok_or(SctError::IndexProvenanceMissing {
                artefact: "database",
            })?;
        let index_provenance = index
            .provenance()
            .filter(|provenance| !provenance.release_id.is_empty())
            .ok_or(SctError::IndexProvenanceMissing {
                artefact: "FST index",
            })?;
        if database.release_id != index_provenance.release_id {
            return Err(SctError::IndexProvenanceMismatch {
                database_release: database.release_id.clone(),
                index_release: index_provenance.release_id.clone(),
            });
        }
        if let (Some(database_fingerprint), Some(index_fingerprint)) = (
            database.content_fingerprint.as_ref(),
            index_provenance.content_fingerprint.as_ref(),
        ) {
            if database_fingerprint != index_fingerprint {
                return Err(SctError::IndexContentMismatch {
                    database_fingerprint: database_fingerprint.clone(),
                    index_fingerprint: index_fingerprint.clone(),
                });
            }
        }
        self.fst = Some(index);
        Ok(())
    }

    /// Whether an FST index is attached.
    pub fn has_fst(&self) -> bool {
        self.fst.is_some()
    }

    /// Exact term lookup through the attached FST index.
    pub fn fst_exact(&self, term: &str) -> Result<Vec<AutocompleteHit>, SctError> {
        Ok(self
            .fst()?
            .lookup_exact(term)
            .into_iter()
            .map(AutocompleteHit::from)
            .collect())
    }

    /// Prefix lookup through the attached FST index.
    pub fn fst_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<AutocompleteHit>, SctError> {
        self.fst()?
            .lookup_prefix(prefix, limit)
            .map(|hits| hits.into_iter().map(AutocompleteHit::from).collect())
            .map_err(anyhow_query)
    }

    /// Fuzzy term lookup through the attached FST index.
    pub fn fst_fuzzy(
        &self,
        term: &str,
        max_distance: u32,
        limit: usize,
    ) -> Result<Vec<AutocompleteHit>, SctError> {
        self.fst()?
            .lookup_fuzzy(term, max_distance, limit)
            .map(|hits| hits.into_iter().map(AutocompleteHit::from).collect())
            .map_err(anyhow_query)
    }

    /// Return concepts whose indexed terms contain every supplied word.
    pub fn fst_words(
        &self,
        words: &[&str],
        limit: usize,
    ) -> Result<Vec<AutocompleteHit>, SctError> {
        Ok(self
            .fst()?
            .lookup_words(words, limit)
            .into_iter()
            .map(AutocompleteHit::from)
            .collect())
    }

    /// Ranked search-as-you-type over the attached FST index.
    pub fn autocomplete(
        &self,
        query: &str,
        limit: usize,
        fuzzy: bool,
    ) -> Result<Vec<AutocompleteHit>, SctError> {
        Ok(self
            .fst()?
            .search_typeahead(query, limit, fuzzy)
            .into_iter()
            .map(AutocompleteHit::from)
            .collect())
    }

    fn fst(&self) -> Result<&crate::index::Index, SctError> {
        self.fst.as_ref().ok_or(SctError::FstNotAttached)
    }

    #[cfg(feature = "cli")]
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// One concept returned by [`Snomed::concept`].
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Concept {
    pub id: String,
    pub fsn: String,
    pub preferred_term: String,
    pub synonyms: Vec<String>,
    pub hierarchy: String,
    pub hierarchy_path: Vec<String>,
    pub parents: Vec<ConceptRef>,
    pub children_count: usize,
    pub attributes: IndexMap<String, Vec<ConceptRef>>,
    pub active: bool,
    pub definition_status: String,
    pub module: String,
    pub effective_time: String,
    pub ctv3_codes: Vec<String>,
    pub read2_codes: Vec<String>,
    pub member_of: Vec<RefsetMembership>,
    /// Why this concept was retired, when it is inactive and the release
    /// records a reason. Always `None` for an active concept, and `None` on a
    /// database built before payload refsets were ingested.
    pub inactivation_reason: Option<InactivationReason>,
    /// What to use instead of this concept, when it is inactive: the RF2
    /// historical associations (`replaced_by`, `same_as`, ...) with the
    /// replacement's preferred term resolved. Empty for an active concept.
    pub historical_associations: Vec<HistoryAssociation>,
}

/// The reason a concept was inactivated, from the concept-inactivation
/// indicator reference set.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InactivationReason {
    /// SCTID of the inactivation value (e.g. `900000000000482003`).
    pub id: String,
    /// Human label (e.g. `Duplicate`). Falls back to the SCTID itself when the
    /// value concept is not present in this edition and is not one of the
    /// standard values.
    pub label: String,
}

#[cfg(feature = "serve")]
pub(crate) struct ConceptDesignations {
    pub preferred_term: String,
    pub fsn: String,
    pub synonyms: Vec<String>,
    pub active: bool,
    pub module: String,
    pub effective_time: String,
}

/// A reference set containing a concept.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefsetMembership {
    pub id: String,
    pub preferred_term: String,
}

/// One lexical search result.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchHit {
    pub id: String,
    pub preferred_term: String,
    pub fsn: String,
    pub hierarchy: String,
}

/// A compact concept record used by hierarchy methods.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConceptSummary {
    pub id: String,
    pub preferred_term: String,
    pub fsn: String,
}

/// The relationship between two concepts.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Subsumption {
    Equivalent,
    Subsumes,
    SubsumedBy,
    NotSubsumed,
}

/// A terminology supported by the cross-mapping engine.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Terminology {
    Snomed,
    Read2,
    Ctv3,
    Icd10,
    Opcs4,
}

impl Terminology {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snomed => "snomed",
            Self::Read2 => "read2",
            Self::Ctv3 => "ctv3",
            Self::Icd10 => "icd10",
            Self::Opcs4 => "opcs4",
        }
    }
}

impl FromStr for Terminology {
    type Err = SctError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "snomed" => Ok(Self::Snomed),
            "read2" => Ok(Self::Read2),
            "ctv3" => Ok(Self::Ctv3),
            "icd10" => Ok(Self::Icd10),
            "opcs4" => Ok(Self::Opcs4),
            _ => Err(SctError::UnsupportedTerminology {
                value: value.to_string(),
            }),
        }
    }
}

impl fmt::Display for Terminology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One cross-terminology mapping result.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mapping {
    pub target: String,
    pub snomed: String,
    pub display: Option<String>,
}

/// One historical association from a concept to a related concept.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryAssociation {
    pub source: String,
    pub association: String,
    pub target: String,
    pub target_display: Option<String>,
}

/// One result from an attached FST index.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutocompleteHit {
    /// SCTID as a string so JSON consumers do not lose integer precision.
    pub id: String,
    pub display: String,
    pub matched: String,
    pub semantic_tag: Option<String>,
    pub score: f32,
}

/// Compatibility between the current crate and a database's stored schema.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchemaCompatibility {
    Current,
    Older { database: u32, supported: u32 },
    Unknown,
}

impl From<crate::index::Hit> for AutocompleteHit {
    fn from(hit: crate::index::Hit) -> Self {
        Self {
            id: hit.concept_id.to_string(),
            display: hit.term,
            matched: hit.matched,
            semantic_tag: hit.semantic_tag,
            score: hit.score,
        }
    }
}

/// Options for [`Snomed::search_with`].
#[derive(Debug, Clone, Copy)]
pub struct SearchOptions<'a> {
    pub query: &'a str,
    pub limit: u32,
    pub hierarchy: Option<&'a str>,
    literal: bool,
}

impl<'a> SearchOptions<'a> {
    pub fn new(query: &'a str, limit: u32) -> Self {
        Self {
            query,
            limit,
            hierarchy: None,
            literal: false,
        }
    }

    pub fn hierarchy(mut self, hierarchy: &'a str) -> Self {
        self.hierarchy = Some(hierarchy);
        self
    }

    /// Treat the complete query as literal text rather than allowing FTS5 operators.
    pub fn literal(mut self) -> Self {
        self.literal = true;
        self
    }
}

/// Error returned by SDK operations.
#[non_exhaustive]
#[derive(Debug)]
pub enum SctError {
    Open {
        path: PathBuf,
        source: rusqlite::Error,
    },
    Query {
        source: Box<dyn Error + Send + Sync>,
    },
    InvalidData {
        field: &'static str,
        source: serde_json::Error,
    },
    UnsupportedTerminology {
        value: String,
    },
    InvalidSctid {
        value: String,
        source: std::num::ParseIntError,
    },
    ConceptNotFound {
        id: String,
    },
    Index {
        path: PathBuf,
        source: Box<dyn Error + Send + Sync>,
    },
    IndexProvenanceMismatch {
        database_release: String,
        index_release: String,
    },
    IndexProvenanceMissing {
        artefact: &'static str,
    },
    IndexContentMismatch {
        database_fingerprint: String,
        index_fingerprint: String,
    },
    FstNotAttached,
    UnsupportedSchema {
        database: u32,
        supported: u32,
    },
    InconsistentSchema {
        minimum: u32,
        maximum: u32,
    },
    Codelist {
        source: Box<dyn Error + Send + Sync>,
    },
}

impl SctError {
    fn query(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Query {
            source: Box::new(source),
        }
    }

    fn invalid_data(field: &'static str, source: serde_json::Error) -> Self {
        Self::InvalidData { field, source }
    }
}

impl fmt::Display for SctError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { path, .. } => {
                write!(f, "failed to open {} read-only", path.display())
            }
            Self::Query { .. } => write!(f, "SNOMED CT query failed"),
            Self::InvalidData { field, .. } => {
                write!(f, "database contains invalid JSON in {field}")
            }
            Self::UnsupportedTerminology { value } => {
                write!(f, "unsupported terminology {value:?}")
            }
            Self::InvalidSctid { value, .. } => write!(f, "invalid SCTID {value:?}"),
            Self::ConceptNotFound { id } => write!(f, "SNOMED CT concept {id} was not found"),
            Self::Index { path, .. } => write!(f, "failed to open FST index {}", path.display()),
            Self::IndexProvenanceMismatch {
                database_release,
                index_release,
            } => write!(
                f,
                "FST index release {index_release:?} does not match database release {database_release:?}"
            ),
            Self::IndexProvenanceMissing { artefact } => {
                write!(f, "{artefact} has no release identifier for FST validation")
            }
            Self::IndexContentMismatch {
                database_fingerprint,
                index_fingerprint,
            } => write!(
                f,
                "FST index content {index_fingerprint} does not match database content {database_fingerprint}"
            ),
            Self::FstNotAttached => write!(f, "no FST index is attached"),
            Self::UnsupportedSchema {
                database,
                supported,
            } => write!(
                f,
                "database schema version {database} is too new; this crate supports version {supported}"
            ),
            Self::InconsistentSchema { minimum, maximum } => write!(
                f,
                "database mixes schema versions {minimum} through {maximum}"
            ),
            Self::Codelist { .. } => write!(f, "codelist operation failed"),
        }
    }
}

impl Error for SctError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source, .. } => Some(source),
            Self::Query { source } => Some(source.as_ref()),
            Self::InvalidData { source, .. } => Some(source),
            Self::UnsupportedTerminology { .. } => None,
            Self::InvalidSctid { source, .. } => Some(source),
            Self::ConceptNotFound { .. } => None,
            Self::Index { source, .. } => Some(source.as_ref()),
            Self::IndexProvenanceMismatch { .. }
            | Self::IndexProvenanceMissing { .. }
            | Self::IndexContentMismatch { .. }
            | Self::FstNotAttached
            | Self::UnsupportedSchema { .. }
            | Self::InconsistentSchema { .. } => None,
            Self::Codelist { source } => Some(source.as_ref()),
        }
    }
}

pub(crate) fn open_db_readonly(
    path: &Path,
    cache_size_kib: Option<u32>,
) -> Result<Connection, SctError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|source| SctError::Open {
        path: path.to_path_buf(),
        source,
    })?;

    let mut pragmas = String::from("PRAGMA query_only = ON; PRAGMA mmap_size = 2147483648;");
    if let Some(kib) = cache_size_kib {
        pragmas.push_str(&format!("PRAGMA cache_size = -{kib};"));
    }
    conn.execute_batch(&pragmas).map_err(SctError::query)?;
    Ok(conn)
}

pub(crate) fn query_schema_compatibility(
    conn: &Connection,
) -> Result<SchemaCompatibility, SctError> {
    let has_column = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('concepts') WHERE name = 'schema_version'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !has_column {
        return Ok(SchemaCompatibility::Unknown);
    }
    let (minimum, maximum): (Option<u32>, Option<u32>) = conn
        .query_row(
            "SELECT MIN(schema_version), MAX(schema_version) FROM concepts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(SctError::query)?;
    let (Some(minimum), Some(maximum)) = (minimum, maximum) else {
        return Ok(SchemaCompatibility::Unknown);
    };
    if minimum != maximum {
        return Err(SctError::InconsistentSchema { minimum, maximum });
    }
    let database = minimum;
    if database == SCHEMA_VERSION {
        return Ok(SchemaCompatibility::Current);
    }
    if database < SCHEMA_VERSION {
        return Ok(SchemaCompatibility::Older {
            database,
            supported: SCHEMA_VERSION,
        });
    }
    Err(SctError::UnsupportedSchema {
        database,
        supported: SCHEMA_VERSION,
    })
}

pub(crate) fn query_concept(conn: &Connection, id: &str) -> Result<Option<Concept>, SctError> {
    let definition_status = if conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('concepts') WHERE name = 'definition_status'",
            [],
            |_| Ok(()),
        )
        .is_ok()
    {
        "definition_status"
    } else {
        "'' AS definition_status"
    };
    let sql = format!(
        "SELECT id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path,
                parents, children_count, attributes, active, {definition_status},
                module, effective_time, ctv3_codes, read2_codes
         FROM concepts WHERE id = ?1"
    );
    let result = conn.query_row(&sql, params![id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, bool>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, String>(11)?,
            row.get::<_, String>(12)?,
            row.get::<_, String>(13)?,
            row.get::<_, String>(14)?,
        ))
    });

    let (
        id,
        fsn,
        preferred_term,
        synonyms,
        hierarchy,
        hierarchy_path,
        parents,
        children_count,
        attributes,
        active,
        definition_status,
        module,
        effective_time,
        ctv3_codes,
        read2_codes,
    ) = match result {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(source) => return Err(SctError::query(source)),
    };

    Ok(Some(Concept {
        id: id.clone(),
        fsn,
        preferred_term,
        synonyms: parse_json("synonyms", &synonyms)?,
        hierarchy,
        hierarchy_path: parse_json("hierarchy_path", &hierarchy_path)?,
        parents: parse_json("parents", &parents)?,
        children_count: usize::try_from(children_count).map_err(SctError::query)?,
        attributes: parse_json("attributes", &attributes)?,
        active,
        definition_status,
        module,
        effective_time,
        ctv3_codes: parse_json("ctv3_codes", &ctv3_codes)?,
        read2_codes: parse_json("read2_codes", &read2_codes)?,
        member_of: query_refset_memberships(conn, &id)?,
        // Only inactive concepts carry these. RF2 association and inactivation
        // rows are recorded against the *retired* concept, so querying for an
        // active one is guaranteed to return nothing - skip the work rather
        // than pay it on the overwhelmingly common path.
        inactivation_reason: if active {
            None
        } else {
            query_inactivation_reason(conn, &id)?
        },
        historical_associations: if active {
            Vec::new()
        } else {
            query_history(conn, &id)?
        },
    }))
}

/// RF2 refset holding the reason a *concept* was inactivated. Deliberately not
/// `900000000000490003`, which is the parallel indicator for inactivated
/// *descriptions* and whose referenced component is a description id, not a
/// concept id.
const CONCEPT_INACTIVATION_INDICATOR_REFSET: &str = "900000000000489007";

/// Human labels for the standard concept-inactivation values, used when the
/// value concept itself is not in the loaded edition. These are SNOMED CT
/// metadata concepts: a full release contains them, but a database built from
/// a subset (or the committed synthetic fixture) may not, and reporting a bare
/// SCTID as the reason a code was retired is not much use to a reader.
///
/// Verified against the preferred terms in a UK Monolith 42.3.0 release rather
/// than written from memory.
const INACTIVATION_REASON_LABELS: &[(&str, &str)] = &[
    ("900000000000482003", "Duplicate"),
    ("900000000000483008", "Outdated"),
    ("900000000000484002", "Ambiguous"),
    ("900000000000485001", "Erroneous"),
    ("900000000000486000", "Limited"),
    ("900000000000492006", "Pending move"),
    ("900000000000495008", "Concept non-current"),
];

/// Why a concept was inactivated, if the release says.
///
/// Returns `None` - rather than failing - when the database predates the
/// payload-refset tables, so a query against an older database degrades to
/// "reason unknown" instead of an error.
pub(crate) fn query_inactivation_reason(
    conn: &Connection,
    id: &str,
) -> Result<Option<InactivationReason>, SctError> {
    if !table_exists(conn, "attribute_value_refset_members").map_err(SctError::query)? {
        return Ok(None);
    }
    // `active = 1` matters: a superseded indicator row is retained in the
    // Snapshot with active = 0, and treating one as current would report a
    // reason for a concept that is not inactivated at all.
    let value_id: Option<String> = conn
        .query_row(
            "SELECT value_id FROM attribute_value_refset_members
             WHERE referenced_component_id = ?1 AND refset_id = ?2 AND active = 1
             ORDER BY effective_time DESC LIMIT 1",
            params![id, CONCEPT_INACTIVATION_INDICATOR_REFSET],
            |row| row.get(0),
        )
        .optional()
        .map_err(SctError::query)?;

    let Some(value_id) = value_id else {
        return Ok(None);
    };
    let label = lookup_preferred_term_opt(conn, &value_id)?
        .or_else(|| {
            INACTIVATION_REASON_LABELS
                .iter()
                .find(|(candidate, _)| *candidate == value_id)
                .map(|(_, label)| (*label).to_string())
        })
        .unwrap_or_else(|| value_id.clone());
    Ok(Some(InactivationReason {
        id: value_id,
        label,
    }))
}

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
}

/// Preferred term for a concept that may legitimately be absent from this
/// edition, so a missing row is `None` rather than an error.
fn lookup_preferred_term_opt(conn: &Connection, id: &str) -> Result<Option<String>, SctError> {
    conn.query_row(
        "SELECT preferred_term FROM concepts WHERE id = ?1",
        [id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(SctError::query)
}

#[cfg(feature = "serve")]
pub(crate) fn query_concept_designations(
    conn: &Connection,
    id: &str,
) -> Result<Option<ConceptDesignations>, SctError> {
    let result = conn.query_row(
        "SELECT preferred_term, fsn, synonyms, active, module, effective_time
         FROM concepts WHERE id = ?1",
        [id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    );
    let (preferred_term, fsn, synonyms, active, module, effective_time) = match result {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(source) => return Err(SctError::query(source)),
    };
    Ok(Some(ConceptDesignations {
        preferred_term,
        fsn,
        synonyms: parse_json("synonyms", &synonyms)?,
        active,
        module,
        effective_time,
    }))
}

pub(crate) fn query_search(
    conn: &Connection,
    options: SearchOptions<'_>,
) -> Result<Vec<SearchHit>, SctError> {
    let limit = options.limit as usize;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let fts_query = sanitise_fts_query(options.query, options.literal);
    let map_row = |row: &rusqlite::Row<'_>| {
        Ok(SearchHit {
            id: row.get(0)?,
            preferred_term: row.get(1)?,
            fsn: row.get(2)?,
            hierarchy: row.get(3)?,
        })
    };

    let hits = if let Some(hierarchy) = options.hierarchy {
        let mut stmt = conn
            .prepare(
                "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy
                 FROM concepts_fts
                 JOIN concepts c ON concepts_fts.rowid = c.rowid
                 WHERE concepts_fts MATCH ?1 AND c.hierarchy = ?2
                 ORDER BY rank
                 LIMIT ?3",
            )
            .map_err(SctError::query)?;
        let hits = stmt
            .query_map(params![fts_query, hierarchy, limit as i64], map_row)
            .map_err(SctError::query)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(SctError::query)?;
        hits
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT c.id, c.preferred_term, c.fsn, c.hierarchy
                 FROM concepts_fts
                 JOIN concepts c ON concepts_fts.rowid = c.rowid
                 WHERE concepts_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(SctError::query)?;
        let hits = stmt
            .query_map(params![fts_query, limit as i64], map_row)
            .map_err(SctError::query)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(SctError::query)?;
        hits
    };
    Ok(hits)
}

#[cfg(any(feature = "cli", test))]
fn query_search_ids(
    conn: &Connection,
    options: SearchOptions<'_>,
) -> Result<Vec<String>, SctError> {
    if options.limit == 0 {
        return Ok(Vec::new());
    }
    let fts_query = sanitise_fts_query(options.query, options.literal);
    let ids = if let Some(hierarchy) = options.hierarchy {
        let mut stmt = conn
            .prepare(
                "SELECT c.id
                 FROM concepts_fts
                 JOIN concepts c ON concepts_fts.rowid = c.rowid
                 WHERE concepts_fts MATCH ?1 AND c.hierarchy = ?2
                 ORDER BY rank
                 LIMIT ?3",
            )
            .map_err(SctError::query)?;
        let ids = stmt
            .query_map(
                params![fts_query, hierarchy, i64::from(options.limit)],
                |row| row.get(0),
            )
            .map_err(SctError::query)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(SctError::query)?;
        ids
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT c.id
                 FROM concepts_fts
                 JOIN concepts c ON concepts_fts.rowid = c.rowid
                 WHERE concepts_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(SctError::query)?;
        let ids = stmt
            .query_map(params![fts_query, i64::from(options.limit)], |row| {
                row.get(0)
            })
            .map_err(SctError::query)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(SctError::query)?;
        ids
    };
    Ok(ids)
}

pub(crate) fn query_refsets(
    conn: &Connection,
    limit: Option<u32>,
) -> Result<Vec<RefsetSummary>, SctError> {
    crate::refset::list_refsets(conn, limit.map(i64::from)).map_err(anyhow_query)
}

pub(crate) fn query_refset_members(
    conn: &Connection,
    id: &str,
    limit: Option<u32>,
) -> Result<Vec<RefsetMember>, SctError> {
    crate::refset::list_refset_members(conn, id, limit.map(i64::from)).map_err(anyhow_query)
}

pub(crate) fn query_refset_compare(
    conn: &Connection,
    left: &str,
    right: &str,
    limit: Option<u32>,
) -> Result<RefsetComparison, SctError> {
    crate::refset::compare_refsets(conn, left, right, limit.map(i64::from)).map_err(anyhow_query)
}

pub(crate) fn query_refset_profile(
    conn: &Connection,
    id: &str,
) -> Result<Vec<HierarchyCount>, SctError> {
    crate::refset::profile_refset_by_hierarchy(conn, id).map_err(anyhow_query)
}

pub(crate) fn query_map(
    conn: &Connection,
    source: Terminology,
    code: &str,
    target: Terminology,
    forward_history: bool,
) -> Result<Vec<Mapping>, SctError> {
    crate::mapping::transcode_one(
        conn,
        source.as_str(),
        code,
        target.as_str(),
        forward_history,
    )
    .map(|rows| {
        rows.into_iter()
            .map(|row| Mapping {
                target: row.target,
                snomed: row.snomed,
                display: row.display,
            })
            .collect()
    })
    .map_err(anyhow_query)
}

pub(crate) fn query_history(
    conn: &Connection,
    id: &str,
) -> Result<Vec<HistoryAssociation>, SctError> {
    let mut stmt = match conn.prepare(
        "SELECT h.source_id, h.association, h.target_id, c.preferred_term
         FROM concept_history h
         LEFT JOIN concepts c ON c.id = h.target_id
         WHERE h.source_id = ?1
         ORDER BY h.association, h.target_id",
    ) {
        Ok(stmt) => stmt,
        Err(source) if source.to_string().contains("no such table") => return Ok(Vec::new()),
        Err(source) => return Err(SctError::query(source)),
    };
    let rows = stmt
        .query_map([id], |row| {
            Ok(HistoryAssociation {
                source: row.get(0)?,
                association: row.get(1)?,
                target: row.get(2)?,
                target_display: row.get(3)?,
            })
        })
        .map_err(SctError::query)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(SctError::query)
}

pub(crate) fn query_direct(
    conn: &Connection,
    id: &str,
    parents: bool,
    limit: u32,
) -> Result<Vec<ConceptSummary>, SctError> {
    let sql = if parents {
        "SELECT DISTINCT c.id, c.preferred_term, c.fsn
         FROM concepts c JOIN concept_isa ci ON ci.parent_id = c.id
         WHERE ci.child_id = ?1 ORDER BY c.preferred_term LIMIT ?2"
    } else {
        "SELECT DISTINCT c.id, c.preferred_term, c.fsn
         FROM concepts c JOIN concept_isa ci ON ci.child_id = c.id
         WHERE ci.parent_id = ?1 ORDER BY c.preferred_term LIMIT ?2"
    };
    query_summaries(conn, sql, params![id, limit])
}

pub(crate) fn query_ancestors(
    conn: &Connection,
    id: &str,
) -> Result<Vec<ConceptSummary>, SctError> {
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn).map_err(anyhow_query)?;
    let tct = crate::ecl::eval::has_tct(conn).map_err(anyhow_query)?;
    query_ancestors_with_tct(conn, id, tct)
}

pub(crate) fn query_ancestors_with_tct(
    conn: &Connection,
    id: &str,
    tct: bool,
) -> Result<Vec<ConceptSummary>, SctError> {
    let numeric_id = parse_sctid(id)?;
    let ancestors =
        crate::ecl::eval::ancestors_with_tct(conn, numeric_id, tct).map_err(|source| {
            SctError::Query {
                source: source.into_boxed_dyn_error(),
            }
        })?;
    let mut summaries = query_summaries_for_ids(conn, ancestors)?;
    summaries.sort_by_key(|summary| {
        std::cmp::Reverse(hierarchy_depth(conn, &summary.id).unwrap_or_default())
    });
    Ok(summaries)
}

pub(crate) fn query_descendants(
    conn: &Connection,
    id: &str,
    limit: u32,
) -> Result<Vec<ConceptSummary>, SctError> {
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn).map_err(anyhow_query)?;
    parse_sctid(id)?;
    let sql = if crate::ecl::eval::has_tct(conn).map_err(anyhow_query)? {
        "SELECT c.id, c.preferred_term, c.fsn
         FROM concept_ancestors ca
         JOIN concepts c ON c.id = CAST(ca.descendant_id AS TEXT)
         WHERE ca.ancestor_id = ?1 AND ca.descendant_id != ?1
         ORDER BY c.preferred_term, c.id LIMIT ?2"
    } else {
        "WITH RECURSIVE descendants(id) AS (
             SELECT child_id FROM concept_isa WHERE parent_id = ?1
             UNION
             SELECT ci.child_id FROM concept_isa ci
             JOIN descendants d ON ci.parent_id = d.id
         )
         SELECT c.id, c.preferred_term, c.fsn
         FROM descendants d JOIN concepts c ON c.id = d.id
         ORDER BY c.preferred_term, c.id LIMIT ?2"
    };
    query_summaries(conn, sql, params![id, limit])
}

pub(crate) fn query_subsumption(
    conn: &Connection,
    left: &str,
    right: &str,
) -> Result<Subsumption, SctError> {
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn).map_err(anyhow_query)?;
    let left_id = parse_sctid(left)?;
    let right_id = parse_sctid(right)?;
    require_concept(conn, left)?;
    if left != right {
        require_concept(conn, right)?;
    }
    if left == right {
        return Ok(Subsumption::Equivalent);
    }
    let tct = crate::ecl::eval::has_tct(conn).map_err(anyhow_query)?;
    let right_ancestors =
        crate::ecl::eval::ancestors_with_tct(conn, right_id, tct).map_err(|source| {
            SctError::Query {
                source: source.into_boxed_dyn_error(),
            }
        })?;
    if right_ancestors.contains(&left_id) {
        return Ok(Subsumption::Subsumes);
    }
    let left_ancestors =
        crate::ecl::eval::ancestors_with_tct(conn, left_id, tct).map_err(|source| {
            SctError::Query {
                source: source.into_boxed_dyn_error(),
            }
        })?;
    if left_ancestors.contains(&right_id) {
        Ok(Subsumption::SubsumedBy)
    } else {
        Ok(Subsumption::NotSubsumed)
    }
}

/// RF2 `definitionStatusId` for primitive concepts.
const PRIMITIVE_SCTID: &str = "900000000000074008";

fn query_proximal_primitive_supertypes(
    conn: &Connection,
    id: &str,
) -> Result<Vec<ConceptSummary>, SctError> {
    let _snapshot = crate::ecl::eval::ReadSnapshot::begin(conn).map_err(anyhow_query)?;
    let numeric_id = parse_sctid(id)?;
    require_concept(conn, id)?;
    if !has_definition_status_column(conn).map_err(SctError::query)? {
        return Err(anyhow_query(anyhow::anyhow!(
            "database has no 'definition_status' column; rebuild with a current sct \
             (`sct ndjson` then `sct sqlite`) to compute proximal primitive supertypes"
        )));
    }

    let tct = crate::ecl::eval::has_tct(conn).map_err(anyhow_query)?;
    let mut candidates =
        crate::ecl::eval::ancestors_with_tct(conn, numeric_id, tct).map_err(anyhow_query)?;
    candidates.insert(numeric_id);

    let mut primitive_ids = crate::ecl::IdSet::new();
    for &candidate in &candidates {
        if definition_status(conn, candidate)? == PRIMITIVE_SCTID {
            primitive_ids.insert(candidate);
        }
    }

    // Keep only the most specific primitives: drop any primitive that is a
    // proper ancestor of another primitive still in the set.
    let mut ancestors_of: std::collections::HashMap<u64, crate::ecl::IdSet> =
        std::collections::HashMap::with_capacity(primitive_ids.len());
    for &candidate in &primitive_ids {
        let ancestors =
            crate::ecl::eval::ancestors_with_tct(conn, candidate, tct).map_err(anyhow_query)?;
        ancestors_of.insert(candidate, ancestors);
    }
    let proximal: crate::ecl::IdSet = primitive_ids
        .iter()
        .copied()
        .filter(|&p| {
            !primitive_ids
                .iter()
                .any(|&q| q != p && ancestors_of[&q].contains(&p))
        })
        .collect();

    if proximal.is_empty() {
        return Err(anyhow_query(anyhow::anyhow!(
            "no primitive ancestors found for {id}; the database's \
             definition_status data may be incomplete"
        )));
    }

    query_summaries_for_ids(conn, proximal)
}

fn has_definition_status_column(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM pragma_table_info('concepts') WHERE name = 'definition_status'",
        [],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
}

/// Read a concept's `definition_status`. The column is nullable in the real
/// schema (see `sct sqlite`'s `concepts` DDL), and databases migrated from an
/// older schema can leave it unset, so NULL is read as "unknown" - which is
/// simply not primitive - rather than surfacing a raw column-type error.
fn definition_status(conn: &Connection, id: u64) -> Result<String, SctError> {
    conn.query_row(
        "SELECT definition_status FROM concepts WHERE id = ?1",
        [id.to_string()],
        |row| row.get::<_, Option<String>>(0),
    )
    .map(Option::unwrap_or_default)
    .map_err(SctError::query)
}

fn query_summaries(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<ConceptSummary>, SctError> {
    let mut stmt = conn.prepare(sql).map_err(SctError::query)?;
    let rows = stmt
        .query_map(params, |row| {
            Ok(ConceptSummary {
                id: row.get(0)?,
                preferred_term: row.get(1)?,
                fsn: row.get(2)?,
            })
        })
        .map_err(SctError::query)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(SctError::query)
}

fn query_summaries_for_ids(
    conn: &Connection,
    ids: crate::ecl::IdSet,
) -> Result<Vec<ConceptSummary>, SctError> {
    let mut stmt = conn
        .prepare_cached("SELECT id, preferred_term, fsn FROM concepts WHERE id = ?1")
        .map_err(SctError::query)?;
    let mut summaries = Vec::with_capacity(ids.len());
    for id in ids {
        let summary = stmt
            .query_row([id.to_string()], |row| {
                Ok(ConceptSummary {
                    id: row.get(0)?,
                    preferred_term: row.get(1)?,
                    fsn: row.get(2)?,
                })
            })
            .map_err(SctError::query)?;
        summaries.push(summary);
    }
    Ok(summaries)
}

fn hierarchy_depth(conn: &Connection, id: &str) -> Result<usize, SctError> {
    let path: String = conn
        .query_row(
            "SELECT hierarchy_path FROM concepts WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .map_err(SctError::query)?;
    let path: Vec<String> = parse_json("hierarchy_path", &path)?;
    Ok(path.len())
}

fn parse_sctid(id: &str) -> Result<u64, SctError> {
    id.parse::<u64>().map_err(|source| SctError::InvalidSctid {
        value: id.to_string(),
        source,
    })
}

fn require_concept(conn: &Connection, id: &str) -> Result<(), SctError> {
    let exists = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM concepts WHERE id = ?1)",
            [id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(SctError::query)?;
    if exists {
        Ok(())
    } else {
        Err(SctError::ConceptNotFound { id: id.to_string() })
    }
}

fn anyhow_query(source: anyhow::Error) -> SctError {
    SctError::Query {
        source: source.into_boxed_dyn_error(),
    }
}

fn codelist_error(source: anyhow::Error) -> SctError {
    SctError::Codelist {
        source: source.into_boxed_dyn_error(),
    }
}

/// Parse an in-memory `.codelist` document.
pub fn parse_codelist(text: &str) -> Result<CodelistFile, SctError> {
    crate::codelist::parse_codelist(text).map_err(codelist_error)
}

/// Read and parse a `.codelist` file.
pub fn read_codelist(path: impl AsRef<Path>) -> Result<CodelistFile, SctError> {
    crate::codelist::read_codelist(path.as_ref()).map_err(codelist_error)
}

/// Render a codelist to its front-matter and line-oriented text representation.
pub fn render_codelist(codelist: &CodelistFile) -> Result<String, SctError> {
    crate::codelist::render_codelist(codelist).map_err(codelist_error)
}

/// Render and write a `.codelist` file.
pub fn write_codelist(codelist: &CodelistFile, path: impl AsRef<Path>) -> Result<(), SctError> {
    crate::codelist::write_codelist(codelist, path.as_ref()).map_err(codelist_error)
}

/// Classify a codelist `includes:` reference as an id, path, or URL.
pub fn parse_include_ref(raw: &str) -> IncludeRef {
    crate::codelist::parse_include_ref(raw)
}

/// Resolve an id or path include reference to a local path.
pub fn resolve_include_path(
    reference: &IncludeRef,
    including_file_dir: impl AsRef<Path>,
    registry: impl AsRef<Path>,
) -> Result<PathBuf, SctError> {
    crate::codelist::resolve_include_path(reference, including_file_dir.as_ref(), registry.as_ref())
        .map_err(codelist_error)
}

/// Resolve a codelist's effective members, including recursively composed lists.
pub fn effective_members_of(
    codelist: &CodelistFile,
    file: impl AsRef<Path>,
    registry: impl AsRef<Path>,
) -> Result<Vec<EffectiveMember>, SctError> {
    crate::codelist::effective_members_of(codelist, file.as_ref(), registry.as_ref())
        .map_err(codelist_error)
}

pub(crate) fn query_refset_memberships(
    conn: &Connection,
    id: &str,
) -> Result<Vec<RefsetMembership>, SctError> {
    let mut stmt = match conn.prepare(
        "SELECT rm.refset_id, COALESCE(c.preferred_term, '(unknown refset)')
         FROM refset_members rm
         LEFT JOIN concepts c ON c.id = rm.refset_id
         WHERE rm.referenced_component_id = ?1
         ORDER BY c.preferred_term",
    ) {
        Ok(stmt) => stmt,
        Err(source) if source.to_string().contains("no such table") => return Ok(Vec::new()),
        Err(source) => return Err(SctError::query(source)),
    };
    let rows = stmt
        .query_map(params![id], |row| {
            Ok(RefsetMembership {
                id: row.get(0)?,
                preferred_term: row.get(1)?,
            })
        })
        .map_err(SctError::query)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(SctError::query)
}

fn parse_json<T>(field: &'static str, value: &str) -> Result<T, SctError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(value).map_err(|source| SctError::invalid_data(field, source))
}

fn sanitise_fts_query(query: &str, literal: bool) -> String {
    let query = query.trim();
    if query.is_empty() {
        return String::new();
    }
    if literal {
        return format!("\"{}\"", query.replace('"', "\"\""));
    }
    let upper = query.to_uppercase();
    let has_operators = query.contains('"')
        || query.contains('*')
        || query.contains('^')
        || upper.contains(" AND ")
        || upper.contains(" OR ")
        || upper.contains(" NOT ");
    if has_operators {
        query.to_string()
    } else {
        format!("\"{}\"", query.replace('"', "\"\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_open_does_not_create_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.db");
        let error = match Snomed::open(&path) {
            Ok(_) => panic!("missing database unexpectedly opened"),
            Err(error) => error,
        };
        assert!(!path.exists());
        assert!(matches!(error, SctError::Open { .. }));
    }

    #[test]
    fn plain_text_search_is_quoted() {
        assert_eq!(
            sanitise_fts_query("heart attack", false),
            "\"heart attack\""
        );
    }

    #[test]
    fn fts_operators_are_preserved() {
        assert_eq!(
            sanitise_fts_query("heart AND attack", false),
            "heart AND attack"
        );
        assert_eq!(sanitise_fts_query("myocardial*", false), "myocardial*");
    }

    #[test]
    fn search_query_whitespace_and_quotes_are_safe() {
        assert_eq!(sanitise_fts_query("  asthma  ", false), "\"asthma\"");
        assert_eq!(sanitise_fts_query("", false), "");
        assert_eq!(sanitise_fts_query("   ", false), "");
        assert_eq!(
            sanitise_fts_query(r#"he said "yes""#, false),
            r#"he said "yes""#
        );
        assert_eq!(
            sanitise_fts_query(r#"he said "yes""#, true),
            r#""he said ""yes""""#
        );
    }

    #[test]
    fn lexical_rank_ties_remain_bounded_and_stable() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (
                 id TEXT NOT NULL,
                 preferred_term TEXT NOT NULL,
                 fsn TEXT NOT NULL,
                 synonyms TEXT NOT NULL,
                 hierarchy TEXT NOT NULL
             );
             CREATE VIRTUAL TABLE concepts_fts USING fts5(
                 preferred_term, fsn, synonyms,
                 content='concepts', content_rowid='rowid'
             );
             INSERT INTO concepts VALUES
                 ('9', 'same', 'same', '[]', 'Clinical finding'),
                 ('1', 'same', 'same', '[]', 'Clinical finding'),
                 ('2', 'same', 'same', '[]', 'Clinical finding');
             INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild');",
        )
        .unwrap();

        let hits = query_search(&conn, SearchOptions::new("same", 2)).unwrap();
        let ids: Vec<_> = hits.iter().map(|hit| hit.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(
            query_search_ids(&conn, SearchOptions::new("same", 2)).unwrap(),
            ids
        );
        for _ in 0..3 {
            assert_eq!(
                query_search(&conn, SearchOptions::new("same", 2))
                    .unwrap()
                    .iter()
                    .map(|hit| hit.id.as_str())
                    .collect::<Vec<_>>(),
                ids
            );
        }

        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT c.id
                 FROM concepts_fts
                 JOIN concepts c ON concepts_fts.rowid = c.rowid
                 WHERE concepts_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .unwrap();
        let plan = stmt
            .query_map(params!["\"same\"", 2], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(plan.iter().all(|step| !step.contains("TEMP B-TREE")));
    }

    #[test]
    fn descendant_limit_is_stable_with_and_without_tct() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (
                 id TEXT NOT NULL,
                 preferred_term TEXT NOT NULL,
                 fsn TEXT NOT NULL
             );
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             INSERT INTO concepts VALUES
                 ('100', 'Root', 'Root'),
                 ('9', 'Same term', 'Same term'),
                 ('1', 'Same term', 'Same term');
             INSERT INTO concept_isa VALUES ('9', '100'), ('1', '100');",
        )
        .unwrap();
        assert_eq!(query_descendants(&conn, "100", 1).unwrap()[0].id, "1");

        conn.execute_batch(
            "CREATE TABLE concept_ancestors (
                 ancestor_id INTEGER NOT NULL,
                 descendant_id INTEGER NOT NULL,
                 depth INTEGER NOT NULL
             );
             INSERT INTO concept_ancestors VALUES (100, 9, 1), (100, 1, 1);
             CREATE INDEX idx_ca_ancestor ON concept_ancestors(ancestor_id);
             CREATE INDEX idx_ca_descendant ON concept_ancestors(descendant_id);
             CREATE UNIQUE INDEX idx_ca_pair
                 ON concept_ancestors(ancestor_id, descendant_id);
             CREATE TABLE concept_ancestors_meta (
                 schema_version INTEGER NOT NULL,
                 include_self INTEGER NOT NULL CHECK (include_self IN (0, 1))
             );
             INSERT INTO concept_ancestors_meta VALUES (1, 0);",
        )
        .unwrap();
        conn.execute_batch(crate::ecl::eval::TCT_INVALIDATION_TRIGGERS_SQL)
            .unwrap();
        assert_eq!(query_descendants(&conn, "100", 1).unwrap()[0].id, "1");
    }

    fn primitive_hierarchy_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (
                 id TEXT NOT NULL,
                 preferred_term TEXT NOT NULL,
                 fsn TEXT NOT NULL,
                 definition_status TEXT NOT NULL
             );
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             INSERT INTO concepts VALUES
                 ('1', 'Root', 'Root', '900000000000074008'),
                 ('2', 'Primitive mid', 'Primitive mid', '900000000000074008'),
                 ('3', 'Defined leaf', 'Defined leaf', '900000000000073002'),
                 ('4', 'Primitive branch A', 'Primitive branch A', '900000000000074008'),
                 ('5', 'Primitive branch B', 'Primitive branch B', '900000000000074008'),
                 ('6', 'Defined multi-parent', 'Defined multi-parent', '900000000000073002');
             INSERT INTO concept_isa VALUES
                 ('2', '1'), ('3', '2'),
                 ('4', '1'), ('5', '1'),
                 ('6', '4'), ('6', '5');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn proximal_primitive_supertypes_prunes_less_specific_ancestors() {
        let conn = primitive_hierarchy_db();
        let result = query_proximal_primitive_supertypes(&conn, "3").unwrap();
        assert_eq!(
            result.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["2"]
        );
    }

    #[test]
    fn proximal_primitive_supertypes_of_a_primitive_concept_is_itself() {
        let conn = primitive_hierarchy_db();
        let result = query_proximal_primitive_supertypes(&conn, "2").unwrap();
        assert_eq!(
            result.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["2"]
        );
    }

    #[test]
    fn proximal_primitive_supertypes_keeps_incomparable_primitives() {
        let conn = primitive_hierarchy_db();
        let result = query_proximal_primitive_supertypes(&conn, "6").unwrap();
        assert_eq!(
            result.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["4", "5"]
        );
    }

    #[test]
    fn proximal_primitive_supertypes_errors_without_definition_status_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (
                 id TEXT NOT NULL, preferred_term TEXT NOT NULL, fsn TEXT NOT NULL
             );
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             INSERT INTO concepts VALUES ('1', 'Root', 'Root');",
        )
        .unwrap();

        let error = query_proximal_primitive_supertypes(&conn, "1").unwrap_err();
        assert!(format!("{error:?}").contains("definition_status"));
    }

    #[test]
    fn proximal_primitive_supertypes_errors_when_data_is_incomplete() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (
                 id TEXT NOT NULL, preferred_term TEXT NOT NULL, fsn TEXT NOT NULL,
                 definition_status TEXT NOT NULL
             );
             CREATE TABLE concept_isa (child_id TEXT NOT NULL, parent_id TEXT NOT NULL);
             INSERT INTO concepts VALUES ('1', 'Root', 'Root', '');",
        )
        .unwrap();

        let error = query_proximal_primitive_supertypes(&conn, "1").unwrap_err();
        assert!(format!("{error:?}").contains("no primitive ancestors"));
    }

    #[test]
    fn proximal_primitive_supertypes_errors_for_missing_concept() {
        let conn = primitive_hierarchy_db();
        let error = query_proximal_primitive_supertypes(&conn, "999").unwrap_err();
        assert!(matches!(error, SctError::ConceptNotFound { id } if id == "999"));
    }
}
