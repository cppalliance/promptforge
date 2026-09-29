//! The PromptForge virtual filesystem: generic machinery (canonical
//! interned paths, the claims model, the mount router, the
//! operation-observation seam, and backends) and the promptforge policy
//! over it.
//!
//! The crate root holds promptforge policy: [`ModePolicy`], the editor mode
//! gate. The machinery modules hold no promptforge policy (no run
//! concepts). The declared store is generic machinery:
//! [`VfsRefBuilder::store`] names any mount as the store, and the store
//! view's strict logical-path rules are the store's caller contract, not
//! promptforge policy.
//!
//! This crate is the permanent bottom of the dependency stack: std only,
//! no workspace or external crates.

pub mod detail;
mod error;
mod glob;
mod handle;
mod host;
mod memory;
mod observe;
mod path;
mod router;
mod stat;
mod traits;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

pub use error::{PathReason, VfsError};
pub use handle::{Access, VfsRef};
pub use host::HostBackend;
pub use memory::MemoryBackend;
pub use observe::{OpEvent, OpSink, Origin};
pub use path::{VfsPath, VfsPathBuf};
pub use router::VfsRefBuilder;
pub use stat::{Entry, FileType, Stat};
pub use traits::{AcquireContext, AllowAll, ExecId, Op, Policy, Verdict, Vfs, VfsAccess};

/// The default handle: a memory store at `/` and nothing else.
///
/// [`VfsRefBuilder::store`] mounts the backend at the root and
/// declares it the store, so relative paths address the store
/// directly.
impl Default for VfsRef {
    fn default() -> VfsRef {
        VfsRef::builder().store("/", MemoryBackend::new()).build()
    }
}

/// The editor mode: what the model may mutate right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every mutation is refused pending user approval; reads flow.
    Ask,
    /// Mutations are allowed only to markdown paths; reads flow.
    Plan,
    /// Every operation is allowed.
    Agent,
}

/// The mode gate: one policy per handle, consulted on every operation
/// before the claims check. Modes gate mutations, never reads. The
/// current mode sits behind a shared `Arc`: the UI holds the
/// [`ModeHandle`] and flips modes mid-run, and the next operation sees
/// it - no executor involvement. One-way vs reversible is just who
/// still holds the handle.
#[derive(Debug)]
pub struct ModePolicy {
    mode: Arc<Mutex<Mode>>,
}

/// The UI's half of the mode gate. Clones share the one cell.
#[derive(Debug, Clone)]
pub struct ModeHandle {
    mode: Arc<Mutex<Mode>>,
}

impl ModePolicy {
    /// Returns a policy starting in `mode`.
    #[must_use]
    pub fn new(mode: Mode) -> ModePolicy {
        ModePolicy {
            mode: Arc::new(Mutex::new(mode)),
        }
    }

    /// Returns the UI's half: flipping it mid-run takes effect on the
    /// next operation.
    #[must_use]
    pub fn handle(&self) -> ModeHandle {
        ModeHandle {
            mode: Arc::clone(&self.mode),
        }
    }
}

impl ModeHandle {
    /// Sets the current mode.
    pub fn set(&self, mode: Mode) {
        *lock(&self.mode) = mode;
    }

    /// Returns the current mode.
    #[must_use]
    pub fn mode(&self) -> Mode {
        *lock(&self.mode)
    }
}

/// Poison-safe lock on the shared mode cell.
fn lock(mode: &Arc<Mutex<Mode>>) -> MutexGuard<'_, Mode> {
    mode.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the operation mutates storage. Modes gate mutations, never
/// reads.
fn is_mutation(op: Op) -> bool {
    matches!(
        op,
        Op::Write
            | Op::Append
            | Op::Delete
            | Op::Rename
            | Op::Mkdir
            | Op::Copy
            | Op::Symlink
            | Op::Chmod
    )
}

/// Plan mode's markdown rule: a `.md` suffix, case-sensitively - the
/// virtual namespace follows POSIX path rules strictly, so `.MD` is not
/// markdown and the case-insensitive suggestion does not apply.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "virtual paths are POSIX-strict; case-insensitive extension matching is a host-OS notion"
)]
fn is_markdown(path: &VfsPath) -> bool {
    path.as_str().ends_with(".md")
}

