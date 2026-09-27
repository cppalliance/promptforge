//! The gated-backend test helpers for the scheduler suites: a one-shot gate on
//! the first backend write or append beside the store and observer that open it.
//!
//! The gate no longer forces a conflict - claims are never released during a
//! run, so an unordered second op conflicts however late it starts. It parks
//! the first write-intent op so the cancel-wait suite can cancel a child
//! while its write is still in flight: the cancel lands while the child is
//! parked, whatever the blocking pool's timing. The parked wait is bounded:
//! a cancel that stopped reporting would otherwise strand the run-end drain
//! on the parked op, and the test must fail, never hang.

use std::sync::Condvar;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::*;
use promptforge_vfs::{Entry, ExecId, MemoryBackend, Stat, Vfs, VfsAccess, VfsError, VfsPath};

/// A one-shot gate for the first backend write or append: the first
/// write-intent op the backend serves parks until a [`GateObserver`]
/// opens the gate, so a test's outcome cannot depend on how late that
/// op's blocking-pool thread starts. The cancel-wait suite parks a child
/// until its owner's cancel is observed. The parked wait is bounded: a
/// cancel that stopped reporting would otherwise strand the run-end
/// drain on the parked op, and the test must fail, never hang.
#[derive(Default)]
pub(in super::super) struct StoreGate {
    released: Mutex<bool>,
    release: Condvar,
    taken: AtomicBool,
}

impl StoreGate {
    /// Parks the first caller until the gate opens; later callers pass.
    fn block_first(&self) {
        if self.taken.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut released = self
            .released
            .lock()
            .expect("the gate mutex is not poisoned");
        while !*released {
            let (guard, elapsed) = self
                .release
                .wait_timeout(released, Duration::from_secs(10))
                .expect("the gate mutex is not poisoned");
            released = guard;
            if elapsed.timed_out() {
                // Backstop only: a working claims model opens the gate
                // from the conflict observation long before this.
                break;
            }
        }
    }

    /// Releases the parked op.
    fn open(&self) {
        let mut released = self
            .released
            .lock()
            .expect("the gate mutex is not poisoned");
        *released = true;
        self.release.notify_all();
    }
}

/// Opens the gate when a task is cancelled: the observation fires before
/// the answer that ends the run posts, so the parked op completes ahead
/// of the run-end drain that awaits it. Every observation also forwards
/// to `inner`, so a test can keep its own recorder behind the gate.
pub(in super::super) struct GateObserver {
    gate: Arc<StoreGate>,
    inner: Arc<dyn Observer>,
}

impl GateObserver {
    pub(in super::super) fn new(
        gate: &Arc<StoreGate>,
        inner: Arc<dyn Observer>,
    ) -> Arc<GateObserver> {
        Arc::new(GateObserver {
            gate: Arc::clone(gate),
            inner,
        })
    }
}

impl Observer for GateObserver {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        if matches!(
            event,
            Observation::StoreWriteFailed
                | Observation::StoreAppendFailed
                | Observation::TaskCancelled { .. }
        ) {
            self.gate.open();
        }
        self.inner.observe(execution, section, event);
    }
}

/// A memory backend whose first `write` or `append` parks on the gate, so
/// a test can hold a child's write in flight until its owner's cancel is
/// observed.
struct GatedStore {
    inner: MemoryBackend,
    gate: Arc<StoreGate>,
}

/// A test store mounting a [`GatedStore`] on `gate`.
pub(in super::super) fn gated_store(gate: &Arc<StoreGate>) -> TestStore {
    TestStore::from_vfs(
        VfsRef::builder()
            .mount(
                promptforge_vfs::STORE_MOUNT,
                GatedStore {
                    inner: MemoryBackend::new(),
                    gate: Arc::clone(gate),
                },
            )
            .build(),
    )
}

impl Vfs for GatedStore {
    fn acquire(&mut self, id: ExecId) -> std::result::Result<Box<dyn VfsAccess>, VfsError> {
        Ok(Box::new(GatedAccess {
            inner: self.inner.acquire(id)?,
            gate: Arc::clone(&self.gate),
        }))
    }

    fn release(&mut self, id: ExecId) -> std::result::Result<(), VfsError> {
        self.inner.release(id)
    }
}

struct GatedAccess {
    inner: Box<dyn VfsAccess>,
    gate: Arc<StoreGate>,
}

impl VfsAccess for GatedAccess {
    fn read(&self, path: &VfsPath) -> std::result::Result<Vec<u8>, VfsError> {
        self.inner.read(path)
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> std::result::Result<(), VfsError> {
        self.gate.block_first();
        self.inner.write(path, contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> std::result::Result<(), VfsError> {
        self.gate.block_first();
        self.inner.append(path, contents)
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> std::result::Result<(), VfsError> {
        self.inner.remove(path, recursive)
    }

    fn exists(&self, path: &VfsPath) -> std::result::Result<bool, VfsError> {
        self.inner.exists(path)
    }

    fn glob(&self, pattern: &str) -> std::result::Result<Vec<String>, VfsError> {
        self.inner.glob(pattern)
    }

    fn list(&self, path: &VfsPath) -> std::result::Result<Vec<Entry>, VfsError> {
        self.inner.list(path)
    }

    fn stat(&self, path: &VfsPath) -> std::result::Result<Stat, VfsError> {
        self.inner.stat(path)
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> std::result::Result<(), VfsError> {
        self.inner.mkdir(path, recursive)
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> std::result::Result<(), VfsError> {
        self.inner.rename(from, to)
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> std::result::Result<(), VfsError> {
        self.inner.copy(from, to)
    }
}
