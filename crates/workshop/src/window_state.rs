//! Window geometry through the workspace file. The shell restores the
//! saved size, position, and maximized flag from
//! `GET /workspace/file/current` before the window shows, writes them
//! back through `PUT /workspace/file/window-state` - debounced while the
//! user drags, once more on close - and reapplies them when the SPA opens,
//! saves as, or duplicates a workspace file (its
//! `promptforge:workspace-opened` event). The workspace file, not a
//! plugin-owned location, owns the geometry: an ephemeral workspace has
//! nowhere to keep it and the server answers `saved: false`.
//!
//! Every failure here logs and continues. Geometry is zone-two
//! degradation: a window that opens at the default size is a nuisance, a
//! window that never opens is a boot failure, and nothing in this module
//! is allowed to cause the second.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use tauri::{
    Listener as _, LogicalPosition, LogicalSize, Manager as _, PhysicalPosition, PhysicalSize,
    WebviewWindow, WindowEvent,
};

/// How long a burst of resize and move events must go quiet before the
/// geometry is written. A drag fires dozens of events a second; one save
/// per gesture is plenty.
pub(crate) const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
/// The budget for the final save on close. Past it the window closes
/// with whatever the last debounced save wrote.
pub(crate) const CLOSE_SAVE_TIMEOUT: Duration = Duration::from_secs(2);
/// The budget for a fetch or a debounced save against the in-process
/// server: loopback, so generous.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// The Tauri event the SPA emits after switching the workspace file, so
/// the shell can apply the file's geometry to the live window.
const WORKSPACE_OPENED_EVENT: &str = "promptforge:workspace-opened";
/// How far inside the saved top-left corner the monitor probe looks: a
/// window whose first title-bar pixels are on a monitor can be grabbed.
const VISIBILITY_INSET: i32 = 16;

/// The window geometry as the workspace file keeps it: the server's
/// `WindowState` mirrored field for field, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WindowState {
    /// Logical width.
    pub(crate) width: u32,
    /// Logical height.
    pub(crate) height: u32,
    /// Logical x position of the window's outer top-left corner.
    pub(crate) x: i32,
    /// Logical y position of the window's outer top-left corner.
    pub(crate) y: i32,
    /// Whether the window is maximized; the rectangle is the normal one
    /// to return to.
    pub(crate) maximized: bool,
}

/// The part of the `GET /workspace/file/current` answer the shell reads.
/// The path, name, and grants belong to the SPA and are ignored here.
#[derive(Debug, Deserialize)]
pub(crate) struct CurrentResponse {
    /// The saved geometry; `null` while ephemeral or never saved.
    pub(crate) window_state: Option<WindowState>,
}

/// The `PUT /workspace/file/window-state` answer.
#[derive(Debug, Deserialize)]
pub(crate) struct SavedResponse {
    /// Whether the geometry was written; `false` for an ephemeral
    /// workspace.
    pub(crate) saved: bool,
}

/// The shell's minimal client for the in-process server's workspace-file
/// routes. The server admits it as a native client: no `Origin`, a
/// loopback `Host`, and `application/json` on the body it sends.
#[derive(Debug, Clone)]
pub(crate) struct ServerClient {
    /// The server's bound URL, the same one the window loads.
    base_url: url::Url,
    /// The HTTP client, bounded by [`REQUEST_TIMEOUT`] per request.
    http: reqwest::Client,
}

impl ServerClient {
    /// Builds a client against the server at `base_url`.
    pub(crate) fn new(base_url: &url::Url) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("build the window-state HTTP client")?;
        Ok(Self {
            base_url: base_url.clone(),
            http,
        })
    }

    fn endpoint(&self, path: &str) -> anyhow::Result<url::Url> {
        self.base_url
            .join(path)
            .with_context(|| format!("resolve {path} against {}", self.base_url))
    }

    /// Fetches the open workspace's saved geometry: `None` while the
    /// workspace is ephemeral or has never saved one.
    pub(crate) async fn current(&self) -> anyhow::Result<Option<WindowState>> {
        let url = self.endpoint("workspace/file/current")?;
        let response = self
            .http
            .get(url)
            .send()
            .await
            .context("request the current workspace file")?
            .error_for_status()
            .context("the current-workspace request was refused")?;
        let body: CurrentResponse = response
            .json()
            .await
            .context("parse the current workspace file")?;
        Ok(body.window_state)
    }

    /// Saves `state` into the open workspace file within `timeout`.
    /// Returns whether the server wrote it (`false` for an ephemeral
    /// workspace).
    pub(crate) async fn put_window_state(
        &self,
        state: WindowState,
        timeout: Duration,
    ) -> anyhow::Result<bool> {
        let url = self.endpoint("workspace/file/window-state")?;
        let response = self
            .http
            .put(url)
            .timeout(timeout)
            .json(&state)
            .send()
            .await
            .context("send the window state")?
            .error_for_status()
            .context("the window-state save was refused")?;
        let body: SavedResponse = response
            .json()
            .await
            .context("parse the window-state answer")?;
        Ok(body.saved)
    }
}

