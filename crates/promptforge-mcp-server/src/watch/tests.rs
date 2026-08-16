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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use notify::EventKind;
use notify::event::{AccessKind, CreateKind, ModifyKind};
use rmcp::model::{CallToolRequestParams, CallToolResult, JsonObject};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::fixture::{Fixture, config_source};
use super::{Interesting, Watcher, debounce};
use crate::catalog::{Catalog, CatalogHandle};
use crate::config::Config;
use crate::error::WatchErrorKind;
use crate::server::{PreparedTools, PromptForgeServer};

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

/// The same absolute path spelled as a Windows verbatim path, which is the form
/// `std::fs::canonicalize` returns there and the form `notify` does not deliver.
fn verbatim(path: &Path) -> PathBuf {
    PathBuf::from(format!(r"\\?\{}", path.display()))
}

/// An absolute spelling of `path`, resolving no symlink and reading no
/// directory, so the result holds whether or not the path exists.
fn absolute(path: &str) -> PathBuf {
    std::path::absolute(path).expect("an absolute spelling of a relative path")
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
fn a_same_named_configuration_file_elsewhere_is_not_interesting() {
    // The whole configuration path is matched, not just its name: a
    // `prompts.toml` in another directory is a different file, and a save to it
    // must not trigger this server's reload.
    let root = Path::new("/srv/pf");
    let interesting = filter(root);
    assert!(
        interesting.matches(&modified(&root.join("prompts.toml"))),
        "the watched configuration file still counts"
    );
    assert!(
        !interesting.matches(&modified(Path::new("/somewhere/else/prompts.toml"))),
        "a file with the same name in another directory is not the watched one"
    );
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
fn a_relative_configured_root_matches_a_plain_absolute_event_path() {
    // The Windows case, and the one that shipped broken: `[paths].prompts` is
    // the relative `prompts` and `notify` delivers a plain absolute path.
    // Neither the configured spelling nor the verbatim path `canonicalize`
    // returns there is a prefix of that, so every event was dropped and the
    // server watched successfully and never reloaded. The crate's own `src`
    // stands in for the directory: the test reads nothing under it.
    let interesting = Interesting::new(Path::new("src"), Path::new("prompts.toml"));

    assert!(interesting.matches(&modified(&absolute("src").join("watch.rs"))));
    assert!(
        interesting.matches(&modified(&absolute("src").join("watch").join("tests.rs"))),
        "a nested path under the root counts too"
    );
}

#[test]
fn a_relative_configured_root_matches_a_verbatim_event_path() {
    // The other direction, which is what a backend that canonicalizes its own
    // watch root delivers on Windows.
    let interesting = Interesting::new(Path::new("src"), Path::new("prompts.toml"));

    assert!(interesting.matches(&modified(&verbatim(&absolute("src").join("watch.rs")))));
}

#[test]
fn an_absolute_configured_root_matches_an_event_path_in_the_other_form() {
    // Neither side may be allowed to decide the answer by its spelling: a
    // verbatim prefix on one and not the other is not a difference in path.
    let plain_root = Interesting::new(&absolute("src"), Path::new("prompts.toml"));
    assert!(plain_root.matches(&modified(&verbatim(&absolute("src").join("watch.rs")))));

    let verbatim_root = Interesting::new(&verbatim(&absolute("src")), Path::new("prompts.toml"));
    assert!(verbatim_root.matches(&modified(&absolute("src").join("watch.rs"))));
}

#[test]
fn a_path_outside_the_prompts_directory_is_still_not_interesting() {
    // Normalizing both sides must not widen what matches: a sibling of the
    // watched directory is not under it in any spelling.
    let interesting = Interesting::new(Path::new("src"), Path::new("prompts.toml"));
    let sibling = absolute("Cargo.toml");

    assert!(!interesting.matches(&modified(&sibling)));
    assert!(!interesting.matches(&modified(&verbatim(&sibling))));
    assert!(
        !interesting.matches(&modified(Path::new("/elsewhere/src/watch.rs"))),
        "a directory named the same somewhere else is not the watched root"
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

    let watcher =
        Watcher::start(&source, Arc::new(config), catalog).expect("starting nothing cannot fail");
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

    let started = Watcher::start(&source, Arc::new(config), catalog);
    assert!(started.is_err());
}

#[tokio::test]
async fn shutdown_signals_and_awaits_a_clean_stop() {
    let dir = tempfile::tempdir().expect("create a temporary root");
    let root = dir.path();
    fs::create_dir_all(root.join("prompts")).expect("create the prompts directory");
    let source = root.join("prompts.toml");
    fs::write(&source, config_source(root, "")).expect("write the configuration");
    let config = Config::load(&source).expect("the configuration loads");
    let catalog = Arc::new(CatalogHandle::new(Catalog::new(Vec::new())));

    let watcher = Watcher::start(&source, Arc::new(config), catalog)
        .expect("the watch starts")
        .expect("watch is on by default");
    // Returns only once the debounce task has ended: a shutdown that failed to
    // close the event stream, or failed to await, would hang this test rather
    // than pass it.
    watcher.shutdown().await;
}

// ---------------------------------------------------------------------------
// The fixture's own encoding
// ---------------------------------------------------------------------------

/// One `tools/call` request over an object of arguments.
fn call(name: &'static str, arguments: Value) -> CallToolRequestParams {
    let arguments: JsonObject = match arguments {
        Value::Object(map) => map,
        other => panic!("arguments must be an object, got {other}"),
    };
    CallToolRequestParams::new(name).with_arguments(arguments)
}

/// The text of a result's single content block.
fn text_of(result: &CallToolResult) -> String {
    let [block] = result.content.as_slice() else {
        panic!("expected exactly one content block")
    };
    block.as_text().expect("the block is text").text.clone()
}

#[tokio::test]
async fn a_value_with_quotes_and_newlines_round_trips_through_the_fixture() {
    // The fixture encodes the description into a YAML scalar and the value into
    // a Lua literal. Content with a quote, a newline, or a YAML/Lua
    // metacharacter must carry through as data: raw interpolation would have
    // terminated the scalar or spilled onto the next line, producing a file that
    // would not resolve or one that resolved to something other than what was
    // written.
    let fixture = Fixture::new();
    let server = PromptForgeServer::new(
        Arc::clone(&fixture.config),
        Arc::clone(&fixture.catalog),
        Arc::new(
            PreparedTools::new(
                &fixture.config.gateway,
                &fixture.config.tools,
                promptforge_core::model::ModelCatalog::empty(),
            )
            .expect("prepare fixture live tools"),
        ),
    );

    let value = "line one's \"quote\"\nline two: [[bracket]] and a \\ backslash\ttab";
    let description = "a description with 'quotes', \"more\",\nand a newline";
    fixture.rewrite("alpha", description, value);
    assert!(
        fixture
            .reload()
            .expect("a value with quotes and newlines still resolves")
            .published,
        "the escaped fixture content resolves"
    );

    let ran = server
        .dispatch(call("run_prompt", json!({ "prompt": "alpha" })))
        .await
        .expect("the runner answers");
    assert_eq!(
        text_of(&ran),
        value,
        "the value round-trips verbatim through the Lua literal"
    );
    assert_eq!(
        fixture.description("alpha"),
        description,
        "the description round-trips verbatim through the YAML scalar"
    );
}

#[test]
fn starting_outside_a_runtime_is_a_typed_error_rather_than_a_panic() {
    // A plain `#[test]`, so there is no ambient Tokio runtime. The debounce
    // task has nowhere to run, and an operator has to hear about that as a
    // returned error rather than as a panic out of the first spawn.
    let dir = tempfile::tempdir().expect("create a temporary root");
    let root = dir.path();
    fs::create_dir_all(root.join("prompts")).expect("create the prompts directory");
    let source = root.join("prompts.toml");
    fs::write(&source, config_source(root, "")).expect("write the configuration");
    let config = Config::load(&source).expect("the configuration loads");
    let catalog = Arc::new(CatalogHandle::new(Catalog::new(Vec::new())));

    let error = Watcher::start(&source, Arc::new(config), catalog)
        .expect_err("no runtime is an error, not a panic");
    assert_eq!(error.kind(), WatchErrorKind::Runtime);
}
