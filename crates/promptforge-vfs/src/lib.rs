//! PromptForge policy over the shared virtual filesystem machinery.
//!
//! This crate carries promptforge policy, never generic machinery: the
//! `/_promptforge` mount layout, the [`empty`] stock handle, and
//! [`ModePolicy`], the editor mode gate. Generic machinery (traits,
//! claims, routing, backends) lives in `shared-vfs` below.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use shared_vfs::{MemoryBackend, Op, Policy, Verdict, VfsPath, VfsRef};

/// The mount prefix of the run-scoped store. Hosts seed before `run()`
/// and extract after through this mount; callers never hardcode the
/// path - [`empty`] installs it and the Store facade scopes to it.
pub const STORE_MOUNT: &str = "/_promptforge/store";

/// The stock handle: a router with a fresh memory backend at
/// [`STORE_MOUNT`]. Empty of content, not of mounts, so callers can
/// seed before `run()` and extract after.
#[must_use]
pub fn empty() -> VfsRef {
    VfsRef::builder()
        .mount(STORE_MOUNT, MemoryBackend::new())
        .build()
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
/// current mode lives behind a shared `Arc`: the UI holds the
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
/// virtual namespace is POSIX-shaped and strict, so `.MD` is not
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
    use shared_vfs::{Origin, VfsError, VfsRef};

    use super::{Mode, ModePolicy, STORE_MOUNT, empty};

    #[test]
    fn empty_carries_the_store_mount() -> Result<(), VfsError> {
        let vfs = empty();
        let access = vfs.acquire(Origin::new("empty store mount test"))?;
        let path = format!("{STORE_MOUNT}/paper.md");
        access.write(&path, b"# draft")?;
        assert_eq!(access.read(&path)?, b"# draft");
        // Empty of content, not of mounts: the mount exists and serves.
        assert!(access.exists(&path)?);
        Ok(())
    }

    #[test]
    fn empty_serves_nothing_outside_the_store_mount() -> Result<(), VfsError> {
        let vfs = empty();
        let access = vfs.acquire(Origin::new("empty namespace test"))?;
        assert!(matches!(
            access.read("/elsewhere.txt"),
            Err(VfsError::NotFound(_))
        ));
        Ok(())
    }

    #[test]
    fn a_mode_flip_through_the_shared_handle_is_visible_on_the_next_operation()
    -> Result<(), VfsError> {
        let policy = ModePolicy::new(Mode::Ask);
        let handle = policy.handle();
        let vfs = VfsRef::with_policy(empty(), policy);
        let access = vfs.acquire(Origin::new("mode flip test"))?;
        let path = format!("{STORE_MOUNT}/notes.md");
        match access.write(&path, b"x") {
            Err(VfsError::PermissionDenied(reason)) => {
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
        access.write(&path, b"x")?;
        assert_eq!(access.read(&path)?, b"x");
        Ok(())
    }

    #[test]
    fn plan_mode_allows_mutations_only_to_markdown_paths() -> Result<(), VfsError> {
        let policy = ModePolicy::new(Mode::Plan);
        let vfs = VfsRef::with_policy(empty(), policy);
        let access = vfs.acquire(Origin::new("plan mode test"))?;
        let markdown = format!("{STORE_MOUNT}/notes.md");
        let binary = format!("{STORE_MOUNT}/data.bin");
        access.write(&markdown, b"# ok")?;
        match access.write(&binary, b"x") {
            Err(VfsError::PermissionDenied(reason)) => {
                assert!(
                    reason.contains("Plan"),
                    "names the rule that fired: {reason}"
                );
            }
            other => panic!("expected a denial, got {other:?}"),
        }
        // The denied write never partially applied.
        assert!(!access.exists(&binary)?);
        Ok(())
    }

    #[test]
    fn modes_gate_mutations_never_reads() -> Result<(), VfsError> {
        let policy = ModePolicy::new(Mode::Agent);
        let handle = policy.handle();
        let vfs = VfsRef::with_policy(empty(), policy);
        let path = format!("{STORE_MOUNT}/paper.md");
        vfs.acquire(Origin::new("read gate test"))?
            .write(&path, b"text")?;
        // Even in Ask, the strictest mode, reads flow.
        handle.set(Mode::Ask);
        let access = vfs.acquire(Origin::new("read gate test"))?;
        assert_eq!(access.read(&path)?, b"text");
        assert!(access.exists(&path)?);
        Ok(())
    }
}
