//! Post-run store dumps for the interactive prompt runner.
//!
//! After every run, [`dump_store`] copies the run's virtual files to a
//! directory next to the prompt file named `<prompt-stem>.store`, so a prompt
//! author can inspect what the prompt wrote. The directory is cleared before
//! each dump so stale files never masquerade as the current run, and it is
//! removed entirely when the store is empty. StoreRef paths that cannot map to a
//! safe relative filesystem path are reported on the caller's status sink and
//! skipped; nothing is ever written outside the dump directory.
//!
//! The runner also buffers raw model turns through [`TraceCapture`] and writes
//! them under `<prompt-stem>.store/.trace/` after the store dump, so clearing
//! the dump directory cannot erase the turn files from the run just finished.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use anyhow::{Context as _, Result, bail};
use promptforge_core::debug::{DebugCapture, DebugEvent};
use promptforge_core::store::StoreRef;

/// Copies every file in `store` under the prompt's dump directory,
/// announcing each dumped path as one line on `status`.
///
/// The dump directory is `prompt_path` with its extension replaced by
/// `store` (for `briefer.md` that is `briefer.store`, a sibling of the
/// prompt). An existing dump directory is removed first, and an empty store
/// leaves no directory behind. A store path that is absolute, traverses with
/// `..`, or carries characters unsafe on the local filesystem is reported on
/// `status` and skipped. Status writes are advisory and never fail the dump.
///
/// # Errors
///
/// Returns an error when the dump path exists but is not a directory, or
/// when removing, creating, or writing under the dump directory fails.
pub(crate) fn dump_store(
    store: &StoreRef,
    prompt_path: &Path,
    status: &mut dyn Write,
) -> Result<()> {
    let directory = dump_directory(prompt_path);
    if directory.symlink_metadata().is_ok() {
        if !directory.is_dir() {
            bail!(
                "{} exists and is not a directory, refusing to replace it",
                directory.display()
            );
        }
        std::fs::remove_dir_all(&directory)
            .with_context(|| format!("clear the previous store dump {}", directory.display()))?;
    }

    let paths = store
        .glob("**")
        .context("enumerate the run's store files")?;
    if paths.is_empty() {
        return Ok(());
    }

    std::fs::create_dir_all(&directory)
        .with_context(|| format!("create the store dump directory {}", directory.display()))?;
    for path in paths {
        let Some(relative) = safe_relative_path(&path) else {
            // Status lines are advisory; a closed pipe must not fail the dump.
            let _ignored = writeln!(status, "store dump skipped unsafe path {path:?}");
            continue;
        };
        let target = directory.join(relative);
        // A colliding pair of store paths (a file where another entry needs a
        // directory, or names differing only by case on a case-insensitive
        // filesystem) skips this entry like an unsafe path does, so one
        // collision cannot abort the rest of the dump.
        if let Some(parent) = target.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            let _ignored = writeln!(
                status,
                "store dump skipped {path:?}: create {}: {error}",
                parent.display()
            );
            continue;
        }
        let contents = store
            .read(&path)
            .with_context(|| format!("read store file {path:?}"))?;
        if let Err(error) = std::fs::write(&target, contents) {
            let _ignored = writeln!(
                status,
                "store dump skipped {path:?}: write {}: {error}",
                target.display()
            );
            continue;
        }
        let _ignored = writeln!(status, "store dump wrote {}", target.display());
    }
    Ok(())
}

/// Returns the dump directory for `prompt_path`: the prompt's own path with
/// its extension replaced by `store`, a sibling of the prompt file.
pub(crate) fn dump_directory(prompt_path: &Path) -> PathBuf {
    prompt_path.with_extension("store")
}

/// Buffers raw model-turn payloads during a run and writes them under
/// `<prompt-stem>.store/.trace/` when [`TraceCapture::flush`] runs.
///
/// Events are held in memory until flush so [`dump_store`]'s directory clear
/// cannot delete turn files from the run that just finished. Flush is a no-op
/// when no turns were captured.
pub(crate) struct TraceCapture {
    events: Mutex<Vec<(u32, DebugEvent)>>,
    dump_root: PathBuf,
}

