//! The process progress renderer: emits the hub as tracing log lines.
//!
//! The renderer is a plain thread polling [`ProgressHub::snapshot`], so it
//! also covers startup provisioning, which runs before the tokio runtime
//! exists. Producers never format output; this thread is the gateway
//! process's one presentation of the hub. Visual progress lives in the
//! config UI status bar and the tray label; the terminal carries logs only.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use shared_progress::{OperationId, OperationSnapshot, ProgressHub};

/// Snapshot poll cadence of the renderer thread.
const RENDER_INTERVAL: Duration = Duration::from_millis(120);

/// Log cadence: a line per 5% step per node.
const LOG_STEP_PERCENT: u64 = 5;

/// A running renderer thread. Dropping it signals the thread and joins it,
/// so every exit path of the serving lifecycle stops rendering.
#[derive(Debug)]
pub(crate) struct Renderer {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Renderer {
    /// Starts the renderer thread for `hub`, emitting tracing lines on every
    /// stream. A spawn failure degrades to no rendering (logged), never to
    /// a boot failure.
    pub(crate) fn start(hub: &Arc<ProgressHub>) -> Renderer {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("progress-renderer".to_string())
            .spawn({
                let hub = Arc::clone(hub);
                let stop = Arc::clone(&stop);
                move || log_loop(&hub, &stop)
            });
        match thread {
            Ok(thread) => Renderer {
                stop,
                thread: Some(thread),
            },
            Err(error) => {
                tracing::error!(
                    "failed to spawn the progress renderer thread: {error}; progress will not render"
                );
                Renderer { stop, thread: None }
            }
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("the progress renderer thread panicked");
        }
    }
}

/// The renderer thread's body: each tick's new lines go to `tracing::info!`.
fn log_loop(hub: &ProgressHub, stop: &AtomicBool) {
    let mut renderer = LineRenderer::default();
    while !stop.load(Ordering::Relaxed) {
        for line in renderer.lines(&hub.snapshot()) {
            tracing::info!("{line}");
        }
        std::thread::sleep(RENDER_INTERVAL);
    }
}

/// Log rendering as a pure snapshot-to-lines transform: a `started` line
/// on first sight of a node, a percent line per [`LOG_STEP_PERCENT`] step,
/// and a `done` line at completion - a node first seen already complete
/// earns its `started` and `done` lines together. State for a node that
/// leaves the snapshot is dropped, so a later operation reusing the path
/// reports afresh and the map stays bounded by the live operations.
#[derive(Debug, Default)]
pub(crate) struct LineRenderer {
    emitted: HashMap<(OperationId, String), u64>,
}

impl LineRenderer {
    /// The lines for one snapshot tick.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "snapshot fractions are clamped to 0.0..=1.0"
    )]
    pub(crate) fn lines(&mut self, snapshot: &[OperationSnapshot]) -> Vec<String> {
        let mut lines = Vec::new();
        let mut seen = HashSet::with_capacity(self.emitted.len());
        for operation in snapshot {
            for node in &operation.nodes {
                let key = (operation.operation, node.path.clone());
                seen.insert(key.clone());
                let percent = (node.fraction * 100.0).round() as u64;
                match self.emitted.get(&key) {
                    None => {
                        lines.push(format!("{}: started", node.path));
                        // A sub-poll-interval operation is first seen
                        // complete; it still earns its `done` line.
                        if percent == 100 {
                            lines.push(format!("{}: done", node.path));
                        }
                        self.emitted.insert(key, percent);
                    }
                    Some(&previous) => {
                        let line = if percent == 100 && previous < 100 {
                            Some(format!("{}: done", node.path))
                        } else if percent >= previous + LOG_STEP_PERCENT {
                            Some(format!("{}: {}%", node.path, percent))
                        } else {
                            None
                        };
                        if let Some(line) = line {
                            lines.push(line);
                            self.emitted.insert(key, percent);
                        }
                    }
                }
            }
        }
        self.emitted.retain(|key, _| seen.contains(key));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_to_lines_reports_started_steps_and_done() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("model.bin", 1.0);
        let mut renderer = LineRenderer::default();

        assert_eq!(renderer.lines(&hub.snapshot()), ["model.bin: started"]);
        leaf.set_fraction(0.04);
        assert!(
            renderer.lines(&hub.snapshot()).is_empty(),
            "a move below the 5% step stays quiet"
        );
        leaf.set_fraction(0.07);
        assert_eq!(renderer.lines(&hub.snapshot()), ["model.bin: 7%"]);
        leaf.complete();
        assert_eq!(renderer.lines(&hub.snapshot()), ["model.bin: done"]);
        assert!(
            renderer.lines(&hub.snapshot()).is_empty(),
            "a finished node does not report twice"
        );
    }

    #[test]
    fn a_node_first_seen_complete_reports_started_and_done() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("quick.bin", 1.0);
        leaf.complete();
        let mut renderer = LineRenderer::default();

        assert_eq!(
            renderer.lines(&hub.snapshot()),
            ["quick.bin: started", "quick.bin: done"],
            "a sub-poll-interval operation still earns its done line"
        );
        assert!(
            renderer.lines(&hub.snapshot()).is_empty(),
            "a finished node does not report twice"
        );
    }

    #[test]
    fn a_detached_operation_drops_its_state_and_reports_afresh() {
        let hub = Arc::new(ProgressHub::new());
        let mut renderer = LineRenderer::default();
        {
            let tree = hub.operation();
            let _leaf = tree.register("model.bin", 1.0);
            assert_eq!(renderer.lines(&hub.snapshot()), ["model.bin: started"]);
        }
        assert!(
            renderer.lines(&hub.snapshot()).is_empty(),
            "an idle hub emits no lines"
        );
        assert!(
            renderer.emitted.is_empty(),
            "a detached operation drops its cadence state"
        );
        let tree = hub.operation();
        let _leaf = tree.register("model.bin", 1.0);
        assert_eq!(
            renderer.lines(&hub.snapshot()),
            ["model.bin: started"],
            "a re-attached operation reports from the beginning"
        );
    }

    /// A shared buffer that captures what the renderer loop logs, so the
    /// test can assert on the emitted lines.
    #[derive(Clone, Default)]
    struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl LogBuffer {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("log buffer")).into_owned()
        }
    }

    impl std::io::Write for LogBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log buffer").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
        type Writer = LogBuffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn the_render_loop_emits_tracing_lines_for_a_hub_operation() {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("model.bin", 1.0);
        leaf.complete();
        let stop = AtomicBool::new(false);
        // The loop runs on this thread so the thread-local subscriber
        // captures its lines; the scoped helper stops it as soon as the
        // done line lands. The deadline only bounds a broken loop - the
        // verdict never depends on timing.
        let stopper = buffer.clone();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                while !stopper.contents().contains("model.bin: done")
                    && std::time::Instant::now() < deadline
                {
                    std::thread::yield_now();
                }
                stop.store(true, Ordering::Relaxed);
            });
            log_loop(&hub, &stop);
        });

        let logs = buffer.contents();
        assert!(
            logs.contains("model.bin: started"),
            "the loop logs the started line, got: {logs}"
        );
        assert!(
            logs.contains("model.bin: done"),
            "the loop logs the done line, got: {logs}"
        );
    }
}
