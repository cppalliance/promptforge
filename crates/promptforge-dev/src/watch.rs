//! Debounced watch-mode rerun loop for the interactive prompt runner.
//!
//! The filesystem watcher covers the prompt file's parent directory filtered
//! to the prompt's file name, because editors often save through an atomic
//! rename that replaces the watched inode. Raw change notifications funnel into
//! a bounded capacity-one channel, and [`rerun_on_changes`] coalesces each
//! notification burst behind a quiet period before driving one rerun. The
//! reusable [`RunEnv`] is built once and lent to every rerun, so the
//! already-running gateway's catalog, tools, and picker stay warm across saves.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use notify::{RecursiveMode, Watcher as _};
use promptforge_core::CancelHandle;
use promptforge_core::observe::Observer;
use tokio::sync::mpsc::{self, Receiver};

use crate::config::GatewayEnv;
use crate::diagnostics::{VerboseObserver, format_dev_failure};
use crate::run::{CapturePolicy, RunEnv};

/// Quiet period that must elapse after the last change notification before a
/// rerun fires, absorbing editor write-then-rename save bursts.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// What the settle logic decided to do next.
enum Settle {
    /// A debounced change is ready to rerun.
    Rerun,
    /// The loop should stop (cancelled, or the event source closed).
    Done,
    /// The watcher backend failed; stop and surface the message.
    Backend(String),
}

/// Runs `prompt` once, then reruns it after every debounced save until the
/// process is interrupted.
///
/// Each rerun re-reads, re-parses, and re-executes the file against the
/// already-running gateway, reusing the [`RunEnv`] built once here. A failed
/// run prints its error on stderr and keeps watching; results print to stdout.
///
/// # Errors
///
/// Returns an error when the prompt path has no file name, the filesystem
/// watcher cannot be installed, the run environment cannot be built, or the
/// watcher backend reports a failure that invalidates reliable watching.
pub(crate) async fn run(
    prompt: &Path,
    input: &str,
    gateway: &GatewayEnv,
    capture: CapturePolicy,
    cancel: &CancelHandle,
) -> Result<()> {
    let file_name = prompt
        .file_name()
        .with_context(|| format!("{} names no file to watch", prompt.display()))?
        .to_owned();
    let directory = watched_directory(prompt);

    // Capacity-one wake channel: a full channel already records a pending
    // change, so a noisy watcher or a slow rerun cannot grow an unbounded
    // backlog. A backend error is recorded in `backend_error` (which cannot be
    // lost even when the wake channel is full) and then wakes the loop.
    let (sender, mut receiver) = mpsc::channel::<()>(1);
    let backend_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let callback_backend = Arc::clone(&backend_error);
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        match event {
            Ok(event) if event_touches(&event, &file_name) => {
                // Full means a change is already pending; closed means shutdown.
                let _ignored = sender.try_send(());
            }
            Ok(_) => {}
            Err(error) => {
                // Record the backend error where it cannot be dropped even if the
                // wake channel is full, then wake the loop so it surfaces it
                // rather than parking on stale coverage.
                if let Ok(mut slot) = callback_backend.lock() {
                    slot.get_or_insert_with(|| error.to_string());
                }
                let _ignored = sender.try_send(());
            }
        }
    })
    .context("create the prompt file watcher")?;
    watcher
        .watch(&directory, RecursiveMode::NonRecursive)
        .with_context(|| format!("watch {}", directory.display()))?;

    eprintln!(
        "watching {} for changes; press Ctrl-C to stop",
        prompt.display()
    );

    // Build the reusable run environment once (fetching the catalog once), then
    // lend it to every rerun.
    let env = RunEnv::initialize(gateway, capture).await?;
    let env_ref = &env;

    run_and_report(env_ref, prompt, input, cancel).await;
    let outcome = rerun_on_changes(
        &mut receiver,
        DEBOUNCE,
        cancel,
        &backend_error,
        move || async move {
            eprintln!("{} changed; rerunning", prompt.display());
            run_and_report(env_ref, prompt, input, cancel).await;
        },
    )
    .await;

    // Keep the watcher alive until the loop is done.
    drop(watcher);

    outcome?;
    if cancel.is_cancelled() {
        bail!("interrupted by Ctrl-C");
    }
    Ok(())
}

/// Returns the directory whose entries the watcher observes: the prompt's
/// parent, or the current directory for a bare file name.
fn watched_directory(prompt: &Path) -> PathBuf {
    match prompt.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        Some(_) | None => PathBuf::from("."),
    }
}

/// Reports whether `event` names the watched file.
fn event_touches(event: &notify::Event, file_name: &OsStr) -> bool {
    event
        .paths
        .iter()
        .any(|path| path.file_name() == Some(file_name))
}