impl TraceCapture {
    /// Captures turns for the dump directory beside `prompt_path`.
    pub(crate) fn new(prompt_path: &Path) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            dump_root: dump_directory(prompt_path),
        }
    }

    /// Writes buffered turn files under `.trace/` and announces each path on
    /// `status`. Status writes are advisory and never fail the flush.
    pub(crate) fn flush(&self, status: &mut dyn Write) {
        let events = self
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if events.is_empty() {
            return;
        }
        let trace_dir = self.dump_root.join(".trace");
        if let Err(error) = std::fs::create_dir_all(&trace_dir) {
            let _ignored = writeln!(
                status,
                "trace dump failed: create {}: {error}",
                trace_dir.display()
            );
            return;
        }
        for (turn_index, event) in events {
            let (name, body) = match &event {
                DebugEvent::Request { body } => (format!("turn-{turn_index}-request.json"), body),
                DebugEvent::Response { body, .. } => {
                    (format!("turn-{turn_index}-response.json"), body)
                }
                _ => continue,
            };
            let target = trace_dir.join(name);
            let rendered = match serde_json::to_string_pretty(body) {
                Ok(text) => text,
                Err(error) => {
                    let _ignored = writeln!(
                        status,
                        "trace dump skipped {}: serialize: {error}",
                        target.display()
                    );
                    continue;
                }
            };
            if let Err(error) = std::fs::write(&target, rendered) {
                let _ignored = writeln!(
                    status,
                    "trace dump skipped {}: write: {error}",
                    target.display()
                );
                continue;
            }
            let _ignored = writeln!(status, "trace dump wrote {}", target.display());
        }
    }
}

impl DebugCapture for TraceCapture {
    fn on_event(&self, _execution: &str, _section: &str, turn_index: u32, event: DebugEvent) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((turn_index, event));
    }
}

/// Maps one logical store path to a relative filesystem path, or `None` when
/// the path cannot be written safely inside the dump directory.
///
/// StoreRef paths use `/` separators. A path is rejected when it is empty or
/// absolute, when any component is empty, `.`, or `..`, when a component
/// carries a separator or a character Windows reserves (`\ : * ? " < > |`, a
/// control character, or a trailing dot or space), or when a component's stem
/// is a reserved device name such as `NUL` or `COM1`.
fn safe_relative_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let mut relative = PathBuf::new();
    for component in path.split('/') {
        if !component_is_safe(component) {
            return None;
        }
        relative.push(component);
    }
    Some(relative)
}

/// Reports whether one path component is safe as a file or directory name.
fn component_is_safe(component: &str) -> bool {
    if component.is_empty() || component == "." || component == ".." {
        return false;
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return false;
    }
    if component
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return false;
    }
    let stem = component.split('.').next().unwrap_or(component);
    !is_reserved_device_name(stem)
}

