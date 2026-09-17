//! One backend-neutral transcription worker.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use crate::{DecodeMode, DecodeRequest, Decoder, ModelFactory, TranscribeError};

pub(crate) const INTERIM_JOB_CAPACITY: usize = 8;
pub(crate) const FINAL_JOB_CAPACITY: usize = 8;

struct Job {
    request: DecodeRequest,
    reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
    lifetime_guard: Option<Arc<dyn Send + Sync>>,
}

/// Handle to a decoder confined to its worker thread.
#[derive(Debug)]
pub(crate) struct Transcriber {
    state: Mutex<TranscriberState>,
    stopping: Arc<AtomicBool>,
}

#[derive(Debug)]
struct TranscriberState {
    job_tx: Option<mpsc::SyncSender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
    join_panicked: bool,
}

impl Transcriber {
    /// Spawns one worker and reports whether its optional decoder exists.
    pub(super) fn spawn(
        name: &'static str,
        factory: Arc<dyn ModelFactory>,
        mode: DecodeMode,
        capacity: usize,
    ) -> Result<(Self, mpsc::Receiver<Result<bool, TranscribeError>>), TranscribeError> {
        let (job_tx, job_rx) = mpsc::sync_channel::<Job>(capacity);
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let worker = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                worker_loop(factory.as_ref(), mode, &job_rx, &init_tx, &worker_stopping);
            })
            .map_err(TranscribeError::SpawnWorker)?;
        Ok((
            Self {
                state: Mutex::new(TranscriberState {
                    job_tx: Some(job_tx),
                    worker: Some(worker),
                    join_panicked: false,
                }),
                stopping,
            },
            init_rx,
        ))
    }

    fn submit(
        &self,
        mut request: DecodeRequest,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<String, TranscribeError>>, TranscribeError>
    {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        let lifetime_guard = request.take_lifetime_guard();
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(job_tx) = &state.job_tx else {
            return Err(TranscribeError::WorkerGone);
        };
        job_tx
            .try_send(Job {
                request,
                reply,
                lifetime_guard,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => TranscribeError::Overloaded,
                mpsc::TrySendError::Disconnected(_) => TranscribeError::WorkerGone,
            })?;
        Ok(reply_rx)
    }

    pub(super) async fn transcribe(
        &self,
        request: DecodeRequest,
    ) -> Result<String, TranscribeError> {
        let reply_rx = self.submit(request)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }

    /// Closes the job queue and joins the worker thread.
    ///
    /// This is the blocking, error-reporting path; `Drop` only signals
    /// and detaches through [`Transcriber::signal_and_detach`].
    pub(super) fn shutdown(&self) -> Result<(), TranscribeError> {
        self.stopping.store(true, Ordering::Release);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        drop(state.job_tx.take());
        if let Some(worker) = state.worker.take() {
            state.join_panicked = worker.join().is_err();
        }
        if state.join_panicked {
            Err(TranscribeError::ShutdownPanicked)
        } else {
            Ok(())
        }
    }

    pub(super) fn abandon_startup(&self) {
        // Construction is non-preemptible. Dropping this handle explicitly
        // abandons only a timed-out startup worker so the host can classify
        // the fatal outcome without claiming the thread was stopped.
        self.signal_and_detach();
    }

    /// Signals the worker and detaches its thread without joining.
    ///
    /// The worker captures only owned or `Arc` state (the factory, the job
    /// receiver, the stop flag) and borrows nothing from this handle, so a
    /// detached thread finishes any running decode on its own.
    pub(super) fn signal_and_detach(&self) {
        self.stopping.store(true, Ordering::Release);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        drop(state.job_tx.take());
        drop(state.worker.take());
    }

    pub(super) fn startup_failure(
        startup: TranscribeError,
        cleanup: impl IntoIterator<Item = Result<(), TranscribeError>>,
    ) -> TranscribeError {
        let cleanup = cleanup
            .into_iter()
            .filter_map(Result::err)
            .collect::<Vec<_>>();
        if cleanup.is_empty() {
            startup
        } else {
            TranscribeError::StartupCleanup {
                startup: Box::new(startup),
                cleanup,
            }
        }
    }
}

impl Drop for Transcriber {
    fn drop(&mut self) {
        // `shutdown` is the blocking, error-reporting path; Drop can
        // neither wait nor report, so it signals and detaches.
        self.signal_and_detach();
    }
}

fn worker_loop(
    factory: &dyn ModelFactory,
    mode: DecodeMode,
    job_rx: &mpsc::Receiver<Job>,
    init_tx: &mpsc::SyncSender<Result<bool, TranscribeError>>,
    stopping: &AtomicBool,
) {
    let decoder = catch_unwind(AssertUnwindSafe(|| factory.create(mode)));
    let decoder = match decoder {
        Ok(result) => result,
        Err(_) => Err(TranscribeError::WorkerPanicked),
    };
    let Some(mut decoder): Option<Box<dyn Decoder>> = (match decoder {
        Ok(decoder) => {
            if init_tx.send(Ok(decoder.is_some())).is_err() {
                return;
            }
            decoder
        }
        Err(error) => {
            // Initialization is terminal; cancellation leaves no constructor to receive it.
            drop(init_tx.send(Err(error)));
            return;
        }
    }) else {
        return;
    };
    while !stopping.load(Ordering::Acquire) {
        let Ok(job) = job_rx.recv() else {
            return;
        };
        if stopping.load(Ordering::Acquire) {
            return;
        }
        if job.reply.is_closed() {
            continue;
        }
        let result = catch_unwind(AssertUnwindSafe(|| decoder.decode(job.request)));
        drop(job.lifetime_guard);
        if let Ok(result) = result {
            if !stopping.load(Ordering::Acquire) {
                // A disconnected caller no longer needs this stateless result.
                drop(job.reply.send(result));
            }
        } else {
            // A disconnected caller cannot make the panicked worker reusable.
            drop(job.reply.send(Err(TranscribeError::WorkerPanicked)));
            return;
        }
    }
}

#[cfg(test)]
mod tests {
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
        let (worker, startup) = Transcriber::spawn("bounded-worker-test", factory, mode, capacity)
            .expect("worker spawns");
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
}
