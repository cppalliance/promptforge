//! The filesystem watcher: what turns saving a prompt into a live catalog.
//!
//! A developer writing a prompt cannot restart a service on every save, so
//! `prompts.toml` and the prompts directory are watched and a settled burst of
//! events re-resolves the catalog. The two halves are deliberately separate.
//! This module owns the platform events and the debounce window; [`Reloader`]
//! owns everything a settled window does, and takes no events, so the reload
//! rules are tested by calling them rather than by provoking a filesystem.
//!
//! The window earns its place on Windows above all. An editor saves through a
//! temporary file and renames it into place, which arrives as several events for
//! one save; the window restarts on each event and re-resolves once when they
//! stop, so a burst costs one resolution rather than one per event.
//!
//! Two things the window does not do itself. The re-resolution reads and parses
//! every prompt file, which is blocking work and runs on `spawn_blocking` rather
//! than on a runtime worker. And a watch that reports an error is registered
//! again on the next settled window, because a watch nobody re-establishes is a
//! server that keeps serving and quietly stops picking up saves.
//!
//! `[server].watch = false` starts nothing at all: no platform watcher, no
//! task, and a catalog that is exactly what boot resolved for the life of the
//! process.

#[cfg(test)]
mod fixture;
mod reload;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::catalog::CatalogHandle;
use crate::config::Config;
use crate::error::WatchError;
#[cfg(doc)]
use crate::error::WatchErrorKind;

pub(crate) use crate::watch::reload::Reloader;

/// How many "something changed" tokens the queue holds.
///
/// A token carries no information beyond its own existence, so one waiting
/// token already says what a second would; the depth only has to keep a burst
/// from being lost while the reload before it runs.
const PENDING_EVENTS: usize = 16;

/// A live watch over `prompts.toml` and the prompts directory.
///
/// [`shutdown`](Watcher::shutdown) is the ordered stop: it signals a reload
/// settling right now to publish nothing, closes the event stream, and awaits
/// the debounce task, so no reload lands after the server has decided to stop.
/// Dropping the watcher is the unordered fallback: the platform watcher goes
/// with it and the debounce task is aborted wherever it happens to be, so a
/// dropped watcher cannot reload a catalog somebody else now owns. The task
/// reaches the platform watcher through a [`Weak`] for that reason - it has to
/// re-register a broken watch, and it must not be what keeps one alive.
#[must_use = "dropping the watcher stops live reload; hold it for as long as the server serves"]
pub struct Watcher {
    /// The platform watcher, held so its `Drop` unregisters the watches.
    /// `Option` so [`shutdown`](Watcher::shutdown) can drop it early - closing
    /// the event channel - while `Drop` still covers a watcher stopped without
    /// a shutdown.
    platform: Option<Arc<Mutex<RecommendedWatcher>>>,
    /// The debounce task. `Option` so `shutdown` can take and await it, leaving
    /// `Drop` nothing to abort.
    task: Option<JoinHandle<()>>,
    /// Shared with the reloader. Set to stop a reload settling during shutdown
    /// from publishing a late generation.
    shutdown: Arc<AtomicBool>,
}

