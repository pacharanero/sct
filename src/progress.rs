// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared progress-bar styling for the long-running build commands.
//!
//! Three shapes, one house style:
//!   - [`spinner`]  - an indeterminate phase (e.g. reading from stdin, where no
//!     total is knowable).
//!   - [`byte_bar`] - streaming a file of known size; pair it with
//!     [`ProgressBar::wrap_read`] so the bar advances as bytes are consumed and
//!     an ETA is derived from the byte rate.
//!   - [`count_bar`] - a loop over a known number of items; advance it with
//!     [`ProgressBar::inc`] for a per-item ETA.
//!
//! All three draw to stderr and auto-suppress their animation when stderr is not
//! a terminal (piped, redirected, or under CI), so machine-readable output on
//! stdout is never polluted.

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::borrow::Cow;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Duration;

const TICK: Duration = Duration::from_millis(120);

/// A spinner for a phase with no known total.
pub fn spinner(msg: impl Into<Cow<'static, str>>) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} [{elapsed_precise}] {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(TICK);
    pb.set_message(msg);
    pb
}

/// A byte-oriented bar with an ETA, for streaming a file of known size.
///
/// Advance it by wrapping the underlying reader with `pb.wrap_read(file)`; the
/// bar then ticks automatically as bytes flow through. See [`ndjson_reader`].
pub fn byte_bar(total_bytes: u64) -> ProgressBar {
    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.cyan} [{elapsed_precise}] [{bar:30.cyan/blue}] \
                 {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta}) {msg}",
            )
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.enable_steady_tick(TICK);
    pb
}

/// A count-oriented bar with an ETA, for a loop over a known number of items.
///
/// Advance it with `pb.inc(n)` (or `pb.set_position(i)`).
pub fn count_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.cyan} [{elapsed_precise}] [{bar:30.cyan/blue}] \
                 {human_pos}/{human_len} ({per_sec}, ETA {eta}) {msg}",
            )
            .unwrap()
            .progress_chars("=>-"),
    );
    pb.enable_steady_tick(TICK);
    pb
}

/// Open an NDJSON input (`path`, or `-` for stdin) behind a progress bar.
///
/// For a real file the bar is byte-oriented with an ETA and advances as the
/// returned reader is consumed (via [`ProgressBar::wrap_read`]); for stdin, or
/// a file whose length can't be determined, it falls back to a [`spinner`].
/// Callers set their own message with `pb.set_message(..)` and finish it with
/// `pb.finish_with_message(..)`.
pub fn ndjson_reader(path: &Path) -> Result<(BufReader<Box<dyn Read>>, ProgressBar)> {
    if path.as_os_str() == "-" {
        let reader: Box<dyn Read> = Box::new(std::io::stdin());
        return Ok((BufReader::new(reader), spinner("")));
    }
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    match file.metadata().map(|m| m.len()) {
        Ok(len) if len > 0 => {
            let pb = byte_bar(len);
            let reader: Box<dyn Read> = Box::new(pb.wrap_read(file));
            Ok((BufReader::new(reader), pb))
        }
        _ => {
            let reader: Box<dyn Read> = Box::new(file);
            Ok((BufReader::new(reader), spinner("")))
        }
    }
}
