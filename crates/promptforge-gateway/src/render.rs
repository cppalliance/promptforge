//! The process progress renderer: draws the hub to indicatif bars on a TTY,
//! or emits tracing lines otherwise.
//!
//! The renderer is a plain thread polling [`ProgressHub::snapshot`], so it
//! also covers startup provisioning, which runs before the tokio runtime
//! exists. Producers never format output; this thread is the gateway
//! process's one presentation of the hub.

use std::collections::{HashMap, HashSet};
use std::io::IsTerminal as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use promptforge_progress::{OperationId, OperationSnapshot, ProgressHub};

/// Snapshot poll cadence of the renderer thread.
const RENDER_INTERVAL: Duration = Duration::from_millis(120);

/// Non-TTY log cadence: a line per 5% step per node.
const LOG_STEP_PERCENT: u64 = 5;

/// Bar resolution: fractions render against a fixed step count.
const BAR_STEPS: u64 = 1000;
const BAR_STEPS_F: f64 = 1000.0;

/// A running renderer thread. Dropping it signals the thread and joins it,
/// so every exit path of the serving lifecycle stops rendering.
#[derive(Debug)]
pub(crate) struct Renderer {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Renderer {
    /// Starts the renderer thread for `hub`: indicatif bars when stderr is a
    /// terminal, tracing lines otherwise. A spawn failure degrades to no
    /// rendering (logged), never to a boot failure.
    pub(crate) fn start(hub: &Arc<ProgressHub>) -> Renderer {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("progress-renderer".to_string())
            .spawn({
                let hub = Arc::clone(hub);
                let stop = Arc::clone(&stop);
                move || render_loop(&hub, &stop)
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

fn render_loop(hub: &ProgressHub, stop: &AtomicBool) {
    if std::io::stderr().is_terminal() {
        tty_loop(hub, stop);
    } else {
        log_loop(hub, stop);
    }
}

/// Non-TTY rendering: each tick's new lines go to `tracing::info!`.
fn log_loop(hub: &ProgressHub, stop: &AtomicBool) {
    let mut renderer = LineRenderer::default();
    while !stop.load(Ordering::Relaxed) {
        for line in renderer.lines(&hub.snapshot()) {
            tracing::info!("{line}");
        }
        std::thread::sleep(RENDER_INTERVAL);
    }
}

/// TTY rendering: one indicatif bar per live node, keyed by operation and
/// path; bars for detached operations are cleared.
fn tty_loop(hub: &ProgressHub, stop: &AtomicBool) {
    let multi = MultiProgress::new();
    let mut bars: HashMap<(OperationId, String), ProgressBar> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        let snapshot = hub.snapshot();
        let mut seen = HashSet::with_capacity(bars.len());
        for operation in &snapshot {
            for node in &operation.nodes {
                let key = (operation.operation, node.path.clone());
                seen.insert(key.clone());
                let bar = bars.entry(key).or_insert_with(|| {
                    let bar = multi.add(ProgressBar::new(BAR_STEPS));
                    if let Some(style) = bar_style() {
                        bar.set_style(style);
                    }
                    bar.set_message(node.label.clone());
                    bar
                });
                bar.set_position(bar_position(node.fraction));
            }
        }
        bars.retain(|key, bar| {
            let live = seen.contains(key);
            if !live {
                bar.finish_and_clear();
                multi.remove(bar);
            }
            live
        });
        std::thread::sleep(RENDER_INTERVAL);
    }
    for bar in bars.into_values() {
        bar.finish_and_clear();
    }
}

fn bar_style() -> Option<ProgressStyle> {
    ProgressStyle::with_template("{msg} [{bar:40.cyan/blue}] {percent:>3}%")
        .ok()
        .map(|style| style.progress_chars("=>-"))
}

/// Maps a fraction to a bar position.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the fraction is clamped to 0.0..=1.0 before the cast"
)]
fn bar_position(fraction: f64) -> u64 {
    (fraction.clamp(0.0, 1.0) * BAR_STEPS_F).round() as u64
}

/// Non-TTY rendering as a pure snapshot-to-lines transform: a `started` line
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

    #[test]
    fn bar_position_clamps_and_rounds() {
        assert_eq!(bar_position(0.0), 0);
        assert_eq!(bar_position(0.5), 500);
        assert_eq!(bar_position(1.0), BAR_STEPS);
        assert_eq!(bar_position(2.0), BAR_STEPS, "over-range clamps to full");
        assert_eq!(bar_position(-0.5), 0, "under-range clamps to empty");
        assert_eq!(bar_position(f64::NAN), 0, "NaN maps to empty");
        assert_eq!(bar_position(0.0004), 0, "sub-step rounds down");
        assert_eq!(bar_position(0.0006), 1, "half a step rounds up");
    }
}
