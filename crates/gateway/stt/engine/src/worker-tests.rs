//! Tests for bounded worker queues, cancellation, and shutdown.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::*;

#[derive(Debug, Default, Eq, PartialEq)]
enum ParkPhase {
    #[default]
    Ready,
    Entered,
    Released,
    Finished,
}

#[derive(Debug, Default)]
struct ParkState {
    calls: usize,
    phase: ParkPhase,
    dropped: bool,
}

#[derive(Debug, Clone, Default)]
struct ParkControl {
    state: Arc<(Mutex<ParkState>, Condvar)>,
}

impl ParkControl {
    fn wait_for(&self, predicate: impl Fn(&ParkState) -> bool, message: &str) {
        let (state, changed) = &*self.state;
        let guard = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (guard, timeout) = changed
            .wait_timeout_while(guard, Duration::from_secs(1), |state| !predicate(state))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(!timeout.timed_out() && predicate(&guard), "{message}");
    }

    fn release(&self) {
        let (state, changed) = &*self.state;
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .phase = ParkPhase::Released;
        changed.notify_all();
    }

    fn calls(&self) -> usize {
        self.state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .calls
    }
}

#[derive(Debug)]
struct ParkFactory(ParkControl);

impl ModelFactory for ParkFactory {
    fn create(
        &self,
        _mode: DecodeMode,
    ) -> Result<Option<Box<dyn crate::Decoder>>, TranscribeError> {
        Ok(Some(Box::new(ParkDecoder(self.0.clone()))))
    }
}

struct ParkDecoder(ParkControl);