/// Reports whether `stem` is a Windows reserved device name, which the
/// filesystem would silently redirect rather than store.
fn is_reserved_device_name(stem: &str) -> bool {
    /// One device digit: ASCII `0`-`9` or the legacy superscript digits
    /// `¹ ² ³`, which Windows also treats as device numbers.
    fn is_device_digit(c: char) -> bool {
        c.is_ascii_digit() || matches!(c, '\u{00B9}' | '\u{00B2}' | '\u{00B3}')
    }
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || matches!(upper.strip_prefix("COM"), Some(digit) if digit.chars().count() == 1 && digit.chars().all(is_device_digit))
        || matches!(upper.strip_prefix("LPT"), Some(digit) if digit.chars().count() == 1 && digit.chars().all(is_device_digit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump_to(directory: &Path, store: &StoreRef) -> (PathBuf, String) {
        let prompt = directory.join("fixture.md");
        let mut status = Vec::new();
        dump_store(store, &prompt, &mut status).expect("the dump must succeed");
        (
            prompt.with_extension("store"),
            String::from_utf8(status).expect("status output must be UTF-8"),
        )
    }

    #[test]
    fn dumps_every_store_file_to_its_relative_path_with_raw_contents() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        let store = StoreRef::memory();
        store
            .write("evidence.md", "line one\nline two\n")
            .expect("write");
        store
            .write("notes/deep/found.txt", "nested")
            .expect("write");

        let (dump_dir, status) = dump_to(directory.path(), &store);

        assert_eq!(
            std::fs::read_to_string(dump_dir.join("evidence.md")).expect("read dumped file"),
            "line one\nline two\n",
            "raw contents must round-trip, including the trailing newline"
        );
        assert_eq!(
            std::fs::read_to_string(dump_dir.join("notes").join("deep").join("found.txt"))
                .expect("read nested dumped file"),
            "nested"
        );
        for expected in ["evidence.md", "found.txt"] {
            assert!(
                status
                    .lines()
                    .any(|line| line.starts_with("store dump wrote ") && line.contains(expected)),
                "each dumped path must be announced, missing {expected}: {status}"
            );
        }
    }

    #[test]
    fn unsafe_store_paths_are_skipped_with_a_status_report() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        let store = StoreRef::memory();
        let unsafe_paths = [
            "/absolute.txt",
            "../escape.txt",
            "a/../traversal.txt",
            "C:/drive.txt",
            "back\\slash.txt",
            "nul.txt",
            "trailing. /x",
        ];
        for path in unsafe_paths {
            store.write(path, "must not land on disk").expect("write");
        }
        store.write("safe.txt", "kept").expect("write");

        let (dump_dir, status) = dump_to(directory.path(), &store);

        let entries: Vec<_> = walk(&dump_dir);
        assert_eq!(
            entries,
            vec![dump_dir.join("safe.txt")],
            "only the safe path may be written: {entries:?}"
        );
        for path in unsafe_paths {
            assert!(
                status.contains(&format!("store dump skipped unsafe path {path:?}")),
                "the skip of {path:?} must be reported: {status}"
            );
        }
    }

    #[test]
    fn a_second_dump_clears_the_previous_runs_files() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        let first = StoreRef::memory();
        first.write("stale.txt", "from run one").expect("write");
        let (dump_dir, _) = dump_to(directory.path(), &first);
        assert!(dump_dir.join("stale.txt").is_file());

        let second = StoreRef::memory();
        second.write("fresh.txt", "from run two").expect("write");
        dump_to(directory.path(), &second);

        assert!(
            !dump_dir.join("stale.txt").exists(),
            "the previous run's files must not linger"
        );
        assert_eq!(
            std::fs::read_to_string(dump_dir.join("fresh.txt")).expect("read fresh dump"),
            "from run two"
        );
    }

    #[test]
    fn an_empty_store_leaves_no_dump_directory() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        let populated = StoreRef::memory();
        populated.write("residue.txt", "old").expect("write");
        let (dump_dir, _) = dump_to(directory.path(), &populated);
        assert!(dump_dir.is_dir());

        let (dump_dir, status) = dump_to(directory.path(), &StoreRef::memory());

        assert!(
            !dump_dir.exists(),
            "an empty run must leave no dump directory"
        );
        assert_eq!(status, "", "an empty dump announces nothing");
    }

    #[test]
    fn a_dump_path_occupied_by_a_file_is_refused() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        let prompt = directory.path().join("fixture.md");
        std::fs::write(prompt.with_extension("store"), "not a directory")
            .expect("occupy the dump path");
        let store = StoreRef::memory();
        store.write("a.txt", "x").expect("write");

        let error = dump_store(&store, &prompt, &mut Vec::new())
            .expect_err("a non-directory dump path must be refused");

        assert!(
            format!("{error:#}").contains("not a directory"),
            "unexpected refusal: {error:#}"
        );
    }

    #[test]
    fn safe_relative_paths_accept_ordinary_names_and_reject_escapes() {
        for accepted in ["a.txt", "a/b/c.md", "with space.txt", "dot.in.name"] {
            assert!(
                safe_relative_path(accepted).is_some(),
                "{accepted:?} must be accepted"
            );
        }
        for rejected in [
            "",
            "/a.txt",
            "a//b.txt",
            "..",
            "a/..",
            "../a",
            "a/./b",
            "a\\b",
            "C:/a",
            "a:b",
            "que?.txt",
            "star*.txt",
            "pipe|.txt",
            "quote\".txt",
            "angle<.txt",
            "angle>.txt",
            "ctrl\u{7}.txt",
            "trailing.",
            "trailing ",
            "CON",
            "nul.txt",
            "Com1.log",
            "lpt9",
            "com\u{00B9}",
            "LPT\u{00B3}.txt",
        ] {
            assert!(
                safe_relative_path(rejected).is_none(),
                "{rejected:?} must be rejected"
            );
        }
        for device_lookalike in ["CONSOLE", "COM10", "COMX", "LPT", "nulled.txt"] {
            assert!(
                safe_relative_path(device_lookalike).is_some(),
                "{device_lookalike:?} is not a reserved device name"
            );
        }
    }

    #[test]
    fn a_file_directory_collision_skips_the_loser_and_dumps_the_rest() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        let store = StoreRef::memory();
        store.write("a", "plain file").expect("write");
        store
            .write("a/b.txt", "wants a as a directory")
            .expect("write");
        store.write("c.txt", "unaffected").expect("write");

        let (dump_dir, status) = dump_to(directory.path(), &store);

        assert_eq!(
            std::fs::read_to_string(dump_dir.join("c.txt")).expect("read c.txt"),
            "unaffected",
            "a collision elsewhere must not abort the rest of the dump"
        );
        assert!(
            status.contains("store dump skipped"),
            "the colliding entry must be reported: {status}"
        );
    }

    #[test]
    fn trace_capture_flush_writes_turn_files_under_dot_trace() {
        use promptforge_core::debug::DebugEvent;
        use serde_json::json;

        let directory = tempfile::tempdir().expect("create trace fixture directory");
        let prompt = directory.path().join("fixture.md");
        let capture = TraceCapture::new(&prompt);
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::Request {
                body: json!({ "model": "test", "messages": [] }),
            },
        );
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::Response {
                body: json!({
                    "choices": [{ "message": { "role": "assistant", "content": "hi" } }]
                }),
                finish_reason: Some("stop".into()),
                reasoning_content: None,
            },
        );

        let mut status = Vec::new();
        capture.flush(&mut status);
        let status = String::from_utf8(status).expect("status must be UTF-8");
        let trace_dir = dump_directory(&prompt).join(".trace");
        let request =
            std::fs::read_to_string(trace_dir.join("turn-1-request.json")).expect("request dump");
        let response =
            std::fs::read_to_string(trace_dir.join("turn-1-response.json")).expect("response dump");
        assert!(request.contains("\"model\": \"test\""));
        assert!(response.contains("\"content\": \"hi\""));
        assert!(
            status.contains("trace dump wrote") && status.contains("turn-1-request.json"),
            "each dumped path must be announced: {status}"
        );
    }

    #[test]
    fn dump_store_then_flush_keeps_trace_files() {
        use promptforge_core::debug::DebugEvent;
        use serde_json::json;

        let directory = tempfile::tempdir().expect("create dump-then-flush fixture directory");
        let prompt = directory.path().join("fixture.md");
        let store = StoreRef::memory();
        store.write("evidence.md", "body").expect("write");
        let capture = TraceCapture::new(&prompt);
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::Request {
                body: json!({ "model": "test" }),
            },
        );
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::Response {
                body: json!({ "choices": [] }),
                finish_reason: Some("stop".into()),
                reasoning_content: None,
            },
        );

        let mut status = Vec::new();
        dump_store(&store, &prompt, &mut status).expect("dump must succeed");
        capture.flush(&mut status);

        let dump_dir = dump_directory(&prompt);
        assert_eq!(
            std::fs::read_to_string(dump_dir.join("evidence.md")).expect("evidence"),
            "body"
        );
        assert!(
            dump_dir
                .join(".trace")
                .join("turn-1-request.json")
                .is_file(),
            "flush after dump_store must leave turn files under .trace"
        );
        assert!(
            dump_dir
                .join(".trace")
                .join("turn-1-response.json")
                .is_file()
        );
    }

    #[test]
    fn trace_capture_flush_is_noop_without_events() {
        let directory = tempfile::tempdir().expect("create empty-trace fixture directory");
        let prompt = directory.path().join("fixture.md");
        let capture = TraceCapture::new(&prompt);
        let mut status = Vec::new();
        capture.flush(&mut status);
        assert!(status.is_empty());
        assert!(
            !dump_directory(&prompt).join(".trace").exists(),
            "no turns must leave no .trace directory"
        );
    }

    /// Collects every file under `root`, sorted, for exact-contents asserts.
    fn walk(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("read dump directory") {
                let path = entry.expect("read dump entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }
}
