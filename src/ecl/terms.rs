// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Verify a `|term|` annotation on an ECL concept reference against the
//! concept's actual descriptions (`R69`, `spec/roadmap.md`).
//!
//! The SNOMED CT Compositional Grammar defines a concept reference as
//! `conceptId [ws "|" ws term ws "|"]`, and requires the term to be *some*
//! description of the identified concept - any description, in any dialect,
//! not specifically the preferred term. A mismatched annotation is malformed
//! input with a nasty twist: it exists so a human can sanity-check an
//! expression by reading it, so a wrong identifier still reads as correct.
//!
//! This check is advisory, not an evaluation failure: a concept's preferred
//! term legitimately changes between releases, and a saved codelist or ECL
//! expression must not break on upgrade. It warns on stderr and always lets
//! the expression evaluate.

use rusqlite::{Connection, OptionalExtension};

/// Strip a trailing ` (semantic tag)` from an FSN. A local copy rather than
/// `crate::builder::strip_semantic_tag`: `builder` is gated behind the `cli`
/// feature (Python bindings build without it), while this module is reached
/// from the core evaluator, which is not.
fn strip_semantic_tag(fsn: &str) -> &str {
    match fsn.rfind(" (") {
        Some(pos) => &fsn[..pos],
        None => fsn,
    }
}

/// Check `term` - the annotation supplied for concept `id` - against the
/// database's record of that concept's descriptions, warning on stderr if it
/// matches none of them. Best-effort: any query failure (missing table,
/// legacy schema) is swallowed rather than propagated, since a diagnostic
/// must never turn into a hard failure for an otherwise-valid expression.
pub(crate) fn check_term_annotation(conn: &Connection, id: &str, term: &str) {
    let Ok(Some(descriptions)) = describe_concept(conn, id) else {
        // Concept absent from this database (or the query itself failed) -
        // nothing to verify the annotation against.
        return;
    };
    if descriptions.matches(term) {
        return;
    }

    let mut message = format!(
        "warning: |term| annotation for concept {id} does not match: supplied {term:?}, \
         concept {id} is {actual:?}",
        actual = descriptions.preferred_term,
    );
    if descriptions.fsn_clean != descriptions.preferred_term {
        message.push_str(&format!(" (FSN: {})", descriptions.fsn_clean));
    }
    if let Ok(Some((other_id, other_term))) = find_concept_with_term(conn, term, id) {
        message.push_str(&format!(
            " -- note: {term:?} is the term for concept {other_id} ({other_term}); \
             check for a transposed identifier",
        ));
    }
    eprintln!("{message}");
}

/// The descriptions of one concept relevant to term verification: the FSN (as
/// stored, semantic tag included, and with it stripped), the preferred term,
/// and every synonym. Any of these counts as "a description... in any
/// dialect" per the grammar's requirement.
struct ConceptDescriptions {
    fsn: String,
    fsn_clean: String,
    preferred_term: String,
    synonyms: Vec<String>,
}

impl ConceptDescriptions {
    /// Whether `term` matches (case-insensitively, ignoring leading/trailing
    /// whitespace) any description of this concept.
    fn matches(&self, term: &str) -> bool {
        let term = term.trim();
        [
            self.fsn.as_str(),
            self.fsn_clean.as_str(),
            self.preferred_term.as_str(),
        ]
        .into_iter()
        .chain(self.synonyms.iter().map(String::as_str))
        .any(|candidate| candidate.eq_ignore_ascii_case(term))
    }
}

fn describe_concept(conn: &Connection, id: &str) -> anyhow::Result<Option<ConceptDescriptions>> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT fsn, preferred_term, synonyms FROM concepts WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((fsn, preferred_term, synonyms_json)) = row else {
        return Ok(None);
    };
    let synonyms: Vec<String> = serde_json::from_str(&synonyms_json).unwrap_or_default();
    let fsn_clean = strip_semantic_tag(&fsn).to_string();
    Ok(Some(ConceptDescriptions {
        fsn,
        fsn_clean,
        preferred_term,
        synonyms,
    }))
}

