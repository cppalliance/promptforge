//! Concurrent fanout: per-arm start/terminal accounting, store writes across
//! arms, and the propagated arm-failure error contract.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use promptforge_core::execute::RunErrorKind;
use shared_vfs::{Entry, ExecId, MemoryBackend, Stat, Vfs, VfsAccess, VfsError, VfsPath, VfsRef};

use super::support::{Record, run_fixture};

const FANOUT_BASIC_EXECUTION: &str = "fixture-fanout-basic";
const FANOUT_EPILOG_EXECUTION: &str = "fixture-fanout-epilog";
const FANOUT_STORE_EXECUTION: &str = "fixture-fanout-store";
const FANOUT_FAILURE_EXECUTION: &str = "fixture-fanout-failure";
const FANOUT_CROSS_ARM_EXECUTION: &str = "fixture-fanout-cross-arm-append";

const FANOUT_BASIC: &str = include_str!("../prompts/execution/fanout-basic.md");
const FANOUT_EPILOG: &str = include_str!("../prompts/execution/fanout-epilog.md");
const FANOUT_STORE_WRITES: &str = include_str!("../prompts/execution/fanout-store-writes.md");
const FANOUT_ARM_FAILURE: &str = include_str!("../prompts/execution/fanout-arm-failure.md");
const FANOUT_CROSS_ARM_APPEND: &str =
    include_str!("../prompts/execution/fanout-cross-arm-append.md");

/// The worker-template section name both fanout arms execute under. The
/// observation stream keys arm events by this section, not by `sys.index`
/// (which the runtime injects only into arm Lua), so the exact per-arm index
/// pairing is proven by the arms' index-bearing result rather than the event
/// stream.
const WORKER_SECTION: &str = "Worker";