impl Watcher {
    /// Starts watching `source` and the configured prompts directory, and
    /// answers `None` when `[server].watch` is false.
    ///
    /// Must be called from inside a Tokio runtime: the debounce window runs on a
    /// task of its own.
    ///
    /// # Errors
    /// Returns [`WatchErrorKind::Create`] if the platform watcher cannot be
    /// built, [`WatchErrorKind::Watch`] if either path cannot be watched, and
    /// [`WatchErrorKind::Runtime`] if it is called with no Tokio runtime to run
    /// the debounce task on. A watch that cannot be established is a
    /// configuration problem an operator has to see, so it is an error here
    /// rather than a warning; `watch = false` is the way to serve without one.
    ///
    /// # Examples
    /// ```no_run
    /// # use std::path::Path;
    /// # use std::sync::Arc;
    /// # use promptforge_mcp_server::{Catalog, CatalogHandle, Config, OnBroken, Watcher};
    /// # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
    /// let source = Path::new("prompts.toml");
    /// let config = Config::load(source)?;
    /// let catalog = Arc::new(CatalogHandle::new(Catalog::resolve(&config, OnBroken::Reject)?));
    /// // Called from inside a Tokio runtime: the debounce window runs on a task.
    /// if let Some(watcher) = Watcher::start(source, Arc::new(config), catalog)? {
    ///     // ... serve ...
    ///     watcher.shutdown().await;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn start(
        source: &Path,
        config: Arc<Config>,
        catalog: Arc<CatalogHandle>,
    ) -> Result<Option<Watcher>, WatchError> {
        if !config.server.watch {
            tracing::info!("[server].watch is false: prompts are read once, at boot");
            return Ok(None);
        }
        // The debounce window and the blocking re-resolution both run on tasks,
        // so a runtime has to be present. Detecting its absence here turns a
        // caller's mistake into a returned error rather than a panic from the
        // first `tokio::spawn` below.
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(WatchError::runtime());
        }
        let window = config.server.watch_debounce;
        // The configuration's own directory, not the file: an editor that saves
        // through a temporary replaces the file, and a watch on the old file
        // would go with it. Events for the directory's other files are filtered
        // out by name.
        let roots = Roots {
            prompts: config.paths.prompts.clone(),
            config_dir: config_dir(source).to_path_buf(),
        };
        let interesting = Interesting::new(&roots.prompts, source);

        let (events, pending) = mpsc::channel(PENDING_EVENTS);
        let broken = Arc::new(AtomicBool::new(false));
        let flagged = Arc::clone(&broken);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                match event {
                    Ok(event) if interesting.matches(&event) => {
                        // A full queue means a reload is already owed, which is
                        // what this token would have asked for.
                        let _queued = events.try_send(());
                    }
                    Ok(_ignored) => {}
                    Err(error) => {
                        // Error level, because the developer whose saves stopped
                        // taking effect has no other way to find out. The token
                        // is what gets the watch re-registered, since a dead
                        // watch delivers no event to trigger that later.
                        tracing::error!("filesystem watch: {error}");
                        flagged.store(true, Ordering::Relaxed);
                        let _queued = events.try_send(());
                    }
                }
            })
            .map_err(WatchError::create)?;
        roots.register(&mut watcher)?;
        let watcher = Arc::new(Mutex::new(watcher));

        tracing::info!(
            "watching {} and {} every {}",
            source.display(),
            roots.prompts.display(),
            humantime::format_duration(window)
        );

        let reloader = Arc::new(Reloader::new(source, config, catalog));
        // Shared with the reloader: setting it stops both a pending publish and
        // the settled-window work below.
        let shutdown = reloader.cancel_handle();
        let repair = Arc::downgrade(&watcher);
        let cancel = Arc::clone(&shutdown);
        let task = tokio::spawn(async move {
            debounce(pending, window, move || {
                let reloader = Arc::clone(&reloader);
                let broken = Arc::clone(&broken);
                let repair = repair.clone();
                let roots = roots.clone();
                let cancel = Arc::clone(&cancel);
                async move {
                    // A shutdown signalled while this window settled: do no work
                    // and publish nothing, so a late generation cannot land on a
                    // catalog the process is stopping.
                    if cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    // Before the reload, so a re-registered watch sees whatever
                    // the resolution below is about to read.
                    if broken.swap(false, Ordering::Relaxed) {
                        re_register(&repair, &roots);
                    }
                    // Re-resolution reads and parses every prompt file. That is
                    // blocking work and has no business on a runtime worker. The
                    // watcher owns the logging, since the reload returns its
                    // outcome rather than logging it.
                    match tokio::task::spawn_blocking(move || reloader.reload()).await {
                        Ok(Ok(reload)) => {
                            if reload.retrieval_stale {
                                tracing::warn!(
                                    "reload kept the previous, now stale, retrieval index; \
                                     run_prompt is unaffected"
                                );
                            }
                        }
                        Ok(Err(error)) => {
                            if let Some(cause) = std::error::Error::source(&error) {
                                tracing::warn!("{error}: {cause}");
                            } else {
                                tracing::warn!("{error}");
                            }
                        }
                        Err(join) => tracing::error!("the reload did not finish: {join}"),
                    }
                }
            })
            .await;
        });
        Ok(Some(Watcher {
            platform: Some(watcher),
            task: Some(task),
            shutdown,
        }))
    }

    /// Signals the watch to stop and awaits its clean quiescence.
    ///
    /// Prefer this to dropping the watcher when the shutdown order matters.
    /// Dropping aborts the debounce task wherever it happens to be; this sets
    /// the shutdown flag so a reload settling right now publishes nothing,
    /// closes the event stream so the debounce loop returns, and then awaits the
    /// task - so no reload lands after the server has decided to stop.
    pub async fn shutdown(mut self) {
        // Set first, so a reload already on its blocking task publishes nothing
        // when it returns.
        self.shutdown.store(true, Ordering::SeqCst);
        // Dropping the platform watcher drops the event channel's only sender,
        // so the debounce loop settles whatever it holds and returns rather than
        // waiting on a stream nothing will feed again.
        self.platform = None;
        if let Some(task) = self.task.take() {
            match task.await {
                Ok(()) => {}
                // A cancellation is the drop path racing shutdown, not a fault.
                Err(error) if error.is_cancelled() => {}
                Err(error) => tracing::error!("the watch task did not stop cleanly: {error}"),
            }
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // The unordered fallback: a shutdown that ran already took the task, so
        // this aborts only a watcher dropped without one.
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The platform watcher itself has nothing readable to report, so what is
        // printed is the one thing a reader wants: whether this is still live.
        f.debug_struct("Watcher")
            .field(
                "watching",
                &self.task.as_ref().is_some_and(|task| !task.is_finished()),
            )
            .finish_non_exhaustive()
    }
}