impl crate::Decoder for ParkDecoder {
    fn decode(&mut self, _request: DecodeRequest) -> Result<String, TranscribeError> {
        let (state, changed) = &*self.0.state;
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.calls += 1;
        if state.calls == 1 {
            state.phase = ParkPhase::Entered;
            changed.notify_all();
            state = changed
                .wait_while(state, |state| state.phase != ParkPhase::Released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.phase = ParkPhase::Finished;
        changed.notify_all();
        Ok("scripted".to_owned())
    }
}

impl Drop for ParkDecoder {
    fn drop(&mut self) {
        let (state, changed) = &*self.0.state;
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .dropped = true;
        changed.notify_all();
    }
}

fn request(mode: DecodeMode) -> DecodeRequest {
    DecodeRequest::new(mode, Vec::new(), Vec::new(), String::new())
}

fn parked_worker(mode: DecodeMode, capacity: usize) -> (Transcriber, ParkControl) {
    let control = ParkControl::default();
    let factory: Arc<dyn ModelFactory> = Arc::new(ParkFactory(control.clone()));
    let (worker, startup) =
        Transcriber::spawn("bounded-worker-test", factory, mode, capacity).expect("worker spawns");
    assert!(
        startup
            .recv()
            .expect("startup outcome arrives")
            .expect("decoder starts")
    );
    (worker, control)
}

fn assert_queue_boundary(mode: DecodeMode, capacity: usize) {
    let (worker, control) = parked_worker(mode, capacity);
    let running = worker
        .submit(request(mode))
        .expect("running job is admitted");
    control.wait_for(
        |state| state.phase == ParkPhase::Entered,
        "first job enters the decoder",
    );

    let queued = (0..capacity)
        .map(|_| {
            worker
                .submit(request(mode))
                .expect("every queue slot is admitted")
        })
        .collect::<Vec<_>>();
    let error = worker
        .submit(request(mode))
        .expect_err("capacity plus one must fail without waiting");
    assert!(matches!(error, TranscribeError::Overloaded));

    drop(queued);
    control.release();
    assert_eq!(
        running
            .blocking_recv()
            .expect("worker replies")
            .expect("decode succeeds"),
        "scripted"
    );
    worker.shutdown().expect("worker joins");
    assert_eq!(control.calls(), 1, "cancelled queued jobs never decode");
}

#[test]
fn interim_queue_accepts_exact_capacity_and_rejects_capacity_plus_one() {
    assert_eq!(INTERIM_JOB_CAPACITY, 8);
    assert_queue_boundary(DecodeMode::Interim, INTERIM_JOB_CAPACITY);
}

#[test]
fn final_queue_accepts_exact_capacity_and_rejects_capacity_plus_one() {
    assert_eq!(FINAL_JOB_CAPACITY, 8);
    assert_queue_boundary(DecodeMode::Final, FINAL_JOB_CAPACITY);
}

#[cfg(feature = "test-fixtures")]
#[test]
fn miri_worker_queues_own_exact_capacity_and_reject_the_next_job() {
    assert_queue_boundary(DecodeMode::Interim, INTERIM_JOB_CAPACITY);
    assert_queue_boundary(DecodeMode::Final, FINAL_JOB_CAPACITY);
}

#[cfg(feature = "test-fixtures")]
#[test]
fn miri_shutdown_releases_worker_ownership_once() {
    let (worker, control) = parked_worker(DecodeMode::Interim, INTERIM_JOB_CAPACITY);
    worker.shutdown().expect("first shutdown joins");
    worker.shutdown().expect("second shutdown is idempotent");

    assert!(
        control
            .state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .dropped,
        "joined shutdown releases the worker-owned decoder"
    );
}

#[test]
fn cancellation_while_running_discards_only_that_reply() {
    let (worker, control) = parked_worker(DecodeMode::Interim, INTERIM_JOB_CAPACITY);
    let cancelled = worker
        .submit(request(DecodeMode::Interim))
        .expect("running job is admitted");
    control.wait_for(
        |state| state.phase == ParkPhase::Entered,
        "job enters the decoder",
    );
    drop(cancelled);
    control.release();
    control.wait_for(
        |state| state.phase == ParkPhase::Finished,
        "cancelled native-equivalent work returns",
    );

    let next = worker
        .submit(request(DecodeMode::Interim))
        .expect("worker remains available");
    assert_eq!(
        next.blocking_recv()
            .expect("worker replies")
            .expect("decode succeeds"),
        "scripted"
    );
    worker.shutdown().expect("worker joins");
    assert_eq!(control.calls(), 2);
}

#[test]
fn drop_signals_and_detaches_instead_of_joining_a_running_decode() {
    let (worker, control) = parked_worker(DecodeMode::Interim, INTERIM_JOB_CAPACITY);
    let reply = worker
        .submit(request(DecodeMode::Interim))
        .expect("running job is admitted");
    control.wait_for(
        |state| state.phase == ParkPhase::Entered,
        "job enters the decoder",
    );
    // The delayed releaser turns a blocking join into a failed timing
    // assertion instead of a deadlocked test.
    let releaser = std::thread::spawn({
        let control = control.clone();
        move || {
            std::thread::sleep(Duration::from_millis(500));
            control.release();
        }
    });

    let started = std::time::Instant::now();
    drop(worker);
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "drop signals and detaches instead of joining the running decode"
    );

    releaser.join().expect("the releaser thread joins");
    control.wait_for(
        |state| state.dropped,
        "the detached worker finishes the decode and drops the decoder",
    );
    assert!(
        reply.blocking_recv().is_err(),
        "a stopped worker discards the in-flight reply"
    );
}

#[test]
fn shutdown_joins_the_worker_and_is_idempotent() {
    let (worker, control) = parked_worker(DecodeMode::Interim, INTERIM_JOB_CAPACITY);
    worker.shutdown().expect("first shutdown joins");
    worker.shutdown().expect("second shutdown is idempotent");
    control.wait_for(
        |state| state.dropped,
        "shutdown drops the decoder before returning",
    );
    assert!(worker.submit(request(DecodeMode::Interim)).is_err());
}

#[test]
fn shutdown_waits_for_running_decode_instead_of_detaching() {
    let (worker, control) = parked_worker(DecodeMode::Interim, INTERIM_JOB_CAPACITY);
    let reply = worker
        .submit(request(DecodeMode::Interim))
        .expect("running job is admitted");
    control.wait_for(
        |state| state.phase == ParkPhase::Entered,
        "job enters the decoder",
    );
    let stopping = Arc::clone(&worker.stopping);
    let (returned_tx, returned_rx) = mpsc::channel();
    let shutdown = std::thread::spawn(move || {
        worker.shutdown().expect("worker joins");
        let _ignored = returned_tx.send(());
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while !stopping.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(
        stopping.load(Ordering::Acquire),
        "shutdown closes admission"
    );
    assert!(matches!(
        returned_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    control.release();
    returned_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("shutdown returns after native-equivalent work");
    shutdown.join().expect("shutdown thread does not panic");
    assert!(reply.blocking_recv().is_err(), "shutdown cancels the reply");
}
