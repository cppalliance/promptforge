//! Bounded, prioritized file logging for the PromptForge gateway.
//!
//! [`LogRuntime`] owns one worker thread that drains a bounded priority
//! queue into a rotated `gateway.log`; [`LogWriter`] adapts the queue to
//! `tracing-subscriber`'s field formatter and `MakeWriter` so the binary's
//! fmt layer redacts classified fields before formatting, then enqueues
//! byte-bounded events instead of blocking producer threads on disk.
//!
//! The crate never installs the global subscriber, never reads the
//! environment or the home directory, and never sees Gateway configuration:
//! the caller passes the state directory in through [`LogConfig`] and
//! composes the subscriber itself.

mod config;
mod error;
mod queue;
mod redact;
mod runtime;
mod worker;
mod writer;

pub use crate::config::LogConfig;
pub use crate::error::LogError;
pub use crate::runtime::LogRuntime;
pub use crate::writer::LogWriter;
// Forced onto the public surface by E0446: `MakeWriter::Writer` cannot
// name a private type. Hidden and not part of the API contract.
#[doc(hidden)]
pub use crate::writer::LogEventWriter;

#[cfg(test)]
pub(crate) mod fault_injection {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
    use std::time::Duration;

    #[derive(Debug)]
    pub(crate) struct ReleasePoint {
        entered: SyncSender<()>,
        release: Receiver<()>,
    }

    #[derive(Debug)]
    pub(crate) struct ReleaseControl {
        entered: Receiver<()>,
        release: SyncSender<()>,
    }

    pub(crate) fn release_point() -> (ReleasePoint, ReleaseControl) {
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        (
            ReleasePoint {
                entered: entered_tx,
                release: release_rx,
            },
            ReleaseControl {
                entered: entered_rx,
                release: release_tx,
            },
        )
    }

    impl ReleasePoint {
        pub(crate) fn wait(self) {
            self.entered
                .send(())
                .expect("report that the fault boundary was reached");
            self.release
                .recv()
                .expect("wait for the fault boundary release");
        }
    }

    impl ReleaseControl {
        pub(crate) fn wait_until_reached(&self, timeout: Duration) -> Result<(), RecvTimeoutError> {
            self.entered.recv_timeout(timeout)
        }

        pub(crate) fn release(self) {
            self.release.send(()).expect("release the fault boundary");
        }
    }

    #[derive(Debug, Default)]
    pub(crate) struct InvocationProbe(AtomicBool);

    impl InvocationProbe {
        pub(crate) fn record(&self) {
            self.0.store(true, Ordering::SeqCst);
        }

        pub(crate) fn was_recorded(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }
}

#[cfg(test)]
pub(crate) mod allocation_tracking {
    use std::cell::Cell;

    thread_local! {
        static ENABLED: Cell<bool> = const { Cell::new(false) };
        static MAX_REQUEST: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn record(size: usize) {
        ENABLED.with(|enabled| {
            if enabled.get() {
                MAX_REQUEST.with(|maximum| maximum.set(maximum.get().max(size)));
            }
        });
    }

    pub(crate) struct AllocationTracker {
        active: bool,
    }

    impl AllocationTracker {
        pub(crate) fn start() -> Self {
            ENABLED.with(|enabled| {
                assert!(!enabled.replace(true), "allocation tracking is not nested");
            });
            MAX_REQUEST.with(|maximum| maximum.set(0));
            Self { active: true }
        }

        pub(crate) fn finish(mut self) -> usize {
            self.active = false;
            ENABLED.with(|enabled| enabled.set(false));
            MAX_REQUEST.with(Cell::get)
        }
    }

    impl Drop for AllocationTracker {
        fn drop(&mut self) {
            if self.active {
                let _ = ENABLED.try_with(|enabled| enabled.set(false));
            }
        }
    }
}
