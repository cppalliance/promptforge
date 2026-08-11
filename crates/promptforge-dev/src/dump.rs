//! Store dumps and turn traces for the interactive prompt runner.
//!
//! Each run clears `<prompt-stem>.store/` once at start. The run itself keeps
//! its store in memory (no filesystem writes on the async execution path); when
//! raw capture is authorized, [`TraceCapture`] queues each model turn to a
//! worker thread. After the run, [`dump_store`] reconciles the in-memory store
//! to disk (called off the async runtime by the caller) without wiping
//! `.trace/`.
//!
//! Every write goes through [`fs_safe`], which creates owner-only files and
//! directories, refuses to follow a symlink or reparse point at any component,
//! and writes atomically. StoreRef paths that cannot map to a safe relative
//! filesystem path are skipped and reported; a create/write failure is surfaced
//! as an error so an incomplete dump is never reported as success. Nothing is
//! ever written outside the dump directory.

mod fs_safe;
mod paths;
mod trace_capture;

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use promptforge_core::store::StoreRef;

use self::paths::safe_relative_path;

pub(crate) use self::trace_capture::{SensitiveCapture, TraceCapture};

/// Reconciles every file in `store` under the prompt's dump directory.
///
/// The dump directory is `prompt_path` with its extension replaced by `store`
/// (for `briefer.md` that is `briefer.store`, a sibling of the prompt). Unlike
/// a full wipe, this pass overwrites store files, deletes dump files that are
/// not in the store and not under `.trace/`, and leaves turn traces alone. An
/// empty store with no remaining dump contents (after removing orphans) leaves
/// no dump directory. A store path that is absolute, traverses with `..`, or
/// carries characters unsafe on the local filesystem is reported on `status`
/// and skipped (not an error).
///
/// This performs blocking filesystem I/O and is meant to be called off the
/// async runtime (see `crate::run::reconcile_dump`).
///
/// # Errors
///
/// Returns an error when the dump path exists but is a symlink/reparse point or
/// is not a directory, when reconciliation fails hard, or when any store file
/// could not be created or written (so a partial dump is never silently
/// reported as success).
pub(crate) fn dump_store(
    store: &StoreRef,
    prompt_path: &Path,
    status: &mut dyn Write,
) -> Result<()> {
    let directory = dump_directory(prompt_path);
    fs_safe::reject_reparse_ancestors(&directory)
        .with_context(|| format!("inspect the store dump path {}", directory.display()))?;
    if directory.symlink_metadata().is_ok() && !directory.is_dir() {
        bail!(
            "{} exists and is not a directory, refusing to replace it",
            directory.display()
        );
    }

    let paths = store
        .glob("**")
        .context("enumerate the run's store files")?;
    let mut kept: HashSet<PathBuf> = HashSet::new();
    let mut failures: Vec<anyhow::Error> = Vec::new();

    if !paths.is_empty() {
        fs_safe::create_dir_all_secure(&directory)
            .with_context(|| format!("create the store dump directory {}", directory.display()))?;
    }

    for path in &paths {
        let contents = match store.read(path) {
            Ok(text) => text,
            Err(error) => {
                let _ignored = writeln!(status, "store dump failed {path:?}: read: {error}");
                failures.push(anyhow!("read store path {path:?}: {error}"));
                continue;
            }
        };
        match write_store_entry(&directory, path, &contents, status) {
            Ok(Written::Kept(relative)) => {
                kept.insert(relative);
            }
            Ok(Written::SkippedUnsafe) => {}
            Err(error) => {
                let _ignored = writeln!(status, "store dump failed {path:?}: {error:#}");
                failures.push(error);
            }
        }
    }

    if directory.is_dir() {
        remove_orphaned_store_files(&directory, &kept, status)?;
        prune_empty_dirs(&directory)?;
        // Empty store and nothing left on disk (including no `.trace/`) →
        // remove the dump root so authors see a clean sibling tree.
        if paths.is_empty() && dir_is_effectively_empty(&directory) {
            std::fs::remove_dir_all(&directory)
                .with_context(|| format!("remove empty store dump {}", directory.display()))?;
        }
    }

    // A per-file create/write failure must not be swallowed: surface it so a
    // successful run can never report success behind an incomplete artifact.
    let failure_count = failures.len();
    if let Some(first) = failures.into_iter().next() {
        return Err(first.context(format!(
            "store dump could not write {failure_count} file(s); the dumped artifact is incomplete"
        )));
    }
    Ok(())
}

/// Returns the dump directory for `prompt_path`: the prompt's own path with
/// its extension replaced by `store`, a sibling of the prompt file.
pub(crate) fn dump_directory(prompt_path: &Path) -> PathBuf {
    prompt_path.with_extension("store")
}

/// The disposition of one store entry during a dump.
enum Written {
    /// The entry was written; its relative path is retained for orphan pruning.
    Kept(PathBuf),
    /// The logical path was unsafe to place on this filesystem; reported and
    /// skipped, which is not a failure.
    SkippedUnsafe,
}

