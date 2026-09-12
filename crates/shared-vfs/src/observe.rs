//! The operation-observation seam: [`Origin`], [`OpEvent`], and the op
//! sink.
//!
//! A handle with an installed sink fires it on every admitted operation -
//! after policy and claims pass, before the backend executes - with the
//! op kind, the canonical path, and the caller-supplied [`Origin`]. The
//! seam is fire-and-forget: no outcome flows back, and a policy-denied
//! operation never fires. Claims still key on the internal
//! [`ExecId`](crate::ExecId); the origin is observability, never
//! identity. The deferred consumers - the bounded event log, the Lua
//! pull query, and enrichment policies - subscribe through this seam in
//! later steps.

use std::panic::Location;
use std::sync::Arc;

use crate::path::VfsPath;
use crate::traits::Op;

/// Who asked for an operation: a label and the most precise source
/// position the caller knows. Pure observability - an origin never gates
/// an operation and never appears in a claim.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Origin {
    /// The most specific label the caller has: a section name for a
    /// chain, a tool id for a tool, a fixture name for a test.
    pub label: String,
    /// The source file or document `line` refers to: a Rust source file
    /// for [`Origin::new`], the prompt's name for [`Origin::at`].
    pub file: String,
    /// The 1-based line within `file`.
    pub line: u32,
}

impl Origin {
    /// Stamps the Rust call site via [`Location::caller`]: host code and
    /// tests get their position for free. Use the most specific label
    /// available - a section name for a chain, a tool id for a tool, a
    /// fixture name for a test - never a generic label when a specific
    /// one exists.
    #[must_use]
    #[track_caller]
    pub fn new(label: impl Into<String>) -> Origin {
        let caller = Location::caller();
        Origin {
            label: label.into(),
            file: caller.file().to_owned(),
            line: caller.line(),
        }
    }

    /// Sets an explicit position: the executor and the agent substitute
    /// the prompt's position for the Rust one, so every event's position
    /// is the most precise thing the caller knows. The label guidance of
    /// [`Origin::new`] applies unchanged.
    #[must_use]
    pub fn at(label: impl Into<String>, file: impl Into<String>, line: u32) -> Origin {
        Origin {
            label: label.into(),
            file: file.into(),
            line,
        }
    }
}

/// One admitted operation, handed to the installed sink. Borrows the
/// capability's own values, so firing allocates nothing; a sink that
/// retains events clones out of the views.
#[derive(Debug)]
pub struct OpEvent<'a> {
    pub(crate) op: Op,
    pub(crate) path: &'a VfsPath,
    pub(crate) origin: &'a Origin,
}

impl<'a> OpEvent<'a> {
    /// The operation kind.
    #[must_use]
    pub fn op(&self) -> Op {
        self.op
    }

    /// The canonical path the operation acts on. Two-path operations
    /// (rename, copy) fire one event per path.
    #[must_use]
    pub fn path(&self) -> &'a VfsPath {
        self.path
    }

    /// The origin of the capability that admitted the operation.
    #[must_use]
    pub fn origin(&self) -> &'a Origin {
        self.origin
    }
}

/// The installed operation sink. Must be cheap: store operations fire it
/// from the blocking pool, inline with the operation.
pub type OpSink = Arc<dyn Fn(OpEvent<'_>) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::Origin;

    #[test]
    fn origin_new_stamps_the_callers_file_and_line() {
        let origin = Origin::new("the fixture");
        assert_eq!(origin.line, line!() - 1, "the call site's line");
        assert!(
            origin.file.ends_with("observe.rs"),
            "the call site's file: {}",
            origin.file
        );
        assert_eq!(origin.label, "the fixture");
    }

    #[test]
    fn origin_at_carries_the_explicit_position() {
        let origin = Origin::at("the section", "the prompt", 42);
        assert_eq!(origin.label, "the section");
        assert_eq!(origin.file, "the prompt");
        assert_eq!(origin.line, 42);
    }
}
