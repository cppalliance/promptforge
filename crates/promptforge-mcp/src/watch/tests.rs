//! Watcher tests: the debounce window, the event filter, and starting and
//! stopping.
//!
//! The debounce window is driven by sending tokens on a paused clock, so a burst
//! is a burst by construction rather than by racing a real editor. The event
//! filter is a pure function over a synthesized event. What a settled window
//! *does* is a separate question, tested beside the reload itself: whether a save
//! is seen and what happens when it is share nothing but a token, and only the
//! first needs a real filesystem event, which one integration test covers.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notify::EventKind;
use notify::event::{AccessKind, CreateKind, ModifyKind};
use tokio::sync::mpsc;

use super::fixture::config_source;
use super::sessions::{ListChanged, Sessions};
use super::{Interesting, Watcher, debounce};
use crate::catalog::{Catalog, CatalogHandle};
use crate::config::Config;

/// The debounce window every timing test uses. Its value is immaterial on a
/// paused clock; what matters is that the test advances past it.
const WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

// ---------------------------------------------------------------------------
// The debounce window
// ---------------------------------------------------------------------------

/// Drives `count` tokens through the window and reports how often it settled.
async fn settlements(bursts: &[usize]) -> usize {
    let settled = Arc::new(AtomicUsize::new(0));
    let (events, pending) = mpsc::channel(64);
    let counter = Arc::clone(&settled);
    let window = tokio::spawn(async move {
        debounce(pending, WINDOW, move || {
            let counter = Arc::clone(&counter);
            async move {
                let _previous = counter.fetch_add(1, Ordering::Relaxed);
            }
        })
        .await;
    });
    for &burst in bursts {
        for _ in 0..burst {
            events.send(()).await.expect("the window is still running");
        }
        // The clock is paused, so this advances instantly and lets the window
        // close before the next burst opens.
        tokio::time::sleep(WINDOW * 3).await;
    }
    drop(events);
    window.await.expect("the window task finishes");
    settled.load(Ordering::Relaxed)
}

#[tokio::test(start_paused = true)]
async fn one_burst_of_events_re_resolves_once() {
    // A Windows editor writing through a temporary fires several events per
    // save. The window is what makes that one reload rather than five.
    assert_eq!(settlements(&[5]).await, 1);
}

#[tokio::test(start_paused = true)]
async fn a_single_event_re_resolves_once() {
    assert_eq!(settlements(&[1]).await, 1);
}

#[tokio::test(start_paused = true)]
async fn two_separated_bursts_re_resolve_twice() {
    assert_eq!(settlements(&[3, 4]).await, 2);
}

#[tokio::test(start_paused = true)]
async fn a_closed_queue_settles_what_it_was_holding() {
    // The watcher was dropped mid-window: whatever was already seen is still
    // worth resolving, and then the window is done.
    let settled = Arc::new(AtomicUsize::new(0));
    let (events, pending) = mpsc::channel(4);
    let counter = Arc::clone(&settled);
    let window = tokio::spawn(async move {
        debounce(pending, WINDOW, move || {
            let counter = Arc::clone(&counter);
            async move {
                let _previous = counter.fetch_add(1, Ordering::Relaxed);
            }
        })
        .await;
    });
    events.send(()).await.expect("the window is running");
    drop(events);
    window.await.expect("the window task finishes");
    assert_eq!(settled.load(Ordering::Relaxed), 1);
}

#[tokio::test(start_paused = true)]
async fn a_settled_window_waits_for_the_work_it_moved_off_this_task() {
    // The reload runs on `spawn_blocking`, so the window has to await it: a
    // window that returned before its own reload finished could open the next
    // one over a resolution still in flight.
    let done = Arc::new(AtomicUsize::new(0));
    let (events, pending) = mpsc::channel(4);
    let counter = Arc::clone(&done);
    let window = tokio::spawn(async move {
        debounce(pending, WINDOW, move || {
            let counter = Arc::clone(&counter);
            async move {
                let finished =
                    tokio::task::spawn_blocking(move || counter.fetch_add(1, Ordering::Relaxed))
                        .await;
                assert!(finished.is_ok(), "the blocking reload finishes");
            }
        })
        .await;
    });
    events.send(()).await.expect("the window is running");
    drop(events);
    window.await.expect("the window task finishes");
    assert_eq!(done.load(Ordering::Relaxed), 1);
}

// ---------------------------------------------------------------------------
// The event filter
// ---------------------------------------------------------------------------

/// The filter for a root holding `prompts/` and `prompts.toml`.
fn filter(root: &Path) -> Interesting {
    Interesting::new(&root.join("prompts"), &root.join("prompts.toml"))
}

/// A modification event for one path.
fn modified(path: &Path) -> notify::Event {
    notify::Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path.to_path_buf())
}

