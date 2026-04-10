//! sct — SNOMED CT local-first toolchain.
//!
//! The library crate exposes the building blocks used by the `sct` binary so
//! that integration tests (under `tests/`) and downstream tools can depend on
//! them without going through the CLI.
//!
//! The binary at `src/main.rs` is a thin `clap` wrapper over [`commands`].

pub mod builder;
pub mod commands;
pub mod format;
pub mod rf2;
pub mod schema;