/// The window's geometry in logical pixels, as the file keeps it. The OS
/// reports physical pixels; dividing by the scale factor keeps a window
/// the same apparent size when the file moves between displays.
pub(crate) fn geometry(
    inner: PhysicalSize<u32>,
    outer: PhysicalPosition<i32>,
    scale_factor: f64,
    maximized: bool,
) -> WindowState {
    let size: LogicalSize<u32> = inner.to_logical(scale_factor);
    let position: LogicalPosition<i32> = outer.to_logical(scale_factor);
    WindowState {
        width: size.width,
        height: size.height,
        x: position.x,
        y: position.y,
        maximized,
    }
}

/// Merges a fresh snapshot with the last known geometry. A maximized
/// window reports the screen rectangle, so only its flag is taken and the
/// normal rectangle stays what it was; anything else is taken whole.
pub(crate) fn merge(previous: Option<WindowState>, current: WindowState) -> WindowState {
    match previous {
        Some(previous) if current.maximized => WindowState {
            maximized: true,
            ..previous
        },
        _ => current,
    }
}

/// Trailing-edge debounce on one worker thread: values arriving within
/// `delay` of each other replace one another, and the sink sees only the
/// last of a burst once it goes quiet. One thread, not one per event,
/// because a drag produces hundreds.
pub(crate) struct Debouncer<T> {
    /// The event side; the worker owns the receiver.
    events: mpsc::Sender<T>,
}

impl<T> std::fmt::Debug for Debouncer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Debouncer").finish_non_exhaustive()
    }
}

impl<T: Send + 'static> Debouncer<T> {
    /// Starts the worker. If the thread cannot be spawned the debouncer
    /// still exists and silently drops every value: geometry saves are
    /// best effort.
    pub(crate) fn spawn(delay: Duration, mut sink: impl FnMut(T) + Send + 'static) -> Self {
        let (events, receiver) = mpsc::channel::<T>();
        let spawned = std::thread::Builder::new()
            .name("window-state-saver".to_owned())
            .spawn(move || {
                while let Ok(mut latest) = receiver.recv() {
                    loop {
                        match receiver.recv_timeout(delay) {
                            Ok(value) => latest = value,
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => {
                                sink(latest);
                                return;
                            }
                        }
                    }
                    sink(latest);
                }
            });
        if let Err(error) = spawned {
            eprintln!(
                "could not start the window-state saver; geometry will not be saved: {error}"
            );
        }
        Self { events }
    }

    /// Offers a value; the sink sees it only if nothing newer arrives
    /// within the delay.
    pub(crate) fn push(&self, value: T) {
        // A dead worker means saves are off; there is nothing to report
        // per event.
        let _ = self.events.send(value);
    }
}

/// The geometry last applied or saved, shared by the saver's paths so a
/// maximized snapshot can keep the normal rectangle.
type LastGeometry = Arc<Mutex<Option<WindowState>>>;

/// Restores the open workspace's saved geometry onto `window` - meant to
/// run after the server is healthy and before `show()`, so the restore
/// never flashes. Returns what was applied, for the saver to remember.
/// A missing or unusable geometry leaves the window at its defaults.
pub(crate) fn restore(window: &WebviewWindow, client: &ServerClient) -> Option<WindowState> {
    let state = match tauri::async_runtime::block_on(client.current()) {
        Ok(Some(state)) => state,
        Ok(None) => return None,
        Err(error) => {
            eprintln!(
                "could not read the saved window state; using the default geometry: {error:#}"
            );
            return None;
        }
    };
    apply(window, state)
}

/// Installs the save paths on `window`: a debounced save behind
/// [`SAVE_DEBOUNCE`] on resize and move, one best-effort save bounded by
/// [`CLOSE_SAVE_TIMEOUT`] when the window is asked to close, and a
/// listener that reapplies the geometry when the SPA switches workspace
/// files. `restored` is what [`restore`] applied, if anything.
pub(crate) fn spawn_saver(
    window: &WebviewWindow,
    client: ServerClient,
    restored: Option<WindowState>,
) {
    let last: LastGeometry = Arc::new(Mutex::new(restored));
    let saver = {
        let client = client.clone();
        Debouncer::spawn(SAVE_DEBOUNCE, move |state: WindowState| {
            save(&client, state, REQUEST_TIMEOUT);
        })
    };
    let closing = AtomicBool::new(false);
    let events_window = window.clone();
    let events_last = Arc::clone(&last);
    let events_client = client.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
            if let Some(state) = remember(&events_window, &events_last) {
                saver.push(state);
            }
        }
        WindowEvent::CloseRequested { .. } => {
            if closing.swap(true, Ordering::SeqCst) {
                return;
            }
            if let Some(state) = remember(&events_window, &events_last) {
                save(&events_client, state, CLOSE_SAVE_TIMEOUT);
            }
        }
        _ => {}
    });

    let switched_window = window.clone();
    window
        .app_handle()
        .listen_any(WORKSPACE_OPENED_EVENT, move |_event| {
            let window = switched_window.clone();
            let client = client.clone();
            let last = Arc::clone(&last);
            tauri::async_runtime::spawn(async move {
                match client.current().await {
                    Ok(Some(state)) if usable(state) => {
                        // Seed `last` before the setters run, not after:
                        // `apply` returns as soon as the setters are
                        // dispatched, while the `Resized` and `Moved`
                        // events they raise reach `remember` on the
                        // event-loop thread. A maximized snapshot merged
                        // there must keep this file's rectangle, not the
                        // previous workspace's. Then remember what was
                        // applied, since the position may have been
                        // skipped as off-monitor.
                        *last.lock().unwrap_or_else(PoisonError::into_inner) = Some(state);
                        if let Some(applied) = apply(&window, state) {
                            *last.lock().unwrap_or_else(PoisonError::into_inner) = Some(applied);
                        }
                    }
                    Ok(Some(_) | None) => {}
                    Err(error) => {
                        eprintln!(
                            "could not read the opened workspace's window state; keeping the current geometry: {error:#}"
                        );
                    }
                }
            });
        });
}

