//! Live store dumps and turn traces for the interactive prompt runner.
//!
//! Each run clears `<prompt-stem>.store/` once at start. During the run,
//! [`MirrorStore`] mirrors every store mutation to that directory and
//! [`TraceCapture`] writes `.trace/turn-N-*.json` as each model turn arrives.
//! After the run, [`dump_store`] reconciles disk to the in-memory store without
//! wiping `.trace/`. StoreRef paths that cannot map to a safe relative
//! filesystem path are skipped; nothing is ever written outside the dump
//! directory.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use promptforge_core::debug::{DebugCapture, DebugEvent};
use promptforge_core::store::{MemStore, Store, StoreError, StoreRef};

/// Reconciles every file in `store` under the prompt's dump directory.
///
/// The dump directory is `prompt_path` with its extension replaced by `store`
/// (for `briefer.md` that is `briefer.store`, a sibling of the prompt). Unlike
/// a full wipe, this pass overwrites store files, deletes dump files that are
/// not in the store and not under `.trace/`, and leaves turn traces alone. An
/// empty store with no remaining dump contents (after removing orphans) leaves
/// no dump directory. A store path that is absolute, traverses with `..`, or
/// carries characters unsafe on the local filesystem is reported on `status`
/// and skipped. Status writes are advisory and never fail the dump.
///
/// # Errors
///
/// Returns an error when the dump path exists but is not a directory, or when
/// creating or writing under the dump directory fails hard.
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
    }

    let paths = store
        .glob("**")
        .context("enumerate the run's store files")?;
    let mut kept: HashSet<PathBuf> = HashSet::new();

    if !paths.is_empty() {
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("create the store dump directory {}", directory.display()))?;
    }

    for path in &paths {
        match mirror_store_file(&directory, store, path, status) {
            MirrorOutcome::Wrote(relative) => {
                kept.insert(relative);
            }
            MirrorOutcome::Skipped => {}
        }
    }

    if directory.is_dir() {
        remove_orphaned_store_files(&directory, &kept, status)?;
        prune_empty_dirs(&directory)?;
        // Empty store and nothing left on disk (including no `.trace/`) →
        // remove the dump root so authors see a clean sibling tree.
        if paths.is_empty() && dir_is_effectively_empty(&directory) {
            std::fs::remove_dir_all(&directory).with_context(|| {
                format!("remove empty store dump {}", directory.display())
            })?;
        }
    }

    Ok(())
}

/// Returns the dump directory for `prompt_path`: the prompt's own path with
/// its extension replaced by `store`, a sibling of the prompt file.
pub(crate) fn dump_directory(prompt_path: &Path) -> PathBuf {
    prompt_path.with_extension("store")
}

/// In-memory store that mirrors mutating operations into `dump_root`.
///
/// Reads stay in memory. Unsafe store paths skip the disk side and print a
/// skip line on stderr. `.trace/` is never modified here.
pub(crate) struct MirrorStore {
    inner: MemStore,
    dump_root: PathBuf,
}

impl MirrorStore {
    /// Mirrors into `dump_root` (typically [`dump_directory`]).
    pub(crate) fn new(dump_root: PathBuf) -> Self {
        Self {
            inner: MemStore::new(),
            dump_root,
        }
    }

    fn mirror_write(&mut self, path: &str) {
        let mut sink = std::io::stderr();
        let _ = mirror_store_contents(&self.dump_root, path, &self.inner, &mut sink);
    }

    fn mirror_delete(&mut self, path: &str) {
        let Some(relative) = safe_relative_path(path) else {
            eprintln!("store dump skipped unsafe path {path:?}");
            return;
        };
        let target = self.dump_root.join(&relative);
        match std::fs::remove_file(&target) {
            Ok(()) => eprintln!("store dump removed {}", target.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!(
                    "store dump skipped {path:?}: remove {}: {error}",
                    target.display()
                );
            }
        }
    }
}

impl Store for MirrorStore {
    fn write(&mut self, path: &str, contents: &str) -> Result<(), StoreError> {
        self.inner.write(path, contents)?;
        self.mirror_write(path);
        Ok(())
    }

    fn append(&mut self, path: &str, contents: &str) -> Result<(), StoreError> {
        self.inner.append(path, contents)?;
        self.mirror_write(path);
        Ok(())
    }

    fn read_lines(&self, path: &str) -> Result<String, StoreError> {
        self.inner.read_lines(path)
    }

    fn read(&self, path: &str) -> Result<String, StoreError> {
        self.inner.read(path)
    }

