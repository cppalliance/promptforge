//! The policy hook: the [`Op`] a policy matches on, its [`Verdict`], the
//! [`Policy`] trait each handle consults, and the [`AllowAll`] default.

use crate::path::VfsPath;

/// The kind of filesystem operation being attempted, which a policy
/// matches on.
///
/// The enum is `#[non_exhaustive]`, so a `match` on it outside this crate
/// needs a wildcard arm. A policy should deny in that arm, so an operation
/// added later is refused until the policy names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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

/// A policy's decision about one operation.
///
/// Each reason string reaches a reader. The `Deny` string goes back to
/// the model as the tool error, so it tells the model how to recover.
/// The `Ask` string names what is being asked and which rule fired, for
/// the user to read in an approval dialog. The filesystem refuses a
/// `Deny` operation with `VfsError::PermissionDenied` and an `Ask`
/// operation with `VfsError::ApprovalRequired`, so any approval dialog
/// belongs to the application.
///
/// The enum is `#[non_exhaustive]`, so a `match` on it outside this crate
/// needs a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Verdict {
    /// The operation may proceed.
    Allow,
    /// The operation is refused. The string tells the model how to
    /// recover.
    Deny(String),
    /// The operation needs the user's approval. The string is the text
    /// for an approval dialog.
    Ask(String),
}

/// A hook that decides whether each filesystem operation may proceed.
///
/// Each `VfsRef` has one policy. `Access` consults it on every
/// operation, before checking for conflicting claims. A policy can
/// change its answers during a run through shared state: the
/// application holds the same `Arc` and changes the state mid-run.
pub trait Policy: Send {
    /// Decides whether `op` on `path` may proceed.
    fn check(&self, op: Op, path: &VfsPath) -> Verdict;
}

/// A policy that allows every operation.
///
/// `VfsRef::new` uses it, and `VfsRefBuilder` uses it by default.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AllowAll;

impl Policy for AllowAll {
    fn check(&self, op: Op, path: &VfsPath) -> Verdict {
        let _ = (op, path);
        Verdict::Allow
    }
}
