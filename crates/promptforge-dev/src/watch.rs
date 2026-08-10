//! Debounced watch-mode rerun loop for the interactive prompt runner.
//!
//! The filesystem watcher covers the prompt file's parent directory filtered
//! to the prompt's file name, because editors often save through an atomic
//! rename that replaces the watched inode. Raw change notifications funnel
//! into a channel, and [`rerun_on_changes`] coalesces each notification burst
//! behind a quiet period before driving one rerun. The already-running gateway
//! stays warm across every rerun; this crate never starts it.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use notify::{RecursiveMode, Watcher as _};
use promptforge_core::CancelHandle;
use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::run;

/// Quiet period that must elapse after the last change notification before a
/// rerun fires, absorbing editor write-then-rename save bursts.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Runs `prompt` once, then reruns it after every debounced save until the
/// process is interrupted.
///
/// Each rerun re-reads, re-parses, and re-executes the file against
/// the already-running gateway named by the process environment, and dumps
/// the run's store beside the prompt; the watcher's file-name filter keeps
/// those dump writes from feeding back into the rerun loop. A failed run
/// prints its error on stderr and keeps watching; results print to stdout.
/// The loop itself never returns except through an error while installing
/// the watcher.
///
/// # Errors
///
/// Returns an error when the prompt path has no file name or the filesystem
/// watcher cannot be installed on its parent directory.
pub(crate) async fn run(prompt: &Path, input: &str, cancel: &CancelHandle) -> Result<()> {
    let file_name = prompt
        .file_name()
        .with_context(|| format!("{} names no file to watch", prompt.display()))?
        .to_owned();
    let directory = watched_directory(prompt);

    let (sender, mut receiver) = mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // A backend error is not actionable mid-loop and the next save still
        // produces an event, so errors are deliberately dropped.
        if let Ok(event) = event
            && event_touches(&event, &file_name)
        {
            // A closed channel only means the loop is shutting down.
            let _ignored = sender.send(());
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
    run_and_report(prompt, input, cancel).await;
    rerun_on_changes(&mut receiver, DEBOUNCE, cancel, move || async move {
        eprintln!("{} changed; rerunning", prompt.display());
        run_and_report(prompt, input, cancel).await;
    })
    .await;
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
///
/// Matching is by final path component because a save may arrive as a
/// create, modify, rename, or remove event whose paths differ in everything
/// but the file name.
fn event_touches(event: &notify::Event, file_name: &OsStr) -> bool {
    event
        .paths
        .iter()
        .any(|path| path.file_name() == Some(file_name))
}

/// Drives `rerun` once per debounced change until the event source closes.
///
/// This is the seam the watch loop and its tests share: tests inject events
/// through the channel and observe rerun requests without any filesystem
/// watcher or server.
async fn rerun_on_changes<F, Fut>(
    receiver: &mut UnboundedReceiver<()>,
    debounce: Duration,
    cancel: &CancelHandle,
    mut rerun: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    while next_rerun(receiver, debounce, cancel).await {
        rerun().await;
    }
}

/// Waits for one change notification, then absorbs every further
/// notification until `debounce` of quiet passes.
///
/// Returns `true` when a settled change awaits a rerun and `false` once the
/// event source closes with nothing pending. A source that closes with a
/// change already received still yields that final rerun first.
async fn next_rerun(
    receiver: &mut UnboundedReceiver<()>,
    debounce: Duration,
    cancel: &CancelHandle,
) -> bool {
    tokio::select! {
        biased;
        () = cancel.cancelled() => return false,
        msg = receiver.recv() => {
            if msg.is_none() {
                return false;
            }
        }
    }
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return false,
            timed = tokio::time::timeout(debounce, receiver.recv()) => {
                match timed {
                    Ok(Some(())) => {}
                    Ok(None) | Err(_) => return true,
                }
            }
        }
    }
}

/// Runs the prompt once, printing the result to stdout or the error to stderr.
async fn run_and_report(prompt: &Path, input: &str, cancel: &CancelHandle) {
    match run::run_once(prompt, input, cancel.clone()).await {
        Ok(result) => println!("{result}"),
        Err(error) => {
            eprintln!("{}", run::format_dev_failure(prompt, &error));
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
    fn events_from_the_store_dump_directory_never_match_the_watched_prompt() {
        // Every rerun dumps the store into `<prompt-stem>.store` beside the
        // prompt, inside the watched directory; those writes must not feed
        // back into the rerun loop.
        let file_name = OsStr::new("prompt.md");
        for path in [
            "/w/prompt.store",
            "/w/prompt.store/evidence.md",
            "/w/prompt.store/notes/deep.txt",
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

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_events_coalesces_into_one_rerun_per_quiet_period() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let driver = tokio::spawn(async move {
            for _ in 0..3 {
                sender.send(()).expect("rerun loop must be receiving");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            // Quiet long enough for the first rerun to fire, then one more
            // settled change before the source closes.
            tokio::time::sleep(Duration::from_millis(400)).await;
            sender.send(()).expect("rerun loop must be receiving");
        });

        let mut reruns = 0_u32;
        rerun_on_changes(&mut receiver, Duration::from_millis(300), || {
            reruns += 1;
            async {}
        })
        .await;

        driver.await.expect("event driver must not panic");
        assert_eq!(reruns, 2, "three-burst then one change must rerun twice");
    }

    #[tokio::test(start_paused = true)]
    async fn a_change_already_queued_when_the_source_closes_still_reruns() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        sender.send(()).expect("fresh channel accepts one event");
        drop(sender);

        let mut reruns = 0_u32;
        rerun_on_changes(&mut receiver, Duration::from_millis(300), || {
            reruns += 1;
            async {}
        })
        .await;

        assert_eq!(reruns, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_source_that_closes_without_events_never_reruns() {
        let (sender, mut receiver) = mpsc::unbounded_channel::<()>();
        drop(sender);

        let mut reruns = 0_u32;
        rerun_on_changes(&mut receiver, Duration::from_millis(300), || {
            reruns += 1;
            async {}
        })
        .await;

        assert_eq!(reruns, 0);
    }
}