/// Asserts the worker section emitted exactly one start and one success per arm
/// and no other arm terminal (failed, cancelled, exhausted, or the legacy
/// generic finished).
fn assert_two_arms_all_succeeded(records: &[Record]) {
    let events: Vec<&str> = records
        .iter()
        .filter(|record| {
            record.section == WORKER_SECTION && record.detail.starts_with("Fanout arm ")
        })
        .map(|record| record.detail.as_str())
        .collect();
    let started = events
        .iter()
        .filter(|detail| **detail == "Fanout arm started")
        .count();
    let succeeded = events
        .iter()
        .filter(|detail| **detail == "Fanout arm succeeded")
        .count();
    assert_eq!(
        started, 2,
        "two arms must start under the worker section: {events:?}"
    );
    assert_eq!(
        succeeded, 2,
        "two arms must succeed under the worker section: {events:?}"
    );
    assert_eq!(
        events.len(),
        started + succeeded,
        "each arm must pair one start with one success and emit no failed, cancelled, or exhausted event: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_basic_two_items_prologue_return() {
    let run = run_fixture(
        FANOUT_BASIC,
        "execution/fanout-basic.md",
        FANOUT_BASIC_EXECUTION,
        "",
        None,
    )
    .await;
    let result = run
        .result
        .expect("the fanout basic fixture must execute offline");

    // The index-bearing output pins each arm's `item .. '-' .. sys.index`.
    assert_eq!(result, "alpha-1\nbeta-2");
    assert_two_arms_all_succeeded(&run.recorder.records());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_epilog_two_items() {
    let run = run_fixture(
        FANOUT_EPILOG,
        "execution/fanout-epilog.md",
        FANOUT_EPILOG_EXECUTION,
        "",
        None,
    )
    .await;
    let result = run
        .result
        .expect("the fanout epilog fixture must execute offline");

    assert_eq!(result, "x-1,y-2");
    assert_two_arms_all_succeeded(&run.recorder.records());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_store_writes_persist_across_arms() {
    // Arm-scoped writes under the claims model: each arm writes only its
    // own path, so no two live identities ever claim one path, and the
    // parent's post-join glob sees the merged state because a finished
    // arm's claims release at chain end. (The fixture's old ready-*.md
    // rendezvous polled a live sibling's writes - precisely the cross-arm
    // read-while-written pattern the claims model rejects - so it was
    // removed; interleaving coverage lives in the scheduler's
    // `fanout_arms_interleave_at_io_points_on_one_thread`.)
    let run = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture(
            FANOUT_STORE_WRITES,
            "execution/fanout-store-writes.md",
            FANOUT_STORE_EXECUTION,
            "",
            None,
        ),
    )
    .await
    .expect("the fanout fixture completes");
    let result = run
        .result
        .expect("the fanout store fixture must execute offline");

    // Both prologue-only arms write distinct paths and the parent reply vector
    // stays list-ordered.
    assert_eq!(result, "2:alpha,beta");
    assert_eq!(
        run.store.read("arm-1.md").expect("arm 1 must write"),
        "alpha"
    );
    assert_eq!(
        run.store.read("arm-2.md").expect("arm 2 must write"),
        "beta"
    );
    // The ordered merge: the join's collection-order results land in one
    // parent-written file, deterministic by construction.
    assert_eq!(
        run.store.read("merged.md").expect("the merge must land"),
        "alpha,beta"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cross_arm_append_terminates_the_run_with_a_determinism_violation() {
    // Every store operation is a leaf yield now, so two live arms appending
    // one path genuinely race in the blocking pool; the claims model booms
    // the loser and the violation fails the whole run at the answer
    // boundary. The fixture's pcall proves the violation is uncatchable:
    // were it resumed into the arm, the handler would record the catch and
    // the run would return "alpha,beta" instead of failing.
    let run = run_fixture(
        FANOUT_CROSS_ARM_APPEND,
        "execution/fanout-cross-arm-append.md",
        FANOUT_CROSS_ARM_EXECUTION,
        "",
        None,
    )
    .await;
    let error = match run.result {
        Ok(value) => panic!("a cross-arm append must terminate the run, got {value:?}"),
        Err(error) => error,
    };
    assert_eq!(
        error.kind(),
        RunErrorKind::Determinism,
        "a claims conflict classifies as a determinism violation: {error:?}"
    );
    let text = error.to_string();
    assert!(
        text.contains("evidence.md"),
        "the violation names the contested path: {text}"
    );
    assert!(
        text.contains("conflicts with"),
        "the violation names the conflicting claim: {text}"
    );
    assert_eq!(
        text.matches("ExecId(").count(),
        2,
        "the violation names both arms' identities: {text}"
    );
    // The losing arm's append never reached the backend: exactly one arm's
    // line landed.
    let evidence = run.store.read("evidence.md").expect("one arm appended");
    assert!(
        evidence == "alpha\n" || evidence == "beta\n",
        "exactly one arm's append may land: {evidence:?}"
    );
}

/// A one-shot gate for the winning arm's backend append: the first
/// `append` the backend serves signals `started` and parks until `open`
/// releases it, so the run's terminal failure lands while the winning
/// op - its access clone alive, its write claim held - is still in
/// flight on the blocking pool.
#[derive(Default)]
struct AppendGate {
    started: tokio::sync::Notify,
    released: Mutex<bool>,
    release: Condvar,
    taken: AtomicBool,
}

impl AppendGate {
    /// Parks the first caller until the gate opens; later callers pass.
    fn block_first(&self) {
        if self.taken.swap(true, Ordering::SeqCst) {
            return;
        }
        self.started.notify_one();
        let mut released = self
            .released
            .lock()
            .expect("the gate mutex is not poisoned");
        while !*released {
            released = self
                .release
                .wait(released)
                .expect("the gate mutex is not poisoned");
        }
    }

    /// Waits for the first parked append.
    async fn await_started(&self) {
        self.started.notified().await;
    }

    /// Releases the parked append.
    fn open(&self) {
        let mut released = self
            .released
            .lock()
            .expect("the gate mutex is not poisoned");
        *released = true;
        self.release.notify_all();
    }
}

/// Opens the gate on drop, so a panicking assertion never strands the
/// parked blocking-pool op (the runtime waits for it at shutdown).
struct GateGuard(Arc<AppendGate>);

impl Drop for GateGuard {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// A memory backend whose first `append` parks on the gate, standing in
/// for a slow host backend: the winning arm's op stays in flight across
/// the run's terminal failure.
struct GatedStore {
    inner: MemoryBackend,
    gate: Arc<AppendGate>,
}

impl Vfs for GatedStore {
    fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError> {
        Ok(Box::new(GatedAccess {
            inner: self.inner.acquire(id)?,
            gate: Arc::clone(&self.gate),
        }))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        self.inner.release(id)
    }
}

struct GatedAccess {
    inner: Box<dyn VfsAccess>,
    gate: Arc<AppendGate>,
}

impl VfsAccess for GatedAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.inner.read(path)
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.inner.write(path, contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.gate.block_first();
        self.inner.append(path, contents)
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.inner.remove(path, recursive)
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.inner.exists(path)
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.inner.glob(pattern)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.inner.list(path)
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.inner.stat(path)
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.inner.mkdir(path, recursive)
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.rename(from, to)
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.copy(from, to)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminal_failure_releases_an_in_flight_arms_claims_before_returning() {
    // The winning arm's append parks inside the backend with its write
    // claim held; the sibling's append booms against that claim and the
    // run fails fast at the answer boundary. The run's result must not
    // be delivered while the parked op's access clone still holds the
    // claim: the driver drains (awaits) the in-flight op before
    // returning, so the post-run fresh-access read never meets a
    // lingering claim.
    let gate = Arc::new(AppendGate::default());
    let _guard = GateGuard(Arc::clone(&gate));
    let vfs = VfsRef::builder()
        .mount(
            promptforge_vfs::STORE_MOUNT,
            GatedStore {
                inner: MemoryBackend::new(),
                gate: Arc::clone(&gate),
            },
        )
        .build();
    let mut run_task = tokio::spawn(run_fixture(
        FANOUT_CROSS_ARM_APPEND,
        "execution/fanout-cross-arm-append.md",
        FANOUT_CROSS_ARM_EXECUTION,
        "",
        Some(vfs),
    ));
    // The winning arm's op is now parked inside the backend, its claim
    // held; the sibling's boom needs no test interaction.
    gate.await_started().await;
    // Proving the negative - the run did NOT return while the claim is
    // held - takes a bounded wait. The fail-fast path is pure in-runtime
    // scheduling (no I/O, no timers), so the grace is generous: without
    // the drain the run returns within milliseconds.
    if let Ok(done) = tokio::time::timeout(Duration::from_secs(5), &mut run_task).await {
        let run = done.expect("the run task must not panic");
        let lingering = run.store.read("evidence.md").err();
        panic!(
            "the run returned while an in-flight arm op still held its write claim; \
             a fresh post-run access meets the lingering claim: {lingering:?}"
        );
    }
    gate.open();
    let run = run_task.await.expect("the run task must not panic");
    let error = match run.result {
        Ok(value) => panic!("a cross-arm append must terminate the run, got {value:?}"),
        Err(error) => error,
    };
    assert_eq!(
        error.kind(),
        RunErrorKind::Determinism,
        "a claims conflict classifies as a determinism violation: {error:?}"
    );
    // The invariant the drain restores: a fresh post-run access never
    // meets a lingering claim, and the winning arm's append landed.
    let evidence = run.store.read("evidence.md").expect("one arm appended");
    assert!(
        evidence == "alpha\n" || evidence == "beta\n",
        "exactly one arm's append may land: {evidence:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_failure_propagates() {
    let run = run_fixture(
        FANOUT_ARM_FAILURE,
        "execution/fanout-arm-failure.md",
        FANOUT_FAILURE_EXECUTION,
        "",
        None,
    )
    .await;
    let error = match run.result {
        Ok(value) => panic!("the fanout arm failure must propagate, got {value:?}"),
        Err(error) => error,
    };

    // Assert the stable classification first, then the preserved source context.
    assert_eq!(
        error.kind(),
        RunErrorKind::Lua,
        "a raised arm error must classify as a Lua failure: {error:?}"
    );
    assert!(
        error.to_string().contains("deliberately failed"),
        "error must preserve the arm's message: {error}"
    );
}