    fn str_replace(&mut self, path: &str, old: &str, new: &str) -> Result<(), StoreError> {
        self.inner.str_replace(path, old, new)?;
        self.mirror_write(path);
        Ok(())
    }

    fn delete(&mut self, path: &str) -> Result<(), StoreError> {
        self.inner.delete(path)?;
        self.mirror_delete(path);
        Ok(())
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, StoreError> {
        self.inner.glob(pattern)
    }
}

/// Writes raw model-turn payloads under `<prompt-stem>.store/.trace/` as each
/// event arrives.
pub(crate) struct TraceCapture {
    dump_root: PathBuf,
}

impl TraceCapture {
    /// Captures turns for the dump directory beside `prompt_path`.
    pub(crate) fn new(prompt_path: &Path) -> Self {
        Self {
            dump_root: dump_directory(prompt_path),
        }
    }
}

impl DebugCapture for TraceCapture {
    fn on_event(&self, _execution: &str, _section: &str, turn_index: u32, event: DebugEvent) {
        let (name, body) = match &event {
            DebugEvent::Request { body } => (format!("turn-{turn_index}-request.json"), body),
            DebugEvent::Response { body, .. } => {
                (format!("turn-{turn_index}-response.json"), body)
            }
            _ => return,
        };
        let trace_dir = self.dump_root.join(".trace");
        if let Err(error) = std::fs::create_dir_all(&trace_dir) {
            eprintln!(
                "trace dump failed: create {}: {error}",
                trace_dir.display()
            );
            return;
        }
        let target = trace_dir.join(name);
        let rendered = match serde_json::to_string_pretty(body) {
            Ok(text) => text,
            Err(error) => {
                eprintln!(
                    "trace dump skipped {}: serialize: {error}",
                    target.display()
                );
                return;
            }
        };
        if let Err(error) = std::fs::write(&target, rendered) {
            eprintln!("trace dump skipped {}: write: {error}", target.display());
            return;
        }
        eprintln!("trace dump wrote {}", target.display());
    }
}

enum MirrorOutcome {
    Wrote(PathBuf),
    Skipped,
}

/// Writes one store path into `directory`, announcing on `status`.
fn mirror_store_file(
    directory: &Path,
    store: &StoreRef,
    path: &str,
    status: &mut dyn Write,
) -> MirrorOutcome {
    let Some(relative) = safe_relative_path(path) else {
        let _ignored = writeln!(status, "store dump skipped unsafe path {path:?}");
        return MirrorOutcome::Skipped;
    };
    let target = directory.join(&relative);
    if let Some(parent) = target.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        let _ignored = writeln!(
            status,
            "store dump skipped {path:?}: create {}: {error}",
            parent.display()
        );
        return MirrorOutcome::Skipped;
    }
    let contents = match store.read(path) {
        Ok(text) => text,
        Err(error) => {
            let _ignored = writeln!(status, "store dump skipped {path:?}: read: {error}");
            return MirrorOutcome::Skipped;
        }
    };
    if let Err(error) = std::fs::write(&target, contents) {
        let _ignored = writeln!(
            status,
            "store dump skipped {path:?}: write {}: {error}",
            target.display()
        );
        return MirrorOutcome::Skipped;
    }
    let _ignored = writeln!(status, "store dump wrote {}", target.display());
    MirrorOutcome::Wrote(relative)
}

/// Mirrors from a [`MemStore`] (used by [`MirrorStore`]).
fn mirror_store_contents(
    directory: &Path,
    path: &str,
    store: &MemStore,
    status: &mut dyn Write,
) -> MirrorOutcome {
    let Some(relative) = safe_relative_path(path) else {
        let _ignored = writeln!(status, "store dump skipped unsafe path {path:?}");
        return MirrorOutcome::Skipped;
    };
    let target = directory.join(&relative);
    if let Some(parent) = target.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        let _ignored = writeln!(
            status,
            "store dump skipped {path:?}: create {}: {error}",
            parent.display()
        );
        return MirrorOutcome::Skipped;
    }
    let contents = match store.read(path) {
        Ok(text) => text,
        Err(error) => {
            let _ignored = writeln!(status, "store dump skipped {path:?}: read: {error}");
            return MirrorOutcome::Skipped;
        }
    };
    if let Err(error) = std::fs::write(&target, &contents) {
        let _ignored = writeln!(
            status,
            "store dump skipped {path:?}: write {}: {error}",
            target.display()
        );
        return MirrorOutcome::Skipped;
    }
    let _ignored = writeln!(status, "store dump wrote {}", target.display());
    MirrorOutcome::Wrote(relative)
}