/// Snapshots the window, merges it with the last known geometry, and
/// remembers the result. `None` while minimized (the OS reports a
/// nonsense position) or when the window cannot be read.
fn remember(window: &WebviewWindow, last: &LastGeometry) -> Option<WindowState> {
    let current = snapshot(window)?;
    let mut last = last.lock().unwrap_or_else(PoisonError::into_inner);
    let merged = merge(*last, current);
    *last = Some(merged);
    Some(merged)
}

/// Reads the window's geometry in logical pixels. Runs on the event-loop
/// thread inside a window event, where the getters answer directly.
fn snapshot(window: &WebviewWindow) -> Option<WindowState> {
    let read = || -> tauri::Result<Option<WindowState>> {
        if window.is_minimized()? {
            return Ok(None);
        }
        Ok(Some(geometry(
            window.inner_size()?,
            window.outer_position()?,
            window.scale_factor()?,
            window.is_maximized()?,
        )))
    };
    match read() {
        Ok(state) => state,
        Err(error) => {
            eprintln!("could not read the window geometry; skipping this save: {error}");
            None
        }
    }
}

/// Writes `state` to the server within `timeout`, reporting a refusal
/// and shrugging off an ephemeral workspace's `saved: false`.
fn save(client: &ServerClient, state: WindowState, timeout: Duration) {
    if let Err(error) = tauri::async_runtime::block_on(client.put_window_state(state, timeout)) {
        eprintln!("could not save the window state: {error:#}");
    }
}

/// Whether a saved geometry can be applied at all. An empty size is the
/// one shape the server cannot reject for us; it is reported here and the
/// window keeps its defaults.
fn usable(state: WindowState) -> bool {
    if state.width == 0 || state.height == 0 {
        eprintln!(
            "saved window size {}x{} is empty; using the default geometry",
            state.width, state.height
        );
        return false;
    }
    true
}

/// Applies `state` to `window`: size, then position when the saved
/// corner is on a monitor that still exists, then the maximized flag.
/// Returns the geometry as applied - the window's actual position when
/// the saved one was skipped as off-monitor, so a later maximized
/// snapshot does not re-save a corner the window never occupied - or
/// `None` when the state is unusable.
fn apply(window: &WebviewWindow, state: WindowState) -> Option<WindowState> {
    if !usable(state) {
        return None;
    }
    let outcome = || -> tauri::Result<WindowState> {
        let mut applied = state;
        window.set_size(LogicalSize::new(state.width, state.height))?;
        if corner_is_visible(window, state)? {
            window.set_position(LogicalPosition::new(state.x, state.y))?;
        } else {
            eprintln!(
                "saved window position ({}, {}) is off every monitor; keeping the default position",
                state.x, state.y
            );
            let kept: LogicalPosition<i32> =
                window.outer_position()?.to_logical(window.scale_factor()?);
            applied.x = kept.x;
            applied.y = kept.y;
        }
        if state.maximized {
            window.maximize()?;
        }
        Ok(applied)
    };
    match outcome() {
        Ok(applied) => Some(applied),
        Err(error) => {
            eprintln!("could not apply the saved window state: {error}");
            None
        }
    }
}

/// Whether the saved top-left corner, inset a little so the title bar's
/// first pixels count, lands on a connected monitor. A monitor that was
/// unplugged since the save would otherwise swallow the window.
fn corner_is_visible(window: &WebviewWindow, state: WindowState) -> tauri::Result<bool> {
    let probe: PhysicalPosition<i32> = LogicalPosition::new(
        state.x.saturating_add(VISIBILITY_INSET),
        state.y.saturating_add(VISIBILITY_INSET),
    )
    .to_physical(window.scale_factor()?);
    Ok(window
        .monitor_from_point(f64::from(probe.x), f64::from(probe.y))?
        .is_some())
}

#[cfg(test)]
#[path = "window_state-tests.rs"]
mod tests;
