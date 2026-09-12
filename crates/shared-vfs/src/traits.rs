//! The backend traits, the policy hook, and execution identity.
//!
//! `Vfs` is one backend behind the virtual namespace; `VfsAccess` is one
//! identity's session with it, carrying every filesystem operation.
//! `Policy` is the per-handle hook consulted before the claims check, and
//! `ExecId` is the identity every operation is attributed to.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::VfsError;
use crate::path::{VfsPath, VfsPathBuf, canonicalize};
use crate::types::{Entry, GrepMatch, GrepQuery, GrepResults, Stat};

/// Identity of one serial thread of execution. Process-unique, vended
/// from a process-global monotonic counter. Opaque: no public constructor -
/// it must be nameable (it appears in [`Vfs::acquire`]), but only the
/// handle vends them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExecId(u64);

impl ExecId {
    /// Vends the next process-unique identity.
    // The handle arrives in a later step; nothing vends identities today.
    #[allow(dead_code)]
    pub(crate) fn vend() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// One backend behind the virtual namespace.
///
/// Sync by design: the Lua VM and the executor's single driver thread are
/// synchronous. Bytes at the operation level. `Send` is required, `Sync`
/// is not: the handle serializes access. The only way to touch storage is
/// to acquire an access object bound to an identity.
pub trait Vfs: Send {
    /// Acquires an access object bound to `id`. Every operation on the
    /// returned object is attributed to that identity: backends that
    /// care can know who is touching what; the rest ignore it.
    fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError>;

    /// Releases `id`. Also called from the access object's Drop, so
    /// teardown paths (cancel, panic, early return) cannot skip it.
    fn release(&mut self, id: ExecId) -> Result<(), VfsError>;

    /// Whether this backend rejects all mutations.
    fn read_only(&self) -> bool {
        false
    }
}

/// One identity's session with a backend. Holds the ExecId.
/// All filesystem operations live here - no access object, no ops.
/// Paths arrive validated, canonicalized, and interned; backends never
/// re-validate.
pub trait VfsAccess: Send {
    /// Reads the file at `path` exactly as stored.
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError>;

    /// Reads `len` bytes starting at byte `offset`.
    ///
    /// Default: read whole, slice. Backends that can seek (host
    /// directory, SQLite) override and never materialize the file.
    /// The handle's line-based ranges are built on this.
    fn read_range(&self, path: &VfsPath, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        let data = self.read(path)?;
        let Ok(start) = usize::try_from(offset) else {
            return Err(VfsError::Backend(format!(
                "read_range offset {offset} exceeds the addressable size"
            )));
        };
        let Ok(length) = usize::try_from(len) else {
            return Err(VfsError::Backend(format!(
                "read_range length {len} exceeds the addressable size"
            )));
        };
        if start >= data.len() {
            return Ok(Vec::new());
        }
        let end = data.len().min(start.saturating_add(length));
        Ok(data[start..end].to_vec())
    }

    /// Creates or overwrites the file at `path`.
    ///
    /// Noted but not implemented in v1: a defaulted
    /// `write_owned(&mut self, path: &VfsPath, contents: Vec<u8>)`
    /// delegating to `write`, which the memory overlay would override to
    /// move the buffer with zero copies. Add when profiling calls for it.
    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError>;

    /// Appends to the file at `path`, creating it if absent.
    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError>;

    /// Removes the file, link, or directory at `path`.
    /// Absent is NotFound; a directory without `recursive` is an error.
    /// On a symlink, removes the link, never the target.
    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError>;

    /// A confirmed absence is `Ok(false)`; a backend failure is `Err`.
    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError>;

    /// Returns stored paths matching `pattern`, sorted.
    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError>;

    /// Lists the directory at `path`.
    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError>;

    /// Returns metadata for `path`.
    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError>;

    /// Creates the directory at `path`.
    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError>;

    /// Renames or moves, atomically where the backend allows.
    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError>;

    /// Copies the file at `from` to `to`.
    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError>;

    /// Replaces the unique occurrence of `old` with `new`.
    /// Zero matches and multiple matches are both errors.
    /// Default: read, count, replace, write. Override to push down.
    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str) -> Result<(), VfsError> {
        let bytes = self.read(path)?;
        let text = String::from_utf8(bytes).map_err(|_| {
            VfsError::Backend(format!("str_replace requires UTF-8 text: {path}"))
        })?;
        let count = text.matches(old).count();
        if count == 0 {
            return Err(VfsError::Backend(format!(
                "str_replace found no occurrence of {old:?} in {path}"
            )));
        }
        if count > 1 {
            return Err(VfsError::Backend(format!(
                "str_replace found {count} occurrences of {old:?} in {path}; exactly one is required"
            )));
        }
        let replaced = text.replacen(old, new, 1);
        self.write(path, replaced.as_bytes())
    }

