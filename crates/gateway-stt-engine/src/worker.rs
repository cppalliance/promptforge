//! One backend-neutral transcription worker.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::{Decoder, ModelFactory, TranscribeError};

pub(crate) const INTERIM_JOB_CAPACITY: usize = 8;
pub(crate) const FINAL_JOB_CAPACITY: usize = 8;

struct Job {
    samples: Vec<f32>,
    guidance: Vec<String>,
    finalized: String,
    reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
}

/// Handle to a decoder confined to its worker thread.
#[derive(Debug)]
pub(crate) struct Transcriber {
    job_tx: Option<mpsc::SyncSender<Job>>,
    stopping: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Transcriber {
    /// Spawns one worker and reports whether its optional decoder exists.
    pub(super) fn spawn(
        name: &'static str,
        factory: Arc<dyn ModelFactory>,
        final_model: bool,
        capacity: usize,
    ) -> Result<(Self, mpsc::Receiver<Result<bool, TranscribeError>>), TranscribeError> {
        let (job_tx, job_rx) = mpsc::sync_channel::<Job>(capacity);
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let worker = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                worker_loop(
                    factory.as_ref(),
                    final_model,
                    &job_rx,
                    &init_tx,
                    &worker_stopping,
                );
            })
            .map_err(TranscribeError::SpawnWorker)?;
        Ok((
            Self {
                job_tx: Some(job_tx),
                stopping,
                worker: Some(worker),
            },
            init_rx,
        ))
    }

    fn submit(
        &self,
        samples: Vec<f32>,
        guidance: Vec<String>,
        finalized: String,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<String, TranscribeError>>, TranscribeError>
    {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        let Some(job_tx) = &self.job_tx else {
            return Err(TranscribeError::WorkerGone);
        };
        job_tx
            .try_send(Job {
                samples,
                guidance,
                finalized,
                reply,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => TranscribeError::Overloaded,
                mpsc::TrySendError::Disconnected(_) => TranscribeError::WorkerGone,
            })?;
        Ok(reply_rx)
    }

    pub(super) async fn transcribe(
        &self,
        samples: Vec<f32>,
        guidance: Vec<String>,
        finalized: String,
    ) -> Result<String, TranscribeError> {
        let reply_rx = self.submit(samples, guidance, finalized)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }

    pub(super) fn shutdown(&mut self) {
        self.stopping.store(true, Ordering::Release);
        drop(self.job_tx.take());
        if let Some(worker) = self.worker.take() {
            let _ignored = worker.join();
        }
    }
}

impl Drop for Transcriber {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop(
    factory: &dyn ModelFactory,
    final_model: bool,
    job_rx: &mpsc::Receiver<Job>,
    init_tx: &mpsc::SyncSender<Result<bool, TranscribeError>>,
    stopping: &AtomicBool,
) {
    let decoder = catch_unwind(AssertUnwindSafe(|| {
        if final_model {
            factory.create_final()
        } else {
            factory.create_interim().map(Some)
        }
    }));
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
        let result = catch_unwind(AssertUnwindSafe(|| {
            decoder.transcribe(&job.samples, &job.guidance, &job.finalized)
        }));
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
        fn create_interim(&self) -> Result<Box<dyn Decoder>, TranscribeError> {
            Ok(Box::new(ParkDecoder(self.0.clone())))
        }

        fn create_final(&self) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
            Ok(Some(Box::new(ParkDecoder(self.0.clone()))))
        }

        fn gpu_available(&self) -> bool {
            false
        }
    }

    struct ParkDecoder(ParkControl);

    impl Decoder for ParkDecoder {
        fn transcribe(
            &mut self,
            _samples: &[f32],
            _guidance: &[String],
            _finalized: &str,
        ) -> Result<String, TranscribeError> {
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

    fn parked_worker(final_model: bool, capacity: usize) -> (Transcriber, ParkControl) {
        let control = ParkControl::default();
        let factory: Arc<dyn ModelFactory> = Arc::new(ParkFactory(control.clone()));
        let (worker, startup) =
            Transcriber::spawn("bounded-worker-test", factory, final_model, capacity)
                .expect("worker spawns");
        assert!(
            startup
                .recv()
                .expect("startup outcome arrives")
                .expect("decoder starts")
        );
        (worker, control)
    }

    fn assert_queue_boundary(final_model: bool, capacity: usize) {
        let (mut worker, control) = parked_worker(final_model, capacity);
        let running = worker
            .submit(Vec::new(), Vec::new(), String::new())
            .expect("running job is admitted");
        control.wait_for(
            |state| state.phase == ParkPhase::Entered,
            "first job enters the decoder",
        );

        let queued = (0..capacity)
            .map(|_| {
                worker
                    .submit(Vec::new(), Vec::new(), String::new())
                    .expect("every queue slot is admitted")
            })
            .collect::<Vec<_>>();
        let error = worker
            .submit(Vec::new(), Vec::new(), String::new())
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
        worker.shutdown();
        assert_eq!(control.calls(), 1, "cancelled queued jobs never decode");
    }

    #[test]
    fn interim_queue_accepts_exact_capacity_and_rejects_capacity_plus_one() {
        assert_eq!(INTERIM_JOB_CAPACITY, 8);
        assert_queue_boundary(false, INTERIM_JOB_CAPACITY);
    }

    #[test]
    fn final_queue_accepts_exact_capacity_and_rejects_capacity_plus_one() {
        assert_eq!(FINAL_JOB_CAPACITY, 8);
        assert_queue_boundary(true, FINAL_JOB_CAPACITY);
    }

    #[test]
    fn cancellation_while_running_discards_only_that_reply() {
        let (mut worker, control) = parked_worker(false, INTERIM_JOB_CAPACITY);
        let cancelled = worker
            .submit(Vec::new(), Vec::new(), String::new())
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
            .submit(Vec::new(), Vec::new(), String::new())
            .expect("worker remains available");
        assert_eq!(
            next.blocking_recv()
                .expect("worker replies")
                .expect("decode succeeds"),
            "scripted"
        );
        worker.shutdown();
        assert_eq!(control.calls(), 2);
    }

    #[test]
    fn shutdown_joins_the_worker_and_is_idempotent() {
        let (mut worker, control) = parked_worker(false, INTERIM_JOB_CAPACITY);
        worker.shutdown();
        worker.shutdown();
        control.wait_for(
            |state| state.dropped,
            "shutdown drops the decoder before returning",
        );
        assert!(
            worker
                .submit(Vec::new(), Vec::new(), String::new())
                .is_err()
        );
    }

    #[test]
    fn shutdown_waits_for_running_decode_instead_of_detaching() {
        let (worker, control) = parked_worker(false, INTERIM_JOB_CAPACITY);
        let reply = worker
            .submit(Vec::new(), Vec::new(), String::new())
            .expect("running job is admitted");
        control.wait_for(
            |state| state.phase == ParkPhase::Entered,
            "job enters the decoder",
        );
        let stopping = Arc::clone(&worker.stopping);
        let (returned_tx, returned_rx) = mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            let mut worker = worker;
            worker.shutdown();
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
