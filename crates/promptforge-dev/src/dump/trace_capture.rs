//! Opt-in raw model-turn capture written off the execution task.
//!
//! [`TraceCapture`] implements [`DebugCapture`]. Because raw capture persists
//! verbatim request and response bodies (full prompts, tool arguments and
//! results, and model output), it can only be constructed with a
//! [`SensitiveCapture`] authorization, so writing that material is always a
//! deliberate opt-in rather than a silent default. To honor the trait's
//! "return promptly, never block on I/O" contract, [`on_event`](TraceCapture::on_event)
//! only moves the owned event into a bounded queue; a dedicated worker thread
//! owns serialization and the restricted, atomic file writes.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;

use promptforge_core::debug::{DebugCapture, DebugEvent};
use serde_json::Value;

use super::fs_safe;

/// Bound on the pending-capture queue. A full queue means the worker is behind;
/// further events are counted as dropped rather than blocking the run.
const QUEUE_CAPACITY: usize = 128;

/// Proof that raw, unredacted turn capture was explicitly authorized.
///
/// A [`TraceCapture`] cannot be constructed without one, so persisting raw
/// sensitive bodies is always a deliberate choice made at the process
/// boundary (an explicit `--capture-raw` invocation) rather than a default.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SensitiveCapture(());

impl SensitiveCapture {
    /// Mints an authorization. The caller vouches that raw capture was
    /// explicitly requested.
    pub(crate) fn authorized() -> SensitiveCapture {
        SensitiveCapture(())
    }
}

/// One serialized turn payload awaiting a restricted, atomic write.
struct TraceJob {
    name: String,
    body: Value,
}

/// A nonblocking [`DebugCapture`] that persists raw turn payloads under
/// `<prompt-stem>.store/.trace/`.
pub(crate) struct TraceCapture {
    sender: Mutex<Option<SyncSender<TraceJob>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    dropped: AtomicU64,
}

impl std::fmt::Debug for TraceCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TraceCapture").finish()
    }
}

impl TraceCapture {
    /// Captures turns under `store_dir/.trace/`.
    ///
    /// Requires a [`SensitiveCapture`] authorization because raw bodies are
    /// persisted verbatim.
    pub(crate) fn new(store_dir: &Path, _authorization: SensitiveCapture) -> TraceCapture {
        let trace_dir = store_dir.join(".trace");
        let (sender, receiver) = sync_channel::<TraceJob>(QUEUE_CAPACITY);
        let worker = match std::thread::Builder::new()
            .name("promptforge-dev-trace".to_owned())
            .spawn(move || run_worker(&trace_dir, &receiver))
        {
            Ok(handle) => Some(handle),
            Err(error) => {
                eprintln!("trace capture disabled: could not start the trace worker: {error}");
                None
            }
        };
        // If the worker thread could not be spawned, drop the sender so
        // `on_event` observes a closed channel and never blocks.
        let sender = worker.as_ref().map(|_| sender);
        TraceCapture {
            sender: Mutex::new(sender),
            worker: Mutex::new(worker),
            dropped: AtomicU64::new(0),
        }
    }

    /// Closes the queue, drains and joins the worker, and reports any drops.
    ///
    /// Must be called once the run completes so every queued write lands before
    /// the dump directory is inspected. Because it joins the I/O worker (which
    /// can block), callers on an async runtime must invoke it off the runtime
    /// (for example via `tokio::task::spawn_blocking`).
    pub(crate) fn finish(&self) {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(sender);
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = worker {
            let _ignored = handle.join();
        }
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            eprintln!("trace dump dropped {dropped} event(s): capture queue was full");
        }
    }
}

impl DebugCapture for TraceCapture {
    fn on_event(&self, _execution: &str, _section: &str, turn_index: u32, event: DebugEvent) {
        let job = match event {
            DebugEvent::Request { body, .. } => TraceJob {
                name: format!("turn-{turn_index}-request.json"),
                body,
            },
            DebugEvent::Response { body, .. } => TraceJob {
                name: format!("turn-{turn_index}-response.json"),
                body,
            },
            _ => return,
        };
        let guard = self.sender.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(sender) = guard.as_ref() else {
            return;
        };
        match sender.try_send(job) {
            // Full: the worker is behind. Record the drop rather than block the
            // run's task, per the DebugCapture contract.
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            // Sent, or the worker exited (or never started): nothing to do.
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

/// Drains the queue until the sender closes, writing each payload.
fn run_worker(trace_dir: &Path, receiver: &Receiver<TraceJob>) {
    for job in receiver {
        write_one(trace_dir, &job);
    }
}

/// Serializes and writes one payload with restricted, atomic semantics.
fn write_one(trace_dir: &Path, job: &TraceJob) {
    if let Err(error) = fs_safe::create_dir_all_secure(trace_dir) {
        eprintln!("trace dump failed: create {}: {error}", trace_dir.display());
        return;
    }
    let target = trace_dir.join(&job.name);
    let rendered = match serde_json::to_string_pretty(&job.body) {
        Ok(text) => text,
        Err(error) => {
            eprintln!(
                "trace dump skipped {}: serialize: {error}",
                target.display()
            );
            return;
        }
    };
    if let Err(error) = fs_safe::write_atomic_secure(&target, rendered.as_bytes()) {
        eprintln!("trace dump skipped {}: write: {error}", target.display());
        return;
    }
    eprintln!("trace dump wrote {}", target.display());
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use promptforge_core::debug::DebugCapture;
    use promptforge_core::debug::DebugEvent;

    use super::{SensitiveCapture, TraceCapture};

    #[test]
    fn trace_capture_writes_turn_files_after_finish() {
        let directory = tempfile::tempdir().expect("create trace fixture directory");
        let store_dir = directory.path().join("fixture");
        let capture = TraceCapture::new(&store_dir, SensitiveCapture::authorized());
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::request(json!({ "model": "test", "messages": [] })),
        );
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::response(
                json!({ "choices": [{ "message": { "role": "assistant", "content": "hi" } }] }),
                Some("stop".into()),
                None,
            ),
        );
        capture.finish();

        let trace_dir = store_dir.join(".trace");
        let request =
            std::fs::read_to_string(trace_dir.join("turn-1-request.json")).expect("request dump");
        let response =
            std::fs::read_to_string(trace_dir.join("turn-1-response.json")).expect("response dump");
        assert!(request.contains("\"model\": \"test\""));
        assert!(response.contains("\"content\": \"hi\""));
    }

    #[cfg(unix)]
    #[test]
    fn dumped_trace_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("create trace fixture directory");
        let store_dir = directory.path().join("fixture");
        let capture = TraceCapture::new(&store_dir, SensitiveCapture::authorized());
        capture.on_event("dev-1", "Only", 1, DebugEvent::request(json!({ "a": 1 })));
        capture.finish();

        let file = store_dir.join(".trace").join("turn-1-request.json");
        let mode = std::fs::metadata(&file).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "raw trace files must be owner-only");
    }
}
