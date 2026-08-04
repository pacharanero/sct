// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared stdin batch handling for read commands.

use anyhow::{Context, Result};
use serde::Serialize;
use std::io::{BufRead, Read};

/// Fail-closed batches retain all results before stdout, so cap their inputs to
/// keep valid but hostile pipelines from exhausting process memory.
pub(crate) const MAX_BATCH_ITEMS: usize = 10_000;
pub(crate) const MAX_BATCH_RESULTS: usize = 100_000;
const MAX_BATCH_LINE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) enum LineMode {
    Whole,
    FirstToken,
}

#[derive(Debug, Serialize)]
pub(crate) struct BatchItem<T> {
    pub input: String,
    pub result: T,
}

impl<T> BatchItem<T> {
    pub(crate) fn new(input: String, result: T) -> Self {
        Self { input, result }
    }
}

pub(crate) struct ResultBudget {
    retained: usize,
}

impl ResultBudget {
    pub(crate) fn new() -> Self {
        Self { retained: 0 }
    }

    /// Cap one query one row beyond the remaining budget so callers can detect
    /// overflow without materialising an unbounded result first.
    pub(crate) fn query_limit(&self, requested: Option<u32>) -> u32 {
        let detection_limit = (MAX_BATCH_RESULTS - self.retained + 1) as u32;
        requested.unwrap_or(detection_limit).min(detection_limit)
    }

    pub(crate) fn retain(&mut self, count: usize, label: &str) -> Result<()> {
        anyhow::ensure!(
            count <= MAX_BATCH_RESULTS - self.retained,
            "{label} batch cannot retain more than {MAX_BATCH_RESULTS} results; use a smaller --limit or fewer inputs"
        );
        self.retained += count;
        Ok(())
    }
}

pub(crate) fn read_stdin(mode: LineMode, label: &str) -> Result<Vec<String>> {
    read_lines(std::io::stdin().lock(), mode, label, Some(MAX_BATCH_ITEMS))
}

pub(crate) fn read_stdin_limited(
    mode: LineMode,
    label: &str,
    maximum: usize,
) -> Result<Vec<String>> {
    read_lines(std::io::stdin().lock(), mode, label, Some(maximum))
}

fn read_lines(
    mut reader: impl BufRead,
    mode: LineMode,
    label: &str,
    maximum: Option<usize>,
) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut line_number = 0usize;
    loop {
        let mut line = String::new();
        let bytes = reader
            .by_ref()
            .take((MAX_BATCH_LINE_BYTES + 1) as u64)
            .read_line(&mut line)
            .with_context(|| format!("reading {label} from stdin at line {}", line_number + 1))?;
        if bytes == 0 {
            break;
        }
        line_number += 1;
        anyhow::ensure!(
            line.len() <= MAX_BATCH_LINE_BYTES,
            "{label} at line {line_number} exceeds {MAX_BATCH_LINE_BYTES} bytes"
        );
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = match mode {
            LineMode::Whole => trimmed,
            LineMode::FirstToken if trimmed.starts_with('#') => continue,
            LineMode::FirstToken => trimmed.split_whitespace().next().unwrap_or_default(),
        };
        if !value.is_empty() {
            anyhow::ensure!(
                maximum.is_none_or(|maximum| values.len() < maximum),
                "{label} batch cannot exceed {} entries",
                maximum.expect("checked as present")
            );
            values.push(value.to_string());
        }
    }
    anyhow::ensure!(!values.is_empty(), "no {label} received on stdin");
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn whole_lines_preserve_queries_and_order() {
        let values = read_lines(
            Cursor::new(" heart attack \r\n\n#literal query\nheart attack\n"),
            LineMode::Whole,
            "queries",
            None,
        )
        .unwrap();
        assert_eq!(values, ["heart attack", "#literal query", "heart attack"]);
    }

    #[test]
    fn first_token_lines_accept_piped_ids_and_comments() {
        let values = read_lines(
            Cursor::new("# comment\n22298006 |Myocardial infarction|\n\n46635009\textra\n"),
            LineMode::FirstToken,
            "codes",
            None,
        )
        .unwrap();
        assert_eq!(values, ["22298006", "46635009"]);
    }

    #[test]
    fn empty_input_is_an_error() {
        let error = read_lines(
            Cursor::new("\n # comment\n"),
            LineMode::FirstToken,
            "codes",
            None,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "no codes received on stdin");
    }

    #[test]
    fn bounded_batches_fail_before_retaining_excess_entries() {
        let error = read_lines(
            Cursor::new("one\ntwo\nthree\n"),
            LineMode::Whole,
            "queries",
            Some(2),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "queries batch cannot exceed 2 entries");
    }

    #[test]
    fn default_batch_cap_is_finite() {
        let input = "value\n".repeat(MAX_BATCH_ITEMS + 1);
        let error = read_lines(
            Cursor::new(input),
            LineMode::Whole,
            "queries",
            Some(MAX_BATCH_ITEMS),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("queries batch cannot exceed {MAX_BATCH_ITEMS} entries")
        );
    }

    #[test]
    fn result_budget_caps_queries_and_rejects_cumulative_overflow() {
        let mut budget = ResultBudget::new();
        assert_eq!(budget.query_limit(None), (MAX_BATCH_RESULTS + 1) as u32);
        budget.retain(MAX_BATCH_RESULTS, "query").unwrap();
        assert_eq!(budget.query_limit(None), 1);
        let error = budget.retain(1, "query").unwrap_err();
        assert!(error.to_string().contains("cannot retain more than"));
    }

    #[test]
    fn oversized_line_is_rejected_before_it_is_retained() {
        let input = "x".repeat(MAX_BATCH_LINE_BYTES + 1);
        let error = read_lines(Cursor::new(input), LineMode::Whole, "queries", None).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("queries at line 1 exceeds {MAX_BATCH_LINE_BYTES} bytes")
        );
    }
}