/// Writes one already-read store entry into `directory`, announcing on
/// `status`.
///
/// Maps the logical path to a safe relative path, creates owner-only parents,
/// and writes the contents atomically through [`fs_safe`].
///
/// # Errors
/// Returns an error when creating the parent directory or writing the file
/// fails. An unsafe logical path is reported on `status` and returned as
/// [`Written::SkippedUnsafe`], which callers treat as non-fatal.
fn write_store_entry(
    directory: &Path,
    path: &str,
    contents: &str,
    status: &mut dyn Write,
) -> Result<Written> {
    let Some(relative) = safe_relative_path(path) else {
        let _ignored = writeln!(status, "store dump skipped unsafe path {path:?}");
        return Ok(Written::SkippedUnsafe);
    };
    let target = directory.join(&relative);
    if let Some(parent) = target.parent() {
        fs_safe::create_dir_all_secure(parent)
            .with_context(|| format!("create {} for {path:?}", parent.display()))?;
    }
    fs_safe::write_atomic_secure(&target, contents.as_bytes())
        .with_context(|| format!("write {} for {path:?}", target.display()))?;
    let _ignored = writeln!(status, "store dump wrote {}", target.display());
    Ok(Written::Kept(relative))
}

/// Deletes dump files that are not under `.trace/` and not in `kept`.
///
/// Traversal is no-follow: a symlink or reparse point is left untouched and
/// never recursed into, so reconciliation can neither traverse nor delete
/// outside the dump root.
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
            let file_type = entry
                .file_type()
                .with_context(|| format!("stat {}", path.display()))?;
            // Never follow a link during reconciliation.
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
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
/// `directory` itself or `.trace/`. Traversal is no-follow.
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
            let file_type = entry
                .file_type()
                .with_context(|| format!("stat {}", path.display()))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
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
        for rejected in [
            "/absolute.txt",
            "../escape.txt",
            "a/../traversal.txt",
            "back\\slash.txt",
            "nul.txt",
            "trailing. /x",
        ] {
            assert!(
                store.write(rejected, "must not land on disk").is_err(),
                "the store must reject the unsafe path {rejected:?} at the boundary"
            );
        }
        let dump_skipped = ["C:/drive.txt", "star*.txt", "q?.txt", "pipe|x.txt"];
        for path in dump_skipped {
            store
                .write(path, "must not land on disk")
                .expect("the store accepts the path; the dump is what refuses it");
        }
        store.write("safe.txt", "kept").expect("write");

        let (dump_dir, status) = dump_to(directory.path(), &store);

        let entries: Vec<_> = walk(&dump_dir);
        assert_eq!(
            entries,
            vec![dump_dir.join("safe.txt")],
            "only the safe path may be written: {entries:?}"
        );
        for path in dump_skipped {
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
    fn a_file_directory_collision_surfaces_an_error_but_dumps_the_rest() {
        let directory = tempfile::tempdir().expect("create dump fixture directory");
        let store = StoreRef::memory();
        store.write("a", "plain file").expect("write");
        store
            .write("a/b.txt", "wants a as a directory")
            .expect("write");
        store.write("c.txt", "unaffected").expect("write");

        let prompt = directory.path().join("fixture.md");
        let mut status = Vec::new();
        let error = dump_store(&store, &prompt, &mut status)
            .expect_err("a per-file create/write failure must surface, not be swallowed");
        let status = String::from_utf8(status).expect("status output must be UTF-8");
        let dump_dir = dump_directory(&prompt);

        assert_eq!(
            std::fs::read_to_string(dump_dir.join("c.txt")).expect("read c.txt"),
            "unaffected",
            "a collision elsewhere must not abort the rest of the dump"
        );
        assert!(
            format!("{error:#}").contains("incomplete"),
            "the surfaced error must flag an incomplete dump: {error:#}"
        );
        assert!(
            status.contains("store dump failed"),
            "the colliding entry must be reported: {status}"
        );
    }

    #[test]
    fn dump_store_reconcile_keeps_existing_trace_files() {
        use promptforge_core::debug::DebugCapture;
        use promptforge_core::debug::DebugEvent;
        use serde_json::json;

        let directory = tempfile::tempdir().expect("create dump-then-trace fixture directory");
        let prompt = directory.path().join("fixture.md");
        let capture = TraceCapture::new(&prompt, SensitiveCapture::authorized());
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::request(json!({ "model": "test" })),
        );
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::response(json!({ "choices": [] }), Some("stop".into()), None),
        );
        capture.finish();

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
    fn empty_reconcile_preserves_trace_only_dump() {
        use promptforge_core::debug::DebugCapture;
        use promptforge_core::debug::DebugEvent;
        use serde_json::json;

        let directory = tempfile::tempdir().expect("create trace-only fixture directory");
        let prompt = directory.path().join("fixture.md");
        let capture = TraceCapture::new(&prompt, SensitiveCapture::authorized());
        capture.on_event(
            "dev-1",
            "Only",
            1,
            DebugEvent::request(json!({ "model": "test" })),
        );
        capture.finish();

        let mut status = Vec::new();
        dump_store(&StoreRef::memory(), &prompt, &mut status).expect("reconcile empty store");

        let dump_dir = dump_directory(&prompt);
        assert!(
            dump_dir
                .join(".trace")
                .join("turn-1-request.json")
                .is_file(),
            "empty store must not wipe .trace"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_never_follows_a_symlinked_orphan() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("create fixture directory");
        let outside = directory.path().join("outside.txt");
        std::fs::write(&outside, "must survive").expect("write outside file");

        let store = StoreRef::memory();
        store.write("keep.txt", "kept").expect("write");
        let (dump_dir, _) = dump_to(directory.path(), &store);
        // Plant a symlink orphan inside the dump directory pointing outside.
        symlink(&outside, dump_dir.join("link.txt")).expect("plant symlink");

        // A second reconcile must remove real orphans but never delete through
        // the planted link.
        let second = StoreRef::memory();
        second.write("keep.txt", "kept").expect("write");
        dump_to(directory.path(), &second);

        assert!(
            outside.exists(),
            "reconciliation must not delete the symlink target"
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
