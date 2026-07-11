// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small bounded pool of warm, read-only SQLite connections for `sct serve`.
//!
//! Every FHIR operation runs on `spawn_blocking`, and the original design
//! opened a fresh `Connection` per request - so each request paid a file open,
//! a pragma round, and a full SQL (re)compile, and the `prepare_cached` calls
//! in `ops.rs` never actually cached anything (their cache lives on a
//! connection that was thrown away immediately). Under concurrency the
//! per-request `Connection::open` also contends on SQLite's shared WAL index,
//! so throughput peaked at a handful of clients and then *fell* as load climbed.
//!
//! This pool opens N read-only connections once at startup and lends them out.
//! A borrowed connection keeps its prepared-statement cache warm across
//! requests, and real DB concurrency is bounded to N (chosen ~= the useful
//! parallelism), which turns the throughput curve from "peak then decline" into
//! "climb then plateau". Checkout is synchronous - callers are already on a
//! blocking thread - and blocks briefly when every connection is busy, giving
//! natural backpressure rather than unbounded connection growth.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Condvar, Mutex};

/// A fixed-size pool of read-only [`Connection`]s.
pub struct ConnectionPool {
    idle: Mutex<Vec<Connection>>,
    available: Condvar,
}

impl ConnectionPool {
    /// Open `size` read-only connections to `path`, each with a `cache_kib`
    /// private page cache. Memory-mapped I/O is configured centrally by
    /// [`crate::commands::open_db_readonly`], so all N connections share the
    /// OS page cache for the mapped database rather than each buffering it.
    pub fn open(path: &Path, size: usize, cache_kib: u32) -> Result<Self> {
        let size = size.max(1);
        let mut idle = Vec::with_capacity(size);
        for _ in 0..size {
            idle.push(
                crate::commands::open_db_readonly(path, Some(cache_kib))
                    .context("opening pooled read-only connection")?,
            );
        }
        Ok(Self {
            idle: Mutex::new(idle),
            available: Condvar::new(),
        })
    }

    /// Number of connections the pool manages.
    pub fn size(&self) -> usize {
        self.idle.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Borrow a connection, run `f`, and return the connection to the pool -
    /// even if `f` panics. Blocks while every connection is checked out.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        let conn = self.checkout();
        // A guard returns the connection on drop, so a panic in `f` cannot leak
        // it (which would permanently shrink the pool).
        let lease = Lease {
            pool: self,
            conn: Some(conn),
        };
        f(lease.conn.as_ref().expect("leased connection present"))
    }

    fn checkout(&self) -> Connection {
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(conn) = idle.pop() {
                return conn;
            }
            idle = self.available.wait(idle).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn checkin(&self, conn: Connection) {
        self.idle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(conn);
        self.available.notify_one();
    }
}

/// Returns its connection to the pool when dropped (including on unwind).
struct Lease<'a> {
    pool: &'a ConnectionPool,
    conn: Option<Connection>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.checkin(conn);
        }
    }
}