    /// Searches files under the query's root.
    ///
    /// Default: glob, read, line scan with literal substring matching.
    /// Override for indexed backends. Regex queries return
    /// [`VfsError::Unsupported`]: this crate is std-only, so a regex
    /// engine must come from an overriding backend. Non-UTF-8 files and
    /// directories are skipped.
    fn grep(&self, query: &GrepQuery) -> Result<GrepResults, VfsError> {
        if query.is_regex {
            return Err(VfsError::Unsupported(
                "the default grep matches literal text only; regex requires a backend override"
                    .into(),
            ));
        }
        let base = match query.root.as_str() {
            "/" => "",
            root => root,
        };
        let pattern = match &query.glob_filter {
            Some(filter) => format!("{base}/**/{filter}"),
            None => format!("{base}/**/*"),
        };
        let mut matches = Vec::new();
        let mut truncated = false;
        'files: for path in self.glob(&pattern)? {
            let vfs_path = canonicalize(&path)?;
            let bytes = match self.read(&vfs_path) {
                Ok(bytes) => bytes,
                Err(VfsError::IsADirectory(_)) => continue,
                Err(err) => return Err(err),
            };
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                let hit = if query.case_insensitive {
                    line.to_lowercase()
                        .contains(&query.pattern.to_lowercase())
                } else {
                    line.contains(&query.pattern)
                };
                if !hit {
                    continue;
                }
                if let Some(cap) = query.max_results {
                    if matches.len() >= cap {
                        truncated = true;
                        break 'files;
                    }
                }
                matches.push(GrepMatch {
                    path: path.clone(),
                    line_number: index + 1,
                    line: line.to_owned(),
                });
            }
        }
        Ok(GrepResults { matches, truncated })
    }

    /// Creates a symbolic link at `link` naming `target`.
    ///
    /// POSIX extra; the default returns [`VfsError::Unsupported`].
    fn symlink(&mut self, target: &VfsPath, link: &VfsPath) -> Result<(), VfsError> {
        let _ = target;
        Err(VfsError::Unsupported(format!(
            "symlink is not supported by this backend: {link}"
        )))
    }

    /// Reads the target of the symbolic link at `path`.
    ///
    /// POSIX extra; the default returns [`VfsError::Unsupported`].
    fn read_link(&self, path: &VfsPath) -> Result<VfsPathBuf, VfsError> {
        Err(VfsError::Unsupported(format!(
            "read_link is not supported by this backend: {path}"
        )))
    }

    /// Changes the mode bits of `path`.
    ///
    /// POSIX extra; the default returns [`VfsError::Unsupported`].
    fn chmod(&mut self, path: &VfsPath, mode: u32) -> Result<(), VfsError> {
        let _ = mode;
        Err(VfsError::Unsupported(format!(
            "chmod is not supported by this backend: {path}"
        )))
    }
}

/// What operation is being attempted - the policy matches on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    /// Reading a file's bytes.
    Read,
    /// Creating or overwriting a file.
    Write,
    /// Appending to a file.
    Append,
    /// Removing a file, link, or directory.
    Delete,
    /// Renaming or moving a path.
    Rename,
    /// Creating a directory.
    Mkdir,
    /// Copying a file.
    Copy,
    /// Searching file contents.
    Grep,
    /// Testing for existence.
    Exists,
    /// Matching paths against a pattern.
    Glob,
    /// Listing a directory.
    List,
    /// Reading metadata.
    Stat,
    /// Creating a symbolic link.
    Symlink,
    /// Reading a symbolic link's target.
    ReadLink,
    /// Changing mode bits.
    Chmod,
}

/// The policy's answer. Reasons are load-bearing in both directions:
/// Deny's string flows back to the model as the tool error (its
/// recovery path); Ask's string is what the user sees in the
/// approval dialog (what is being asked, and which rule fired).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The operation may proceed.
    Allow,
    /// The operation is refused; the string is the model's recovery path.
    Deny(String),
    /// The operation needs user approval; the string is the dialog text.
    Ask(String),
}