/// The two paths one watcher covers.
///
/// Kept past registration because a watch that reported an error is registered
/// again from exactly these, and a re-registration that guessed at the paths
/// would be a second source of truth for what is watched.
#[derive(Debug, Clone)]
struct Roots {
    /// The prompts directory, watched recursively.
    prompts: PathBuf,
    /// The configuration file's directory, watched shallowly.
    config_dir: PathBuf,
}

impl Roots {
    /// Registers both watches, naming the path in the failure.
    ///
    /// # Errors
    /// Returns [`WatchErrorKind::Watch`] for the first path that cannot be
    /// watched.
    fn register(&self, watcher: &mut RecommendedWatcher) -> Result<(), WatchError> {
        watch(watcher, &self.prompts, RecursiveMode::Recursive)?;
        watch(watcher, &self.config_dir, RecursiveMode::NonRecursive)
    }
}

/// Registers both roots again after the platform watcher reported an error.
///
/// A `notify` error is not always fatal - a transient one leaves the watch
/// working, and re-registering a live path is what `notify` itself does when a
/// path is watched twice - so the recoverable case costs one log line and
/// carries on. The unrecoverable case is the one that matters: if the path
/// cannot be watched at all, live reload is over for this process, and saying so
/// at error level naming the path is the only way the developer whose saves stop
/// taking effect finds out. It is never worth ending a serving process over.
fn re_register(watcher: &Weak<Mutex<RecommendedWatcher>>, roots: &Roots) {
    let Some(watcher) = watcher.upgrade() else {
        // The `Watcher` was dropped: these watches are already gone, and this
        // task is on its way out with them.
        return;
    };
    // A poisoned lock means a panic while registering. The watcher behind it is
    // still a usable handle, and refusing every later repair over it would turn
    // one panic into a permanently dead watch.
    let mut watcher = watcher.lock().unwrap_or_else(PoisonError::into_inner);
    match roots.register(&mut watcher) {
        Ok(()) => tracing::info!(
            "re-registered the watch on {} and {}",
            roots.prompts.display(),
            roots.config_dir.display()
        ),
        Err(error) => tracing::error!(
            "live reload has stopped and saved prompts will no longer be picked up: {error}. \
             Restart the server once the path is back."
        ),
    }
}

/// Registers one watch, naming the path in the failure.
fn watch(
    watcher: &mut RecommendedWatcher,
    path: &Path,
    mode: RecursiveMode,
) -> Result<(), WatchError> {
    watcher
        .watch(path, mode)
        .map_err(|error| WatchError::watch(path.to_path_buf(), error))
}