/// Look for a concept *other than `exclude_id`* whose FSN, preferred term, or
/// a synonym exactly matches `term` - the "transposed identifier" case, where
/// the supplied annotation is a real term, just for the wrong concept.
/// Best-effort: uses the FTS5 index to find candidates cheaply and confirms
/// an exact match in Rust, so a full-release database is not scanned row by
/// row. Returns `Ok(None)` (rather than erring) when `concepts_fts` is
/// absent, e.g. a hand-built or pre-FTS database.
fn find_concept_with_term(
    conn: &Connection,
    term: &str,
    exclude_id: &str,
) -> anyhow::Result<Option<(String, String)>> {
    if !has_fts_table(conn) {
        return Ok(None);
    }
    let term = term.trim();
    if term.is_empty() {
        return Ok(None);
    }
    let fts_query = format!("\"{}\"", term.replace('"', "\"\""));
    let mut stmt = conn.prepare(
        "SELECT c.id, c.fsn, c.preferred_term, c.synonyms
         FROM concepts_fts
         JOIN concepts c ON concepts_fts.rowid = c.rowid
         WHERE concepts_fts MATCH ?1
         LIMIT 25",
    )?;
    let rows = stmt
        .query_map([fts_query.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (candidate_id, fsn, preferred_term, synonyms_json) in rows {
        if candidate_id == exclude_id {
            continue;
        }
        let synonyms: Vec<String> = serde_json::from_str(&synonyms_json).unwrap_or_default();
        let fsn_clean = strip_semantic_tag(&fsn).to_string();
        let descriptions = ConceptDescriptions {
            fsn,
            fsn_clean,
            preferred_term: preferred_term.clone(),
            synonyms,
        };
        if descriptions.matches(term) {
            return Ok(Some((candidate_id, preferred_term)));
        }
    }
    Ok(None)
}

fn has_fts_table(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type IN ('table', 'view') AND name = 'concepts_fts'",
        [],
        |_| Ok(()),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (
                 id TEXT PRIMARY KEY,
                 fsn TEXT NOT NULL,
                 preferred_term TEXT NOT NULL,
                 synonyms TEXT
             );
             INSERT INTO concepts VALUES
                 ('73211009', 'Diabetes mellitus (disorder)', 'Diabetes mellitus',
                  '[\"Sugar diabetes\"]'),
                 ('22298006', 'Myocardial infarction (disorder)', 'Myocardial infarction',
                  '[\"Heart attack\"]');
             CREATE VIRTUAL TABLE concepts_fts USING fts5(
                 id, preferred_term, synonyms, fsn,
                 content='concepts', content_rowid='rowid'
             );
             INSERT INTO concepts_fts(concepts_fts) VALUES('rebuild');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn matching_preferred_term_is_silent() {
        let conn = test_db();
        // No assertion possible on stderr directly; verifying `matches` -
        // the function the warning path gates on - is silent is the
        // meaningful unit-level check. End-to-end silence is covered by the
        // CLI test.
        let descriptions = describe_concept(&conn, "73211009").unwrap().unwrap();
        assert!(descriptions.matches("Diabetes mellitus"));
        assert!(descriptions.matches("DIABETES MELLITUS")); // case-insensitive
        assert!(descriptions.matches("Sugar diabetes")); // synonym
        assert!(descriptions.matches("Diabetes mellitus (disorder)")); // FSN with tag
    }

    #[test]
    fn wholly_wrong_term_is_not_a_match() {
        let conn = test_db();
        let descriptions = describe_concept(&conn, "73211009").unwrap().unwrap();
        assert!(!descriptions.matches("Hyperthyroidism"));
    }

    #[test]
    fn unknown_concept_has_no_descriptions_to_check() {
        let conn = test_db();
        assert!(describe_concept(&conn, "999999999").unwrap().is_none());
    }

    #[test]
    fn transposed_identifier_is_found_by_term() {
        let conn = test_db();
        let (id, term) = find_concept_with_term(&conn, "Myocardial infarction", "73211009")
            .unwrap()
            .unwrap();
        assert_eq!(id, "22298006");
        assert_eq!(term, "Myocardial infarction");
    }

    #[test]
    fn transposed_identifier_search_excludes_the_queried_concept_itself() {
        let conn = test_db();
        // The concept's own term must not be reported back as "some other
        // concept" when the caller already knows it mismatches.
        assert!(
            find_concept_with_term(&conn, "Diabetes mellitus", "73211009")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn missing_fts_table_is_handled_gracefully() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE concepts (
                 id TEXT PRIMARY KEY,
                 fsn TEXT NOT NULL,
                 preferred_term TEXT NOT NULL,
                 synonyms TEXT
             );
             INSERT INTO concepts VALUES
                 ('73211009', 'Diabetes mellitus (disorder)', 'Diabetes mellitus', '[]');",
        )
        .unwrap();
        assert!(find_concept_with_term(&conn, "Diabetes mellitus", "1")
            .unwrap()
            .is_none());
        // check_term_annotation must not panic without an FTS table either.
        check_term_annotation(&conn, "73211009", "Something else entirely");
    }
}