/// Drives `rerun` once per debounced change until the event source closes or
/// cancellation fires.
///
/// # Errors
/// Returns an error when the watcher backend reports a failure.
async fn rerun_on_changes<F, Fut>(
    receiver: &mut Receiver<()>,
    debounce: Duration,
    cancel: &CancelHandle,
    backend: &Mutex<Option<String>>,
    mut rerun: F,
) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    loop {
        match next_rerun(receiver, debounce, cancel, backend).await {
            Settle::Rerun => rerun().await,
            Settle::Done => return Ok(()),
            Settle::Backend(message) => {
                bail!("prompt file watcher reported an error, stopping: {message}")
            }
        }
    }
}

/// Takes the recorded backend error, if any, leaving the slot empty.
fn take_backend(backend: &Mutex<Option<String>>) -> Option<String> {
    backend
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

/// Waits for one change notification, then absorbs every further notification
/// until `debounce` of quiet passes.
///
/// [`Settle::Rerun`] means a settled change awaits a rerun, [`Settle::Done`]
/// means the source closed or cancellation fired, and [`Settle::Backend`]
/// carries a watcher backend failure.
async fn next_rerun(
    receiver: &mut Receiver<()>,
    debounce: Duration,
    cancel: &CancelHandle,
    backend: &Mutex<Option<String>>,
) -> Settle {
    tokio::select! {
        biased;
        () = cancel.cancelled() => return Settle::Done,
        msg = receiver.recv() => match msg {
            None => return Settle::Done,
            // Every wake checks the loss-proof backend slot first, so a backend
            // error surfaces even if its own wake send was dropped (channel full).
            Some(()) => {
                if let Some(message) = take_backend(backend) {
                    return Settle::Backend(message);
                }
            }
        }
    }
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Settle::Done,
            timed = tokio::time::timeout(debounce, receiver.recv()) => {
                match timed {
                    Ok(Some(())) => {
                        if let Some(message) = take_backend(backend) {
                            return Settle::Backend(message);
                        }
                    }
                    Ok(None) | Err(_) => return Settle::Rerun,
                }
            }
        }
    }
}

