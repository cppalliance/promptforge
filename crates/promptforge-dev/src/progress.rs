//! Setup progress for the dev runner: a small operation tree around the
//! model catalog fetch, the embedding-model load, and tool indexing, rendered
//! to indicatif bars while stderr is a terminal.
//!
//! The tree measures; the renderer is a plain polling thread reading hub
//! snapshots, so the synchronous model load and index build need no async
//! cooperation. Off a terminal nothing spawns and nothing prints, keeping
//! stderr clean for the run's own diagnostics.

use std::collections::{HashMap, HashSet};
use std::io::IsTerminal as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use promptforge_progress::{ProgressHandle, ProgressHub, ProgressTree};

/// Snapshot poll cadence of the renderer thread.
const RENDER_INTERVAL: Duration = Duration::from_millis(120);

/// Bar resolution: fractions render against a fixed step count.
const BAR_STEPS: u64 = 1000;
const BAR_STEPS_F: f64 = 1000.0;

/// The setup operation's tree, its leaf handles, and the renderer drawing
/// them. Weights track expected duration: the embedding-model load dominates,
/// the catalog fetch is one network round trip, and indexing is one forward
/// pass per tool.
///
/// Drop order is the shutdown sequence: the tree detaches first so the
/// renderer's last tick clears every bar, then the renderer stops and joins.
pub(crate) struct SetupProgress {
    /// Held for its Drop: detaches the operation tree from the hub.
    #[allow(dead_code, reason = "read only by the cfg(test) fraction accessor")]
    tree: ProgressTree,
    /// Model catalog fetch: indeterminate, completed when the fetch returns.
    pub(crate) catalog: ProgressHandle,
    /// Embedding-model load: byte-measured by the tool-picker's weight copy.
    pub(crate) model: ProgressHandle,
    /// Tool indexing: one tool-count step per embedded tool.
    pub(crate) tools: ProgressHandle,
    /// Held for its Drop: stops and joins the renderer thread.
    #[expect(dead_code, reason = "dropped, never read")]
    renderer: TtyRenderer,
}

impl SetupProgress {
    /// Attaches the setup tree to a fresh hub and starts the TTY renderer.
    pub(crate) fn new() -> Self {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        Self {
            catalog: tree.register("model catalog", 1.0),
            model: tree.register("embedding model", 4.0),
            tools: tree.register("tool index", 1.0),
            renderer: TtyRenderer::start(&hub),
            tree,
        }
    }

    /// The weighted aggregate fraction across the setup leaves, in `0.0..=1.0`.
    #[cfg(test)]
    pub(crate) fn fraction(&self) -> f64 {
        self.tree.fraction()
    }
}

/// A running renderer thread, or inert when stderr is not a terminal.
/// Dropping it signals the thread and joins it, so every exit path of the
/// setup sequence stops rendering.
#[derive(Debug)]
struct TtyRenderer {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TtyRenderer {
    /// Starts the renderer thread when stderr is a terminal; otherwise returns
    /// an inert renderer. A spawn failure degrades to no rendering (warned),
    /// never to a run failure.
    fn start(hub: &Arc<ProgressHub>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        if !std::io::stderr().is_terminal() {
            return TtyRenderer { stop, thread: None };
        }
        let spawned = std::thread::Builder::new()
            .name("setup-progress".to_string())
            .spawn({
                let hub = Arc::clone(hub);
                let stop = Arc::clone(&stop);
                move || render_loop(&hub, &stop)
            });
        match spawned {
            Ok(thread) => TtyRenderer {
                stop,
                thread: Some(thread),
            },
            Err(error) => {
                eprintln!("warning: cannot spawn the progress renderer: {error}");
                TtyRenderer { stop, thread: None }
            }
        }
    }
}

impl Drop for TtyRenderer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ignored = thread.join();
        }
    }
}

/// One indicatif bar per live node, keyed by path; bars for nodes that leave
/// the snapshot are cleared.
fn render_loop(hub: &ProgressHub, stop: &AtomicBool) {
    let multi = MultiProgress::new();
    let mut bars: HashMap<String, ProgressBar> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        let mut seen = HashSet::with_capacity(bars.len());
        for operation in hub.snapshot() {
            for node in &operation.nodes {
                seen.insert(node.path.clone());
                let bar = bars.entry(node.path.clone()).or_insert_with(|| {
                    let bar = multi.add(ProgressBar::new(BAR_STEPS));
                    if let Ok(style) =
                        ProgressStyle::with_template("{msg} [{bar:40.cyan/blue}] {percent:>3}%")
                    {
                        bar.set_style(style.progress_chars("=>-"));
                    }
                    bar.set_message(node.label.clone());
                    bar
                });
                bar.set_position(bar_position(node.fraction));
            }
        }
        bars.retain(|path, bar| {
            let live = seen.contains(path);
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

/// Maps a fraction to a bar position.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the fraction is clamped to 0.0..=1.0 before the cast"
)]
fn bar_position(fraction: f64) -> u64 {
    (fraction.clamp(0.0, 1.0) * BAR_STEPS_F).round() as u64
}

#[cfg(test)]
mod tests {
    // Fractions are fixed-point millionths, so equality comparisons are exact.
    #![expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]

    use super::*;

    #[test]
    fn bar_position_clamps_to_the_bar_range() {
        assert_eq!(bar_position(-1.0), 0);
        assert_eq!(bar_position(0.0), 0);
        assert_eq!(bar_position(0.5), 500);
        assert_eq!(bar_position(1.0), BAR_STEPS);
        assert_eq!(bar_position(2.0), BAR_STEPS);
        assert_eq!(bar_position(f64::NAN), 0);
    }

    #[test]
    fn setup_tree_aggregates_its_leaves_by_expected_duration() {
        let progress = SetupProgress::new();
        assert_eq!(progress.fraction(), 0.0);
        progress.catalog.complete();
        progress.model.set_fraction(0.5);
        // (1*1 + 4*0.5 + 1*0) / 6: the model leaf carries four shares.
        assert_eq!(progress.fraction(), 0.5);
        progress.model.complete();
        progress.tools.complete();
        assert_eq!(progress.fraction(), 1.0);
    }
}
