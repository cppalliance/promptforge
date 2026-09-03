//! Download progress reporting into progress-tree leaves.
//!
//! The library never chooses a presentation: the owning process renders the
//! hub (the gateway binary draws indicatif bars on a TTY, tracing lines
//! otherwise).

use std::sync::atomic::{AtomicU64, Ordering};

use shared_progress::ProgressHandle;

/// Progress updates for a single HTTP blob download.
pub trait DownloadProgress: Send {
    /// Records the total length in bytes, when the server sent one.
    fn set_len(&self, total: Option<u64>);
    /// Adds `n` downloaded bytes to the running total.
    fn inc(&self, n: u64);
    /// Marks the download complete.
    fn finish(&self);
    /// Marks the download abandoned before completion.
    fn abandon(&self);
}

/// A [`DownloadProgress`] that discards every callback, for callers with no
/// progress tree.
pub(super) struct NoopProgress;

impl DownloadProgress for NoopProgress {
    fn set_len(&self, _total: Option<u64>) {}

    fn inc(&self, _n: u64) {}

    fn finish(&self) {}

    fn abandon(&self) {}
}

/// A [`DownloadProgress`] that reports byte counts into a progress-tree leaf.
///
/// The leaf's fraction is driven by `set_units(downloaded, total)` once
/// [`DownloadProgress::set_len`] supplies the Content-Length; without a
/// length the leaf stays indeterminate until [`DownloadProgress::finish`].
#[derive(Debug)]
pub struct TreeProgress {
    handle: ProgressHandle,
    total: AtomicU64,
    downloaded: AtomicU64,
}

impl TreeProgress {
    /// Creates a reporter that feeds `handle` from the download's byte counts.
    #[must_use]
    pub fn new(handle: ProgressHandle) -> Self {
        Self {
            handle,
            total: AtomicU64::new(0),
            downloaded: AtomicU64::new(0),
        }
    }
}

impl DownloadProgress for TreeProgress {
    fn set_len(&self, total: Option<u64>) {
        self.total.store(total.unwrap_or(0), Ordering::Relaxed);
    }

    fn inc(&self, n: u64) {
        let downloaded = self.downloaded.fetch_add(n, Ordering::Relaxed) + n;
        let total = self.total.load(Ordering::Relaxed);
        if total > 0 {
            self.handle.set_units(downloaded, total);
        }
    }

    fn finish(&self) {
        self.handle.complete();
    }

    fn abandon(&self) {
        // The handle vocabulary has no failure terminal; the operation owner
        // carries failure through its own exit path, so the leaf completes.
        self.handle.complete();
    }
}