/// One policy per VfsRef, consulted by Access on every operation,
/// before the claims check. Dynamic through shared state: the host
/// or UI holds the same Arc and changes behavior mid-run.
pub trait Policy: Send {
    /// Decides whether `op` on `path` may proceed.
    fn check(&self, op: Op, path: &VfsPath) -> Verdict;
}

/// The v1 policy: every operation is allowed.
#[derive(Debug, Default)]
pub struct AllowAll;

impl Policy for AllowAll {
    fn check(&self, op: Op, path: &VfsPath) -> Verdict {
        let _ = (op, path);
        Verdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{AllowAll, Op, Policy, Verdict, VfsAccess};
    use crate::error::VfsError;
    use crate::path::{VfsPath, canonicalize};
    use crate::types::{Entry, GrepQuery, GrepResults, Stat};

    /// Minimal in-memory backend exercising the trait defaults: the
    /// required methods are direct map operations, and glob understands
    /// the one pattern shape the default grep emits (`<root>/**<filter>`).
    struct StubBackend {
        files: BTreeMap<String, Vec<u8>>,
    }

    fn stub(files: &[(&str, &str)]) -> StubBackend {
        StubBackend {
            files: files
                .iter()
                .map(|(name, text)| ((*name).to_owned(), text.as_bytes().to_vec()))
                .collect(),
        }
    }

    fn path(s: &str) -> Result<VfsPath, VfsError> {
        canonicalize(s)
    }

    fn query(root: &str, pattern: &str) -> Result<GrepQuery, VfsError> {
        Ok(GrepQuery {
            pattern: pattern.to_owned(),
            root: canonicalize(root)?.to_buf(),
            is_regex: false,
            case_insensitive: false,
            glob_filter: None,
            max_results: None,
        })
    }

    impl VfsAccess for StubBackend {
        fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
            self.files
                .get(path.as_str())
                .cloned()
                .ok_or_else(|| VfsError::NotFound(path.to_string()))
        }

        fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
            self.files.insert(path.to_string(), contents.to_vec());
            Ok(())
        }

        fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
            self.files
                .entry(path.to_string())
                .or_default()
                .extend_from_slice(contents);
            Ok(())
        }

        fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
            let _ = recursive;
            self.files
                .remove(path.as_str())
                .map(|_| ())
                .ok_or_else(|| VfsError::NotFound(path.to_string()))
        }

        fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
            Ok(self.files.contains_key(path.as_str()))
        }

        fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
            let Some(index) = pattern.find("/**/") else {
                return Ok(Vec::new());
            };
            let prefix = format!("{}/", &pattern[..index]);
            let filter = &pattern[index + 4..];
            let suffix = filter.strip_prefix('*').unwrap_or(filter);
            Ok(self
                .files
                .keys()
                .filter(|name| name.starts_with(&prefix) && name.ends_with(suffix))
                .cloned()
                .collect())
        }

        fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
            let _ = path;
            Err(VfsError::Unsupported("the stub does not list".into()))
        }

        fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
            let _ = path;
            Err(VfsError::Unsupported("the stub does not stat".into()))
        }

        fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
            let _ = (path, recursive);
            Ok(())
        }

        fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
            let bytes = self
                .files
                .remove(from.as_str())
                .ok_or_else(|| VfsError::NotFound(from.to_string()))?;
            self.files.insert(to.to_string(), bytes);
            Ok(())
        }

        fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
            let bytes = self
                .files
                .get(from.as_str())
                .cloned()
                .ok_or_else(|| VfsError::NotFound(from.to_string()))?;
            self.files.insert(to.to_string(), bytes);
            Ok(())
        }
    }

    #[test]
    fn the_default_read_range_slices_a_whole_read() -> Result<(), VfsError> {
        let backend = stub(&[("/a.txt", "hello world")]);
        let bytes = backend.read_range(&path("/a.txt")?, 6, 5)?;
        assert_eq!(bytes, b"world");
        Ok(())
    }

    #[test]
    fn the_default_read_range_clips_at_the_end_of_the_file() -> Result<(), VfsError> {
        let backend = stub(&[("/a.txt", "hello")]);
        assert_eq!(backend.read_range(&path("/a.txt")?, 2, 100)?, b"llo");
        assert!(backend.read_range(&path("/a.txt")?, 100, 5)?.is_empty());
        Ok(())
    }

    #[test]
    fn the_default_str_replace_rewrites_the_unique_occurrence() -> Result<(), VfsError> {
        let mut backend = stub(&[("/a.txt", "alpha beta gamma")]);
        backend.str_replace(&path("/a.txt")?, "beta", "BETA")?;
        assert_eq!(backend.read(&path("/a.txt")?)?, b"alpha BETA gamma");
        Ok(())
    }

    #[test]
    fn the_default_str_replace_rejects_zero_matches() -> Result<(), VfsError> {
        let mut backend = stub(&[("/a.txt", "alpha beta")]);
        let result = backend.str_replace(&path("/a.txt")?, "missing", "x");
        assert!(matches!(result, Err(VfsError::Backend(_))));
        assert_eq!(backend.read(&path("/a.txt")?)?, b"alpha beta");
        Ok(())
    }

    #[test]
    fn the_default_str_replace_rejects_multiple_matches() -> Result<(), VfsError> {
        let mut backend = stub(&[("/a.txt", "foo and foo")]);
        let result = backend.str_replace(&path("/a.txt")?, "foo", "bar");
        assert!(matches!(result, Err(VfsError::Backend(_))));
        assert_eq!(backend.read(&path("/a.txt")?)?, b"foo and foo");
        Ok(())
    }

    #[test]
    fn the_default_grep_matches_literal_text_with_line_numbers() -> Result<(), VfsError> {
        let backend = stub(&[
            ("/docs/a.md", "first hit line\nplain line\nsecond hit line"),
            ("/docs/b.md", "nothing here"),
        ]);
        let results: GrepResults = backend.grep(&query("/docs", "hit")?)?;
        assert!(!results.truncated);
        assert_eq!(results.matches.len(), 2);
        assert_eq!(results.matches[0].path, "/docs/a.md");
        assert_eq!(results.matches[0].line_number, 1);
        assert_eq!(results.matches[0].line, "first hit line");
        assert_eq!(results.matches[1].line_number, 3);
        Ok(())
    }

    #[test]
    fn the_default_grep_honors_case_insensitive_matching() -> Result<(), VfsError> {
        let backend = stub(&[("/a.txt", "MixedCase line")]);
        let mut q = query("/", "mixedcase")?;
        assert!(backend.grep(&q)?.matches.is_empty());
        q.case_insensitive = true;
        assert_eq!(backend.grep(&q)?.matches.len(), 1);
        Ok(())
    }

    #[test]
    fn the_default_grep_scopes_the_search_to_the_glob_filter() -> Result<(), VfsError> {
        let backend = stub(&[("/src/a.rs", "needle"), ("/src/b.txt", "needle")]);
        let mut q = query("/src", "needle")?;
        q.glob_filter = Some("*.rs".to_owned());
        let results = backend.grep(&q)?;
        assert_eq!(results.matches.len(), 1);
        assert_eq!(results.matches[0].path, "/src/a.rs");
        Ok(())
    }

    #[test]
    fn the_default_grep_caps_results_and_reports_truncation() -> Result<(), VfsError> {
        let backend = stub(&[("/a.txt", "hit\nhit\nhit")]);
        let mut q = query("/", "hit")?;
        q.max_results = Some(2);
        let results = backend.grep(&q)?;
        assert_eq!(results.matches.len(), 2);
        assert!(results.truncated);
        q.max_results = Some(10);
        let results = backend.grep(&q)?;
        assert_eq!(results.matches.len(), 3);
        assert!(!results.truncated);
        Ok(())
    }

    #[test]
    fn the_default_grep_rejects_regex_without_a_backend_override() -> Result<(), VfsError> {
        let backend = stub(&[("/a.txt", "hit")]);
        let mut q = query("/", "h.t")?;
        q.is_regex = true;
        assert!(matches!(backend.grep(&q), Err(VfsError::Unsupported(_))));
        Ok(())
    }

    #[test]
    fn unsupported_posix_defaults_return_the_right_error_kind() -> Result<(), VfsError> {
        let mut backend = stub(&[("/a.txt", "x")]);
        assert!(matches!(
            backend.symlink(&path("/a.txt")?, &path("/b.txt")?),
            Err(VfsError::Unsupported(_))
        ));
        assert!(matches!(
            backend.read_link(&path("/a.txt")?),
            Err(VfsError::Unsupported(_))
        ));
        assert!(matches!(
            backend.chmod(&path("/a.txt")?, 0o644),
            Err(VfsError::Unsupported(_))
        ));
        Ok(())
    }

    #[test]
    fn allow_all_permits_every_operation() -> Result<(), VfsError> {
        let policy = AllowAll;
        assert_eq!(policy.check(Op::Write, &path("/a.txt")?), Verdict::Allow);
        Ok(())
    }
}
