// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The read-only SQLite connection pool behind `sct serve`.
//!
//! Every FHIR operation runs on `spawn_blocking`; this pool opens N warm
//! read-only connections once at startup and lends them out, so a borrowed
//! connection keeps its prepared-statement cache warm across requests and
//! real DB concurrency is bounded to N. Checkout is synchronous - callers
//! are already on a blocking thread. Request-serving paths use
//! [`with_until`](ConnectionPool::with_until) so a queued task stops waiting
//! at the request deadline instead of blocking indefinitely; startup checks
//! use plain [`with`](ConnectionPool::with).

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::time::Instant;

/// No connection became available before the caller's deadline. The caller
/// should answer the client with a timeout rather than keep waiting: the
/// request that holds the connection may legitimately run until *its* own
/// deadline, so an unbounded wait here is unbounded latency.
#[derive(Debug)]
pub struct PoolTimeout;

impl std::fmt::Display for PoolTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no database connection became available before the deadline"
        )
    }
}

impl std::error::Error for PoolTimeout {}

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
    ///
    /// For request-serving paths prefer [`with_until`](Self::with_until),
    /// which stops waiting at a deadline instead of blocking indefinitely.
    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> T) -> T {
        let conn = self.checkout();
        // A guard returns the connection on drop, so a panic in `f` cannot leak
        // it (which would permanently shrink the pool).
        let mut lease = Lease {
            pool: self,
            conn: Some(conn),
        };
        f(lease.conn.as_mut().expect("leased connection present"))
    }

    /// Like [`with`](Self::with) but stops waiting for a connection at
    /// `deadline`. A request whose client has already timed out must not
    /// queue a blocking thread indefinitely and then run its query for a
    /// client that has gone (2026-11-23 serve review, F1): it returns
    /// [`PoolTimeout`] as soon as the deadline passes, and the caller turns
    /// that into the request's timeout response.
    pub fn with_until<T>(
        &self,
        deadline: Instant,
        f: impl FnOnce(&mut Connection) -> T,
    ) -> Result<T, PoolTimeout> {
        let conn = self.checkout_until(deadline)?;
        let mut lease = Lease {
            pool: self,
            conn: Some(conn),
        };
        Ok(f(lease.conn.as_mut().expect("leased connection present")))
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

    fn checkout_until(&self, deadline: Instant) -> Result<Connection, PoolTimeout> {
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(PoolTimeout);
            }
            if let Some(conn) = idle.pop() {
                return Ok(conn);
            }
            // `wait_timeout` wakes on `notify_one` *or* the timeout; the loop
            // re-checks both conditions, so a spurious wakeup or a stale
            // notification cannot miss the deadline.
            let (guard, _timed_out) = self
                .available
                .wait_timeout(idle, remaining)
                .unwrap_or_else(|e| e.into_inner());
            idle = guard;
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

/// Returns the connection to the pool on drop, even if the closure it was
/// leased to panicked.
struct Lease<'p> {
    pool: &'p ConnectionPool,
    conn: Option<Connection>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.checkin(conn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tiny_db(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("pool.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();
        path
    }

    #[test]
    fn with_until_runs_when_a_connection_is_available() {
        let dir = tempfile::tempdir().unwrap();
        let pool = ConnectionPool::open(&tiny_db(&dir), 2, 1024).unwrap();
        let value = pool.with_until(Instant::now() + Duration::from_secs(5), |c| {
            c.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))
                .unwrap()
        });
        assert_eq!(value.unwrap(), 1);
    }

    #[test]
    fn with_until_times_out_when_the_pool_is_exhausted() {
        let dir = tempfile::tempdir().unwrap();
        let pool = ConnectionPool::open(&tiny_db(&dir), 1, 1024).unwrap();
        // Hold the pool's only connection for the whole test.
        let held = pool.checkout();
        let started = Instant::now();
        let result = pool.with_until(Instant::now() + Duration::from_millis(50), |_| {
            panic!("the closure must not run while the pool is exhausted")
        });
        assert!(result.is_err());
        assert!(
            started.elapsed() >= Duration::from_millis(40),
            "the wait should have run until the deadline, not return early"
        );
        // Once the connection is returned, a later caller gets through.
        pool.checkin(held);
        let value = pool.with_until(Instant::now() + Duration::from_secs(5), |c| {
            c.query_row("SELECT x FROM t", [], |r| r.get::<_, i64>(0))
                .unwrap()
        });
        assert_eq!(value.unwrap(), 1);
    }

    #[test]
    fn stale_wakeups_do_not_hide_a_missed_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let pool = ConnectionPool::open(&tiny_db(&dir), 1, 1024).unwrap();
        let _held = pool.checkout();
        // A notification with nobody waiting must not let a later, already
        // overdue waiter slip past the deadline check.
        pool.available.notify_all();
        let result = pool.with_until(Instant::now() - Duration::from_millis(1), |_| {
            panic!("the closure must not run past the deadline")
        });
        assert!(result.is_err());
    }

    #[test]
    fn expired_deadline_does_not_borrow_an_idle_connection() {
        let dir = tempfile::tempdir().unwrap();
        let pool = ConnectionPool::open(&tiny_db(&dir), 1, 1024).unwrap();
        assert!(pool
            .with_until(Instant::now() - Duration::from_millis(1), |_| {
                panic!("expired work must not run")
            })
            .is_err());
        assert_eq!(pool.size(), 1);
    }
}
