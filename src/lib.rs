// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! sct - SNOMED CT local-first toolchain.
//!
//! The library crate exposes the building blocks used by the `sct` binary so
//! that integration tests (under `tests/`) and downstream tools can depend on
//! them without going through the CLI.
//!
//! With the `cli` feature, the binary at `src/main.rs` is a thin `clap` wrapper over the command modules.

#[cfg(feature = "cli")]
pub mod builder;
mod codelist;
#[cfg(feature = "cli")]
pub mod commands;
pub mod ecl;
#[cfg(feature = "cli")]
pub mod format;
pub mod index;
mod mapping;
#[cfg(feature = "cli")]
pub mod output;
#[cfg(feature = "cli")]
pub mod paths;
#[cfg(feature = "cli")]
pub mod progress;
pub mod provenance;
mod refset;
#[cfg(feature = "cli")]
pub mod rf2;
pub mod schema;
pub mod sdk;