/// Deletes dump files that are not under `.trace/` and not in `kept`.
fn remove_orphaned_store_files(
    directory: &Path,
    kept: &HashSet<PathBuf>,
    status: &mut dyn Write,
) -> Result<()> {
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", current.display()));
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("read entry under {}", current.display()))?;
            let path = entry.path();
            let file_name = entry.file_name();
            if current == directory && file_name == ".trace" {
                continue;
            }
            let meta = entry
                .metadata()
                .with_context(|| format!("stat {}", path.display()))?;
            if meta.is_dir() {
                pending.push(path);
                continue;
            }
            let Ok(relative) = path.strip_prefix(directory) else {
                continue;
            };
            if kept.contains(relative) {
                continue;
            }
            if let Err(error) = std::fs::remove_file(&path) {
                let _ignored = writeln!(
                    status,
                    "store dump skipped orphan {}: remove: {error}",
                    path.display()
                );
                continue;
            }
            let _ignored = writeln!(status, "store dump removed {}", path.display());
        }
    }
    Ok(())
}

/// Removes empty directories under `directory`, deepest first, never deleting
/// `directory` itself or `.trace/`.
fn prune_empty_dirs(directory: &Path) -> Result<()> {
    let mut dirs = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", current.display()));
            }
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("read entry under {}", current.display()))?;
            let path = entry.path();
            if current == directory && entry.file_name() == ".trace" {
                continue;
            }
            if entry
                .metadata()
                .with_context(|| format!("stat {}", path.display()))?
                .is_dir()
            {
                pending.push(path.clone());
                dirs.push(path);
            }
        }
    }
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in dirs {
        let _ignored = std::fs::remove_dir(&path);
    }
    Ok(())
}

/// True when `directory` has no entries at all.
fn dir_is_effectively_empty(directory: &Path) -> bool {
    match std::fs::read_dir(directory) {
        Ok(mut entries) => entries.next().is_none(),
        Err(_) => true,
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
    fn a_second_dump_removes_orphaned_store_files() {
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

        let (dump_dir, _) = dump_to(directory.path(), &StoreRef::memory());

        assert!(
            !dump_dir.exists(),
            "an empty run must leave no dump directory"
        );
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
    fn trace_capture_writes_turn_files_on_event() {
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

        let trace_dir = dump_directory(&prompt).join(".trace");
        let request =
            std::fs::read_to_string(trace_dir.join("turn-1-request.json")).expect("request dump");
        let response =
            std::fs::read_to_string(trace_dir.join("turn-1-response.json")).expect("response dump");
        assert!(request.contains("\"model\": \"test\""));
        assert!(response.contains("\"content\": \"hi\""));
    }

    #[test]
    fn dump_store_reconcile_keeps_existing_trace_files() {
        use promptforge_core::debug::DebugEvent;
        use serde_json::json;

        let directory = tempfile::tempdir().expect("create dump-then-trace fixture directory");
        let prompt = directory.path().join("fixture.md");
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

        let store = StoreRef::memory();
        store.write("evidence.md", "body").expect("write");
        let mut status = Vec::new();
        dump_store(&store, &prompt, &mut status).expect("dump must succeed");

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
            "reconcile must leave turn files under .trace"
        );
        assert!(
            dump_dir
                .join(".trace")
                .join("turn-1-response.json")
                .is_file()
        );
    }

    #[test]
    fn mirror_store_writes_and_deletes_appear_on_disk_immediately() {
        let directory = tempfile::tempdir().expect("create mirror fixture directory");
        let dump_root = directory.path().join("fixture.store");
        let store = StoreRef::new(Box::new(MirrorStore::new(dump_root.clone())));

        store.write("evidence.md", "live").expect("write");
        assert_eq!(
            std::fs::read_to_string(dump_root.join("evidence.md")).expect("read mirror"),
            "live"
        );

        store.delete("evidence.md").expect("delete");
        assert!(
            !dump_root.join("evidence.md").exists(),
            "delete must remove the mirrored file"
        );
    }

    #[test]
    fn empty_reconcile_preserves_trace_only_dump() {
        use promptforge_core::debug::DebugEvent;
        use serde_json::json;

        let directory = tempfile::tempdir().expect("create trace-only fixture directory");
        let prompt = directory.path().join("fixture.md");
        let capture = TraceCapture::new(&prompt);
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::Request {
                body: json!({ "model": "test" }),
            },
        );

        let mut status = Vec::new();
        dump_store(&StoreRef::memory(), &prompt, &mut status).expect("reconcile empty store");

        let dump_dir = dump_directory(&prompt);
        assert!(
            dump_dir.join(".trace").join("turn-1-request.json").is_file(),
            "empty store must not wipe .trace"
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