/// Runs the prompt once, printing the result to stdout or the error to stderr.
async fn run_and_report(env: &RunEnv, prompt: &Path, input: &str, cancel: &CancelHandle) {
    let observer: Arc<dyn Observer> = Arc::new(VerboseObserver::new(std::io::stderr()));
    match env
        .run_prompt(prompt, input, observer, cancel.clone())
        .await
    {
        Ok(result) => println!("{result}"),
        Err(error) => {
            eprintln!("{}", format_dev_failure(prompt, &error));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use notify::EventKind;
    use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};

    use super::*;

    fn event(kind: EventKind, paths: &[&str]) -> notify::Event {
        paths.iter().fold(notify::Event::new(kind), |event, path| {
            event.add_path(PathBuf::from(path))
        })
    }

    #[test]
    fn events_naming_the_watched_file_match_regardless_of_kind_or_directory() {
        let file_name = OsStr::new("prompt.md");
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            EventKind::Remove(RemoveKind::File),
        ] {
            assert!(
                event_touches(&event(kind, &["/some/where/prompt.md"]), file_name),
                "{kind:?} naming the watched file must match"
            );
        }
        assert!(event_touches(
            &event(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &["/w/.prompt.md.swp", "/w/prompt.md"],
            ),
            file_name,
        ));
    }

    #[test]
    fn events_from_the_store_directory_never_match_the_watched_prompt() {
        let file_name = OsStr::new("prompt.md");
        for path in [
            "/w/prompt",
            "/w/prompt/evidence.md",
            "/w/prompt/notes/deep.txt",
        ] {
            assert!(
                !event_touches(
                    &event(EventKind::Create(CreateKind::Any), &[path]),
                    file_name
                ),
                "{path} must not trigger a rerun"
            );
        }
    }

    #[test]
    fn events_for_other_files_or_without_paths_are_ignored() {
        let file_name = OsStr::new("prompt.md");
        assert!(!event_touches(
            &event(EventKind::Modify(ModifyKind::Any), &["/w/other.md"]),
            file_name,
        ));
        assert!(!event_touches(
            &event(EventKind::Modify(ModifyKind::Any), &[]),
            file_name,
        ));
    }

    #[test]
    fn watched_directory_falls_back_to_the_current_directory() {
        assert_eq!(
            watched_directory(Path::new("prompts/demo.md")),
            PathBuf::from("prompts")
        );
        assert_eq!(watched_directory(Path::new("demo.md")), PathBuf::from("."));
    }

    fn no_backend() -> Mutex<Option<String>> {
        Mutex::new(None)
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_events_coalesces_into_one_rerun_per_quiet_period() {
        let (sender, mut receiver) = mpsc::channel::<()>(1);
        let driver = tokio::spawn(async move {
            for _ in 0..3 {
                let _ignored = sender.try_send(());
                tokio::time::advance(Duration::from_millis(100)).await;
            }
            // Quiet long enough for the first rerun to fire, then one more
            // settled change before the source closes. Virtual time only.
            tokio::time::advance(Duration::from_millis(400)).await;
            let _ignored = sender.try_send(());
        });

        let cancel = CancelHandle::new();
        let backend = no_backend();
        let mut reruns = 0_u32;
        rerun_on_changes(
            &mut receiver,
            Duration::from_millis(300),
            &cancel,
            &backend,
            || {
                reruns += 1;
                async {}
            },
        )
        .await
        .expect("no backend error");

        driver.await.expect("event driver must not panic");
        assert_eq!(reruns, 2, "three-burst then one change must rerun twice");
    }

    #[tokio::test(start_paused = true)]
    async fn a_change_already_queued_when_the_source_closes_still_reruns() {
        let (sender, mut receiver) = mpsc::channel::<()>(1);
        sender
            .try_send(())
            .expect("fresh channel accepts one event");
        drop(sender);

        let cancel = CancelHandle::new();
        let backend = no_backend();
        let mut reruns = 0_u32;
        rerun_on_changes(
            &mut receiver,
            Duration::from_millis(300),
            &cancel,
            &backend,
            || {
                reruns += 1;
                async {}
            },
        )
        .await
        .expect("no backend error");

        assert_eq!(reruns, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_source_that_closes_without_events_never_reruns() {
        let (sender, mut receiver) = mpsc::channel::<()>(1);
        drop(sender);

        let cancel = CancelHandle::new();
        let backend = no_backend();
        let mut reruns = 0_u32;
        rerun_on_changes(
            &mut receiver,
            Duration::from_millis(300),
            &cancel,
            &backend,
            || {
                reruns += 1;
                async {}
            },
        )
        .await
        .expect("no backend error");

        assert_eq!(reruns, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_backend_error_stops_the_loop_with_a_contextual_error() {
        let (sender, mut receiver) = mpsc::channel::<()>(1);
        let backend = Mutex::new(Some("watch coverage lost".to_owned()));
        sender.try_send(()).expect("wake the loop");

        let cancel = CancelHandle::new();
        let mut reruns = 0_u32;
        let error = rerun_on_changes(
            &mut receiver,
            Duration::from_millis(300),
            &cancel,
            &backend,
            || {
                reruns += 1;
                async {}
            },
        )
        .await
        .expect_err("a backend error must stop the loop");

        assert!(
            format!("{error:#}").contains("watch coverage lost"),
            "unexpected error: {error:#}"
        );
        assert_eq!(reruns, 0, "a backend error must not drive a rerun");
    }

    #[tokio::test(start_paused = true)]
    async fn a_backend_error_is_not_lost_when_a_change_is_already_queued() {
        // The single wake slot is already occupied by a pending change, so the
        // backend error's own wake send is dropped (full). It must still surface
        // because the error is recorded in the loss-proof slot, not the channel.
        let (sender, mut receiver) = mpsc::channel::<()>(1);
        sender
            .try_send(())
            .expect("occupy the wake slot with a change");
        assert!(
            sender.try_send(()).is_err(),
            "the wake channel must now be full so the error's wake is dropped"
        );
        let backend = Mutex::new(Some("coverage lost while a change was pending".to_owned()));

        let cancel = CancelHandle::new();
        let mut reruns = 0_u32;
        let error = rerun_on_changes(
            &mut receiver,
            Duration::from_millis(300),
            &cancel,
            &backend,
            || {
                reruns += 1;
                async {}
            },
        )
        .await
        .expect_err("the backend error must not be dropped");

        assert!(
            format!("{error:#}").contains("coverage lost while a change was pending"),
            "unexpected error: {error:#}"
        );
        assert_eq!(reruns, 0, "the error must win over the pending change");
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_while_idle_stops_without_rerunning() {
        // Keep the sender alive so only cancellation, not channel closure, ends
        // the loop.
        let (_sender, mut receiver) = mpsc::channel::<()>(1);
        let cancel = CancelHandle::new();
        let signal = cancel.clone();
        tokio::spawn(async move { signal.cancel() });

        let backend = no_backend();
        let mut reruns = 0_u32;
        rerun_on_changes(
            &mut receiver,
            Duration::from_millis(300),
            &cancel,
            &backend,
            || {
                reruns += 1;
                async {}
            },
        )
        .await
        .expect("no backend error");

        assert_eq!(reruns, 0, "cancelling an idle wait must not rerun");
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_during_debounce_stops_without_a_final_rerun() {
        let (sender, mut receiver) = mpsc::channel::<()>(1);
        sender
            .try_send(())
            .expect("fresh channel accepts one event");
        let cancel = CancelHandle::new();
        let signal = cancel.clone();
        // Cancels once the loop parks in the debounce wait.
        tokio::spawn(async move { signal.cancel() });

        let backend = no_backend();
        let mut reruns = 0_u32;
        rerun_on_changes(
            &mut receiver,
            Duration::from_millis(300),
            &cancel,
            &backend,
            || {
                reruns += 1;
                async {}
            },
        )
        .await
        .expect("no backend error");

        assert_eq!(
            reruns, 0,
            "cancelling during the quiet period must not fire a final rerun"
        );
        drop(sender);
    }
}
