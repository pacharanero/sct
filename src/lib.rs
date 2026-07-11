// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! sct - SNOMED CT local-first toolchain.
//!
//! The library crate exposes the building blocks used by the `sct` binary so
//! that integration tests (under `tests/`) and downstream tools can depend on
//! them without going through the CLI.
//!
//! The binary at `src/main.rs` is a thin `clap` wrapper over [`commands`].

pub mod builder;
pub mod commands;
pub mod ecl;
pub mod format;
pub mod index;
pub mod output;
pub mod paths;
pub mod progress;
pub mod provenance;
pub mod rf2;
pub mod schema;