/// The directory holding the configuration file, which is what gets watched.
///
/// A bare file name has no parent, and an empty parent is the working
/// directory; both become `.`, which is the directory the file is in either way.
fn config_dir(source: &Path) -> &Path {
    match source.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Runs `on_settled` once per burst of events, when the burst has stopped.
///
/// This is the debounce window with the filesystem taken out of it: the caller
/// supplies the events, so a test drives a burst on a paused clock instead of
/// provoking one and waiting. A closed channel settles whatever it was holding
/// and then returns.
///
/// The callback is a future so that a settled window can wait on work it moved
/// off this task; running the re-resolution inline would block a runtime worker
/// for the whole of it.
async fn debounce<F, Fut>(mut events: mpsc::Receiver<()>, window: Duration, mut on_settled: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    while events.recv().await.is_some() {
        loop {
            match tokio::time::timeout(window, events.recv()).await {
                // Another event inside the window: the window restarts, which is
                // what collapses one save's several events into one reload.
                Ok(Some(())) => {}
                Ok(None) => {
                    on_settled().await;
                    return;
                }
                Err(_window_closed) => break,
            }
        }
        on_settled().await;
    }
}

/// The same path without a Windows verbatim (`\\?\`) prefix.
///
/// The two sides of the comparison below are spelled by different code:
/// `std::fs::canonicalize` returns a verbatim path on Windows, while `notify`
/// delivers a plain absolute one, and neither is a prefix of the other. Both
/// sides are put through this, so the prefix cannot decide the answer. No path
/// on another platform begins with those four characters, so the strip needs no
/// `#[cfg]` that could drift. A path that is not UTF-8 is returned untouched,
/// which costs nothing: the plain absolute form matches it already.
fn plain(path: &Path) -> Cow<'_, Path> {
    match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(stripped) => Cow::Owned(PathBuf::from(stripped)),
        None => Cow::Borrowed(path),
    }
}

/// Every spelling one path might arrive as: the path itself, its absolute form,
/// and its canonicalized form, each stripped of the Windows verbatim prefix and
/// deduplicated.
///
/// `notify` does not promise which form it delivers, so all three are held. The
/// absolute form is the one that carries a relative `[paths].prompts` - the
/// shipped default - since a relative path is never a prefix of the absolute
/// path a platform watcher delivers. The canonical form is for a root the
/// backend resolved itself: fsevent canonicalizes its watch root, which is what
/// a symlinked temporary directory arrives as. Resolving them once, here, keeps
/// the syscall off the per-event path, and the watched paths do not move under a
/// running server - `[paths].prompts` is one of the settings a reload refuses to
/// apply. `std::path::absolute` touches no filesystem, resolves no symlink, and
/// answers for a path that does not exist yet.
fn forms(path: &Path) -> Vec<PathBuf> {
    let mut forms = vec![plain(path).into_owned()];
    let resolved = [
        std::path::absolute(path).ok(),
        std::fs::canonicalize(path).ok(),
    ];
    for form in resolved.into_iter().flatten() {
        let form = plain(&form).into_owned();
        if !forms.contains(&form) {
            forms.push(form);
        }
    }
    forms
}

/// Which filesystem events start a debounce window.
struct Interesting {
    /// The prompts directory in every form an event might name it by. Anything
    /// under any of them counts.
    roots: Vec<PathBuf>,
    /// The configuration file in every form an event might name it by. Only an
    /// event for exactly this file counts - matched by its whole path, not by
    /// its name, so a `prompts.toml` in some other directory the config
    /// directory happens to sit beside cannot trigger a reload.
    config: Vec<PathBuf>,
}

impl Interesting {
    /// The filter for one watcher's two roots.
    fn new(prompts: &Path, source: &Path) -> Interesting {
        Interesting {
            roots: forms(prompts),
            config: forms(source),
        }
    }

    /// Whether this event is about something the server reads.
    ///
    /// An access event is excluded because reading a prompt - which the resolver
    /// itself does - would otherwise schedule the next reload.
    fn matches(&self, event: &notify::Event) -> bool {
        if matches!(event.kind, EventKind::Access(_)) {
            return false;
        }
        event.paths.iter().any(|path| self.watched(path))
    }

    /// Whether one path is under the prompts directory or is the configuration
    /// file itself.
    fn watched(&self, path: &Path) -> bool {
        let path = plain(path);
        self.roots.iter().any(|root| path.starts_with(root))
            || self
                .config
                .iter()
                .any(|file| path.as_ref() == file.as_path())
    }
}
