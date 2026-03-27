//! Canonical per-concept record type.
//!
//! This is the stable public interface between the NDJSON producer (`sct ndjson`)
//! and all downstream consumers (`sct sqlite`, `sct parquet`, `sct markdown`, `sct mcp`).
//!
//! The format is versioned with `schema_version` so consumers can detect
//! incompatible format changes at parse time.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Current NDJSON schema version. Increment when the record structure changes
/// in a backward-incompatible way.
pub const SCHEMA_VERSION: u32 = 1;

/// A lightweight reference to another concept (used in parents and attributes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptRef {
    pub id: String,
    pub fsn: String,
}

/// The per-concept JSON record written to the NDJSON artefact.
///
/// One record per line, sorted by `id` (ascending numeric SCTID).
#[derive(Debug, Serialize, Deserialize)]
pub struct ConceptRecord {
    pub id: String,
    pub fsn: String,
    pub preferred_term: String,
    pub synonyms: Vec<String>,
    pub hierarchy: String,
    pub hierarchy_path: Vec<String>,
    pub parents: Vec<ConceptRef>,
    pub children_count: usize,
    pub active: bool,
    pub module: String,
    pub effective_time: String,
    pub attributes: IndexMap<String, Vec<ConceptRef>>,
    pub schema_version: u32,
}