#[test]
fn a_prompt_under_the_watched_directory_is_interesting() {
    let root = Path::new("/srv/pf");
    assert!(filter(root).matches(&modified(&root.join("prompts").join("alpha.md"))));
    assert!(filter(root).matches(&modified(
        &root.join("prompts").join("nested").join("beta.md")
    )));
}

#[test]
fn the_configuration_file_is_interesting() {
    let root = Path::new("/srv/pf");
    assert!(filter(root).matches(&modified(&root.join("prompts.toml"))));
    assert!(filter(root).matches(
        &notify::Event::new(EventKind::Create(CreateKind::Any)).add_path(root.join("prompts.toml"))
    ));
}

#[test]
fn another_file_beside_the_configuration_is_not() {
    // The configuration's whole directory is watched, because an editor that
    // saves through a temporary replaces the file itself. Its neighbours are
    // filtered out by name.
    let root = Path::new("/srv/pf");
    assert!(!filter(root).matches(&modified(&root.join("Cargo.lock"))));
    assert!(!filter(root).matches(&modified(&root.join("gateway.toml"))));
}

#[test]
fn reading_a_prompt_is_not_a_change() {
    // The resolver itself reads every prompt, so an access event would schedule
    // the next reload from the last one.
    let root = Path::new("/srv/pf");
    let read = notify::Event::new(EventKind::Access(AccessKind::Read))
        .add_path(root.join("prompts").join("alpha.md"));
    assert!(!filter(root).matches(&read));
}

#[test]
fn a_relative_configured_path_still_matches_the_event_it_gets() {
    // `[paths].prompts` defaults to the relative `prompts`, and a relative path
    // is never a prefix of the absolute one a platform watcher delivers. Without
    // the canonical form, every event under a relatively configured directory is
    // dropped and the server watches successfully and never reloads. The crate's
    // own `src` stands in for it: the test needs a relative path that exists,
    // and reads nothing.
    let relative = Path::new("src");
    let absolute = fs::canonicalize(relative).expect("the crate's own source directory");
    let interesting = Interesting::new(relative, Path::new("prompts.toml"));

    assert!(
        interesting.matches(&modified(&absolute.join("watch.rs"))),
        "an event delivered in canonical form is an event under the watched root"
    );
    assert!(
        interesting.matches(&modified(&relative.join("watch.rs"))),
        "the configured form still matches, since a backend may deliver either"
    );
}

#[test]
fn a_symlinked_or_dotted_path_still_matches_the_event_it_gets() {
    // What a temporary directory is on macOS, and what any path through a
    // symlink is anywhere: the configured spelling and the canonical one differ,
    // and only one of the two arrives.
    let dir = tempfile::tempdir().expect("create a temporary root");
    let root = dir.path();
    fs::create_dir_all(root.join("prompts")).expect("create the prompts directory");
    let dotted = root.join(".").join("prompts");
    let canonical = fs::canonicalize(root.join("prompts")).expect("canonicalize the root");
    let interesting = Interesting::new(&dotted, &root.join("prompts.toml"));

    assert!(interesting.matches(&modified(&canonical.join("alpha.md"))));
}

// ---------------------------------------------------------------------------
// Starting and stopping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn watch_false_starts_nothing() {
    let dir = tempfile::tempdir().expect("create a temporary root");
    let root = dir.path();
    fs::create_dir_all(root.join("prompts")).expect("create the prompts directory");
    let source = root.join("prompts.toml");
    fs::write(&source, config_source(root, "watch = false\n")).expect("write the configuration");
    let config = Config::load(&source).expect("the configuration loads");
    let catalog = Arc::new(CatalogHandle::new(Catalog::new(Vec::new())));

    let watcher = Watcher::start(
        &source,
        Arc::new(config),
        catalog,
        Arc::new(Sessions::new()),
    )
    .expect("starting nothing cannot fail");
    assert!(
        watcher.is_none(),
        "watch = false leaves the catalog exactly as boot resolved it"
    );
}

#[tokio::test]
async fn an_unwatchable_prompts_directory_is_an_error() {
    let dir = tempfile::tempdir().expect("create a temporary root");
    let root = dir.path();
    let source = root.join("prompts.toml");
    // The configuration names a prompts directory that does not exist: nothing
    // to watch, and an operator has to hear about it rather than lose live
    // reload silently.
    fs::write(&source, config_source(root, "")).expect("write the configuration");
    let config = Config::load(&source).expect("the configuration loads");
    let catalog = Arc::new(CatalogHandle::new(Catalog::new(Vec::new())));

    let started = Watcher::start(
        &source,
        Arc::new(config),
        catalog,
        Arc::new(Sessions::new()),
    );
    assert!(started.is_err());
}

#[tokio::test]
async fn an_empty_session_list_announces_to_nobody() {
    let sessions = Sessions::new();
    assert!(sessions.is_empty());
    sessions.list_changed();
    assert_eq!(sessions.len(), 0);
}