impl Policy for ModePolicy {
    fn check(&self, op: Op, path: &VfsPath) -> Verdict {
        if !is_mutation(op) {
            return Verdict::Allow;
        }
        match *lock(&self.mode) {
            Mode::Agent => Verdict::Allow,
            Mode::Ask => Verdict::Ask(format!(
                "{op:?} on {path} needs user approval: the Ask mode refuses all mutations"
            )),
            // Copy is checked per path, source and destination alike,
            // so Plan refuses to even read a non-markdown source. The
            // policy cannot tell the two apart; conservative refusal
            // is the safe side.
            Mode::Plan if is_markdown(path) => Verdict::Allow,
            Mode::Plan => Verdict::Deny(format!(
                "{op:?} on {path} is refused: the Plan mode allows mutations only to markdown paths"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, ModePolicy};
    use crate::{Origin, VfsError, VfsRef};

    /// A manifest section is a dependency table when it is exactly one of
    /// the three dependency tables, a sub-table of one
    /// (`[dependencies.foo]` declares a dependency the same way), or a
    /// target-qualified dependency table.
    fn is_dependency_table(section: &str) -> bool {
        const TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
        TABLES.iter().any(|table| {
            section == *table
                || section
                    .strip_prefix(*table)
                    .is_some_and(|rest| rest.starts_with('.'))
        }) || (section.starts_with("target.") && section.ends_with(".dependencies"))
    }

    /// The zero-dependency rule is load-bearing: this crate compiles alone
    /// and never rebuilds for a dependency rev, so the manifest must never
    /// declare a dependency. This test reads the crate's own Cargo.toml and
    /// fails if any dependency table has an entry.
    #[test]
    fn the_manifest_declares_no_dependencies() -> Result<(), std::io::Error> {
        let manifest = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
        )?;
        let mut section = String::new();
        for raw_line in manifest.lines() {
            let line = raw_line.trim();
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_owned();
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            assert!(
                !is_dependency_table(&section),
                "zero-dependency rule violated: [{section}] declares `{line}`"
            );
        }
        Ok(())
    }

    #[test]
    fn dependency_sub_tables_count_as_dependency_tables() {
        // Regression: `[dependencies.foo]` once slipped past the exact-
        // match section check while still declaring a dependency.
        for section in [
            "dependencies",
            "dependencies.foo",
            "dev-dependencies",
            "dev-dependencies.foo",
            "build-dependencies",
            "build-dependencies.foo",
            "target.'cfg(windows)'.dependencies",
        ] {
            assert!(is_dependency_table(section), "[{section}] must be caught");
        }
        for section in ["package", "lints", "features", "dependenciesfoo"] {
            assert!(!is_dependency_table(section), "[{section}] must pass");
        }
    }

    #[test]
    fn a_mode_flip_through_the_shared_handle_is_visible_on_the_next_operation()
    -> Result<(), VfsError> {
        let policy = ModePolicy::new(Mode::Ask);
        let handle = policy.handle();
        let vfs = VfsRef::with_policy(VfsRef::default(), policy);
        let access = vfs.acquire(Origin::new("mode flip test"))?;
        let path = "notes.md";
        match access.write(path, b"x") {
            Err(VfsError::PermissionDenied { reason, .. }) => {
                assert!(
                    reason.contains("Ask"),
                    "names the rule that fired: {reason}"
                );
            }
            other => panic!("expected an approval denial, got {other:?}"),
        }
        // The UI flips the mode mid-run through the shared handle; the
        // very next operation sees it.
        handle.set(Mode::Agent);
        access.write(path, b"x")?;
        assert_eq!(access.read(path)?, b"x");
        Ok(())
    }

    #[test]
    fn plan_mode_allows_mutations_only_to_markdown_paths() -> Result<(), VfsError> {
        let policy = ModePolicy::new(Mode::Plan);
        let vfs = VfsRef::with_policy(VfsRef::default(), policy);
        let access = vfs.acquire(Origin::new("plan mode test"))?;
        let markdown = "notes.md";
        let binary = "data.bin";
        access.write(markdown, b"# ok")?;
        match access.write(binary, b"x") {
            Err(VfsError::PermissionDenied { reason, .. }) => {
                assert!(
                    reason.contains("Plan"),
                    "names the rule that fired: {reason}"
                );
            }
            other => panic!("expected a denial, got {other:?}"),
        }
        // The denied write never partially applied.
        assert!(!access.exists(binary)?);
        Ok(())
    }

    #[test]
    fn modes_gate_mutations_never_reads() -> Result<(), VfsError> {
        let policy = ModePolicy::new(Mode::Agent);
        let handle = policy.handle();
        let vfs = VfsRef::with_policy(VfsRef::default(), policy);
        let path = "paper.md";
        vfs.acquire(Origin::new("read gate test"))?
            .write(path, b"text")?;
        // Even in Ask, the strictest mode, reads flow.
        handle.set(Mode::Ask);
        let access = vfs.acquire(Origin::new("read gate test"))?;
        assert_eq!(access.read(path)?, b"text");
        assert!(access.exists(path)?);
        Ok(())
    }
}
