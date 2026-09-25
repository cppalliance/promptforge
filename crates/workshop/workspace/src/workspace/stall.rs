//! A test-only write stall behind `test-fixtures`: it holds the next
//! [`Workspace::write_file`] on the blocking pool until the test releases
//! it, so the route deadline's 408 is reachable deterministically without
//! a slow real write. nextest runs one test per process, so the per-write
//! rendezvous held on the [`Workspace`] cannot leak across tests.
//!
//! [`Workspace`]: super::Workspace

use std::sync::mpsc;
use std::sync::{Mutex, PoisonError};

use super::Workspace;

/// The armed rendezvous for one stalled write: `release` blocks the
/// writer, `done` reports once the write has landed.
pub(crate) struct WriteStall {
    armed: Mutex<Option<WriteRendezvous>>,
}

impl WriteStall {
    pub(crate) fn new() -> Self {
        Self {
            armed: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for WriteStall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WriteStall").finish_non_exhaustive()
    }
}

/// One armed stall's channels: the writer blocks on `release` and sends on
/// `done` once the write has landed. Neither is `Debug`, so the struct
/// stays plain data.
struct WriteRendezvous {
    release: mpsc::Receiver<()>,
    done: mpsc::Sender<()>,
}

/// Signals that a released write has landed when dropped at the end of
/// [`Workspace::write_file`].
pub(crate) struct WriteDone(mpsc::Sender<()>);

impl Drop for WriteDone {
    fn drop(&mut self) {
        // The write may have been abandoned by the route deadline and never
        // cancelled; a failed send just means the test stopped listening.
        let _ = self.0.send(());
    }
}

/// The test's end of an armed stall: releases the stalled write, then
/// reports once the write has landed on disk.
#[must_use]
pub struct WriteStallHandle {
    release: mpsc::Sender<()>,
    done: mpsc::Receiver<()>,
}

impl std::fmt::Debug for WriteStallHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WriteStallHandle")
            .finish_non_exhaustive()
    }
}

impl WriteStallHandle {
    /// Lets the stalled write proceed.
    pub fn release(&self) {
        let _ = self.release.send(());
    }

    /// Blocks until the released write has landed on disk.
    pub fn await_completion(&self) {
        let _ = self.done.recv();
    }
}

impl Workspace {
    /// Arms a stall on the next write: the write blocks on the blocking
    /// pool until the returned handle is released, after which
    /// [`WriteStallHandle::await_completion`] reports the write landed.
    pub fn stall_next_write_for_test(&self) -> WriteStallHandle {
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        *self
            .stall
            .armed
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(WriteRendezvous {
            release: release_rx,
            done: done_tx,
        });
        WriteStallHandle {
            release: release_tx,
            done: done_rx,
        }
    }

    /// Blocks on the armed stall, if any, and returns a guard that reports
    /// completion on drop. Runs on the blocking pool inside
    /// [`Workspace::write_file`], where a blocking wait is expected.
    pub(crate) fn stall_wait(&self) -> Option<WriteDone> {
        let rendezvous = self
            .stall
            .armed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()?;
        // The route deadline may abandon this write while it waits; the
        // blocking-pool task is not cancellable, so it stays here until the
        // test releases it, then lands the write.
        let _ = rendezvous.release.recv();
        Some(WriteDone(rendezvous.done))
    }
}
