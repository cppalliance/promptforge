//! The cloneable handle, the RAII capability, and the claims tables.
//!
//! [`VfsRef`] is the public handle: an `Arc`-shared volume pairing one
//! backend with the claims ledger, behind poison-safe locks. [`Access`]
//! is the RAII capability vended by [`VfsRef::acquire`]: it canonicalizes
//! paths at receipt, consults the handle's policy before the claims check
//! so a denied operation never registers a claim, registers claims, and
//! locks the backend's access object per call. Dropping an [`Access`]
//! releases the identity and its claims, so cancellation, panics, and
//! early returns cannot leak claims.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::VfsError;
use crate::path::{VfsPath, canonicalize};
use crate::router::{Mounts, Router, VfsRefBuilder};
use crate::traits::{AllowAll, ExecId, Op, Policy, Verdict, Vfs, VfsAccess};
use crate::types::{Entry, GrepQuery, GrepResults, Stat};

/// Whether an operation claims read or write intent on its path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClaimKind {
    Read,
    Write,
}

impl fmt::Display for ClaimKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => f.write_str("read"),
            Self::Write => f.write_str("write"),
        }
    }
}

/// The bookkeeping of who is touching what. Every entry is a live,
/// conflict-eligible claim; retired or released claims are deleted,
/// never stored.
struct Claims {
    inner: Mutex<ClaimsTables>,
}

struct ClaimsTables {
    readers: HashMap<VfsPath, Vec<ExecId>>,
    writers: HashMap<VfsPath, Vec<ExecId>>,
    live: HashSet<ExecId>,
}

impl Claims {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ClaimsTables {
                readers: HashMap::new(),
                writers: HashMap::new(),
                live: HashSet::new(),
            }),
        }
    }

    /// Poison-safe lock: each guard scope is one complete table mutation,
    /// so a panicking claimant cannot leave the tables half-updated.
    fn tables(&self) -> MutexGuard<'_, ClaimsTables> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Marks `id` as live. Only live identities hold claims; released
    /// ones are forgotten entirely.
    fn register_live(&self, id: ExecId) {
        self.tables().live.insert(id);
    }

    /// Registers `id`'s claim of `kind` on `path`, failing when another
    /// live identity already holds a conflicting claim. A write conflicts
    /// with any other identity in the path's readers or writers; a read
    /// conflicts only with another identity in its writers; read-read
    /// never conflicts. An identity never conflicts with itself.
    fn claim(&self, path: VfsPath, id: ExecId, kind: ClaimKind) -> Result<(), VfsError> {
        let mut tables = self.tables();
        if let Some(other) = other_claimant(&tables.writers, path, id) {
            return Err(conflict(path, id, kind, other, ClaimKind::Write));
        }
        if kind == ClaimKind::Write
            && let Some(other) = other_claimant(&tables.readers, path, id)
        {
            return Err(conflict(path, id, kind, other, ClaimKind::Read));
        }
        let map = match kind {
            ClaimKind::Read => &mut tables.readers,
            ClaimKind::Write => &mut tables.writers,
        };
        let claimants = map.entry(path).or_default();
        if !claimants.contains(&id) {
            claimants.push(id);
        }
        Ok(())
    }

    /// Deletes `id`'s claims but keeps the identity live: the spawn of a
    /// child is the happens-before edge, so claims that predate the child
    /// can never conflict again.
    fn retire(&self, id: ExecId) {
        delete_claims(&mut self.tables(), id);
    }

    /// Deletes `id`'s claims and forgets the identity.
    fn release(&self, id: ExecId) {
        let mut tables = self.tables();
        tables.live.remove(&id);
        delete_claims(&mut tables, id);
    }
}

/// Returns the first claimant of `path` in `map` other than `id`.
fn other_claimant(
    map: &HashMap<VfsPath, Vec<ExecId>>,
    path: VfsPath,
    id: ExecId,
) -> Option<ExecId> {
    map.get(&path)?.iter().find(|&&other| other != id).copied()
}

/// Removes every claim held by `id`, dropping emptied path entries.
fn delete_claims(tables: &mut ClaimsTables, id: ExecId) {
    tables.readers.retain(|_, ids| {
        ids.retain(|&other| other != id);
        !ids.is_empty()
    });
    tables.writers.retain(|_, ids| {
        ids.retain(|&other| other != id);
        !ids.is_empty()
    });
}

/// The conflict error names the path, both identities, and both claim
/// kinds: the executor maps it to a fatal run error, and the message is
/// the whole diagnosis.
fn conflict(
    path: VfsPath,
    id: ExecId,
    kind: ClaimKind,
    other: ExecId,
    other_kind: ClaimKind,
) -> VfsError {
    VfsError::Conflict(format!(
        "{kind} on {path} by {id:?} conflicts with a {other_kind} claim by {other:?}"
    ))
}

/// One mounted filesystem instance: its backend and the ledger of who is
/// touching what. The two are separately `Arc`-shareable so `overlay()`
/// (a later step) can share the claims table while swapping the backend.
struct Volume {
    backend: Arc<Mutex<Box<dyn Vfs>>>,
    claims: Arc<Claims>,
}

/// The cloneable handle over one backend and its claims ledger.
///
/// Clones share the backend, the claims tables, and the policy: claims
/// registered through one clone conflict with operations through another.
#[derive(Clone)]
pub struct VfsRef {
    volume: Arc<Volume>,
    policy: Arc<dyn Policy + Sync>,
}

impl fmt::Debug for VfsRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VfsRef").finish_non_exhaustive()
    }
}

impl VfsRef {
    /// Returns a handle over `backend` with the [`AllowAll`] policy.
    pub fn new(backend: impl Vfs + 'static) -> VfsRef {
        Self::with_policy(backend, AllowAll)
    }

    /// Returns a handle over `backend` consulting `policy` on every
    /// operation. The policy is dynamic through shared state: the host
    /// holds the same `Arc` and changes behavior mid-run, and the next
    /// operation sees it.
    pub fn with_policy(
        backend: impl Vfs + 'static,
        policy: impl Policy + Sync + 'static,
    ) -> VfsRef {
        VfsRef {
            volume: Arc::new(Volume {
                backend: Arc::new(Mutex::new(Box::new(backend))),
                claims: Arc::new(Claims::new()),
            }),
            policy: Arc::new(policy),
        }
    }

    /// Returns a builder for installing mounts. Mounts are fixed at
    /// [`VfsRefBuilder::build`], so the table is immutable and cheap to
    /// `Arc`-share thereafter.
    #[must_use]
    pub fn builder() -> VfsRefBuilder {
        VfsRefBuilder::new()
    }

    /// Returns a handle with `backend` mounted at `prefix` over this
    /// handle's namespace. The claims table is shared: conflicts are
    /// detected across both views of the same storage.
    ///
    /// # Panics
    /// Panics when `prefix` is not an absolute virtual path or is the
    /// root: an overlay at `/` would replace the base entirely, so use
    /// [`VfsRef::new`] instead.
    #[must_use]
    pub fn overlay(&self, prefix: &str, backend: impl Vfs + 'static) -> VfsRef {
        let canonical = canonicalize(prefix)
            .unwrap_or_else(|err| panic!("invalid overlay prefix {prefix:?}: {err}"));
        assert!(
            canonical.as_str() != "/",
            "an overlay at / would replace the base entirely; use VfsRef::new instead"
        );
        // The base handle mounts at the root of the overlay's router:
        // operations outside the overlay prefix route through the base's
        // own policy and claims under the caller's identity.
        let mut mounts = Mounts::new();
        let root = canonicalize("/")
            .unwrap_or_else(|err| panic!("the namespace root is always valid: {err}"))
            .to_buf();
        mounts.insert(root, Arc::new(Mutex::new(Box::new(self.clone()))));
        mounts.insert(canonical.to_buf(), Arc::new(Mutex::new(Box::new(backend))));
        VfsRef {
            volume: Arc::new(Volume {
                backend: Arc::new(Mutex::new(Box::new(Router::new(mounts)))),
                claims: Arc::clone(&self.volume.claims),
            }),
            policy: Arc::clone(&self.policy),
        }
    }

    /// Acquires the capability for a new serial thread of execution.
    /// This is the only way in: every acquire vends a fresh [`ExecId`].
    ///
    /// # Panics
    /// Panics when the backend fails to acquire the identity. Backends
    /// are expected to accept attribution; a refusal is a backend bug,
    /// not a runtime condition.
    pub fn acquire(&self) -> Access {
        self.acquire_with(ExecId::vend())
    }

    /// Acquires the capability under a given identity: how a mounted
    /// handle forwards the caller's attribution. The identity registers
    /// as live in this handle's claims table, so conflicts are detected
    /// across both views of the same storage.
    ///
    /// # Panics
    /// Panics when the backend fails to acquire the identity; see
    /// [`VfsRef::acquire`].
    pub(crate) fn acquire_with(&self, id: ExecId) -> Access {
        let inner = self
            .backend()
            .acquire(id)
            .unwrap_or_else(|err| panic!("the backend refused to acquire identity {id:?}: {err}"));
        self.volume.claims.register_live(id);
        Access {
            id,
            volume: self.volume.clone(),
            policy: self.policy.clone(),
            inner: Mutex::new(inner),
        }
    }

    /// Builds a handle over a router with a fresh claims table and the
    /// [`AllowAll`] policy: the builder's exit.
    pub(crate) fn from_router(router: Router) -> VfsRef {
        VfsRef {
            volume: Arc::new(Volume {
                backend: Arc::new(Mutex::new(Box::new(router))),
                claims: Arc::new(Claims::new()),
            }),
            policy: Arc::new(AllowAll),
        }
    }

    /// Poison-safe lock on the backend.
    fn backend(&self) -> MutexGuard<'_, Box<dyn Vfs>> {
        self.volume
            .backend
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The public capability. Holds an [`ExecId`] and the backend's access
/// object; every operation canonicalizes the path, checks the policy,
/// checks the claims tables, then locks the backend per call.
#[must_use = "an acquire dropped immediately is a bug: the capability carries the identity's claims"]
pub struct Access {
    id: ExecId,
    volume: Arc<Volume>,
    policy: Arc<dyn Policy + Sync>,
    inner: Mutex<Box<dyn VfsAccess>>,
}

impl Access {
    /// Returns the capability for a new concurrent thread of execution.
    /// The child gets a fresh [`ExecId`], and this capability's claims
    /// are deleted from the tables: they predate the child by
    /// construction, so a retired claim can never conflict again. The
    /// spawn IS the happens-before edge - no fence call, no epochs.
    ///
    /// # Panics
    /// Panics when the backend fails to acquire the child's identity;
    /// see [`VfsRef::acquire`].
    pub fn spawn(&self) -> Access {
        let id = ExecId::vend();
        let inner = self
            .backend()
            .acquire(id)
            .unwrap_or_else(|err| panic!("the backend refused to acquire identity {id:?}: {err}"));
        self.volume.claims.retire(self.id);
        self.volume.claims.register_live(id);
        Access {
            id,
            volume: self.volume.clone(),
            policy: self.policy.clone(),
            inner: Mutex::new(inner),
        }
    }

    /// Returns this capability's identity.
    pub fn id(&self) -> ExecId {
        self.id
    }

    /// Reads the file at `path` exactly as stored.
    ///
    /// # Errors
    /// Returns an error when the policy denies the read, when another
    /// live identity holds a write claim on `path`, or when the backend
    /// fails.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let path = self.gate(Op::Read, path, ClaimKind::Read)?;
        self.inner().read(&path)
    }

    /// Reads the file at `path` as UTF-8 text.
    ///
    /// # Errors
    /// Returns an error when the file's contents are not UTF-8.
    pub fn read_string(&self, path: &str) -> Result<String, VfsError> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes)
            .map_err(|_| VfsError::Backend(format!("read_string requires UTF-8 text: {path}")))
    }

    /// Reads lines `start..=end` of the file at `path`, 1-based and
    /// inclusive, joined by `"\n"` with no trailing newline. An omitted
    /// `end` means the last line; a given `end` clamps down to it; a
    /// `start` past the last line reads as the empty string.
    ///
    /// # Errors
    /// Returns an error when `start` is below 1 or `end` is before
    /// `start`, when the file is missing, or when it is not UTF-8.
    pub fn read_range(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, VfsError> {
        self.with_line_range(path, start, end, |lines, _| lines.join("\n"))
    }

    /// Reads lines `start..=end` as numbered lines, numbered absolutely
    /// from `start`, each right-aligned to the width of the largest
    /// emitted number and followed by `"| "`. Bounds behave exactly as
    /// in [`Access::read_range`].
    ///
    /// # Errors
    /// Returns an error under the same conditions as
    /// [`Access::read_range`].
    pub fn read_range_numbered(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, VfsError> {
        self.with_line_range(path, start, end, |lines, first| {
            let width = (first + lines.len() - 1).to_string().len();
            lines
                .iter()
                .enumerate()
                .map(|(index, line)| format!("{:>width$}| {}", first + index, line))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// Creates or overwrites the file at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the write, when another
    /// live identity holds a claim on `path`, or when the backend fails.
    pub fn write(&self, path: &str, contents: &[u8]) -> Result<(), VfsError> {
        let path = self.gate(Op::Write, path, ClaimKind::Write)?;
        self.inner().write(&path, contents)
    }

    /// Appends to the file at `path`, creating it if absent.
    ///
    /// # Errors
    /// Returns an error when the policy denies the append, when another
    /// live identity holds a claim on `path`, or when the backend fails.
    pub fn append(&self, path: &str, contents: &[u8]) -> Result<(), VfsError> {
        let path = self.gate(Op::Append, path, ClaimKind::Write)?;
        self.inner().append(&path, contents)
    }

    /// Replaces the unique occurrence of `old` with `new` in the file at
    /// `path`. Zero matches and multiple matches are both errors.
    ///
    /// # Errors
    /// Returns an error when the policy denies the write, when another
    /// live identity holds a claim on `path`, when the match count is not
    /// exactly one, or when the backend fails.
    pub fn str_replace(&self, path: &str, old: &str, new: &str) -> Result<(), VfsError> {
        let path = self.gate(Op::Write, path, ClaimKind::Write)?;
        self.inner().str_replace(&path, old, new)
    }

    /// Removes the file, link, or directory at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the delete, when another
    /// live identity holds a claim on `path`, or when the backend fails.
    pub fn remove(&self, path: &str, recursive: bool) -> Result<(), VfsError> {
        let path = self.gate(Op::Delete, path, ClaimKind::Write)?;
        self.inner().remove(&path, recursive)
    }

    /// A confirmed absence is `Ok(false)`; a backend failure is `Err`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the check, when another
    /// live identity holds a write claim on `path`, or when the backend
    /// fails.
    pub fn exists(&self, path: &str) -> Result<bool, VfsError> {
        let path = self.gate(Op::Exists, path, ClaimKind::Read)?;
        self.inner().exists(&path)
    }

    /// Returns stored paths matching `pattern`, sorted.
    ///
    /// # Errors
    /// Returns an error when the policy denies the glob or when the
    /// backend fails.
    pub fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        // The claim key is the canonicalized pattern; the backend
        // receives the pattern verbatim.
        let _claimed = self.gate(Op::Glob, pattern, ClaimKind::Read)?;
        self.inner().glob(pattern)
    }

    /// Lists the directory at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the list, when another
    /// live identity holds a write claim on `path`, or when the backend
    /// fails.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, VfsError> {
        let path = self.gate(Op::List, path, ClaimKind::Read)?;
        self.inner().list(&path)
    }

    /// Returns metadata for `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the stat, when another
    /// live identity holds a write claim on `path`, or when the backend
    /// fails.
    pub fn stat(&self, path: &str) -> Result<Stat, VfsError> {
        let path = self.gate(Op::Stat, path, ClaimKind::Read)?;
        self.inner().stat(&path)
    }

    /// Creates the directory at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the mkdir, when another
    /// live identity holds a claim on `path`, or when the backend fails.
    pub fn mkdir(&self, path: &str, recursive: bool) -> Result<(), VfsError> {
        let path = self.gate(Op::Mkdir, path, ClaimKind::Write)?;
        self.inner().mkdir(&path, recursive)
    }

    /// Renames or moves, atomically where the backend allows. Both paths
    /// are claimed as writes.
    ///
    /// # Errors
    /// Returns an error when the policy denies the rename, when another
    /// live identity holds a claim on either path, or when the backend
    /// fails.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let from = self.gate(Op::Rename, from, ClaimKind::Write)?;
        let to = self.gate(Op::Rename, to, ClaimKind::Write)?;
        self.inner().rename(&from, &to)
    }

    /// Copies the file at `from` to `to`. The source is claimed as a
    /// read, the destination as a write.
    ///
    /// # Errors
    /// Returns an error when the policy denies the copy, when another
    /// live identity holds a conflicting claim on either path, or when
    /// the backend fails.
    pub fn copy(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let from = self.gate(Op::Copy, from, ClaimKind::Read)?;
        let to = self.gate(Op::Copy, to, ClaimKind::Write)?;
        self.inner().copy(&from, &to)
    }

    /// Searches files under the query's root.
    ///
    /// # Errors
    /// Returns an error when the policy denies the search, when another
    /// live identity holds a write claim on the query's root, or when the
    /// backend fails.
    pub fn grep(&self, query: &GrepQuery) -> Result<GrepResults, VfsError> {
        let root = canonicalize(query.root.as_str())?;
        self.check_policy(Op::Grep, root)?;
        self.volume.claims.claim(root, self.id, ClaimKind::Read)?;
        self.inner().grep(query)
    }

    /// Canonicalizes at receipt, consults the policy, then registers the
    /// claim - in that order, so a denied operation never registers a
    /// claim and every claim key is the canonical interned path.
    fn gate(&self, op: Op, path: &str, claim: ClaimKind) -> Result<VfsPath, VfsError> {
        let path = canonicalize(path)?;
        self.check_policy(op, path)?;
        self.volume.claims.claim(path, self.id, claim)?;
        Ok(path)
    }

    /// Consults the handle's policy. v1 maps `Ask` to `PermissionDenied`:
    /// the approval dialog is a host concern above this layer, and the
    /// reason string still names what was asked and which rule fired.
    fn check_policy(&self, op: Op, path: VfsPath) -> Result<(), VfsError> {
        match self.policy.check(op, &path) {
            Verdict::Allow => Ok(()),
            Verdict::Deny(reason) | Verdict::Ask(reason) => Err(VfsError::PermissionDenied(reason)),
        }
    }

    /// Reads the file and resolves one line range while its contents
    /// remain live.
    fn with_line_range(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
        render: impl FnOnce(&[&str], usize) -> String,
    ) -> Result<String, VfsError> {
        if start < 1 {
            return Err(VfsError::Backend(format!(
                "invalid line range for {path}: start {start} is below 1"
            )));
        }
        let contents = self.read_string(path)?;
        let lines: Vec<&str> = contents.lines().collect();
        if start > lines.len() {
            return Ok(String::new());
        }
        let end = end.unwrap_or(lines.len()).min(lines.len());
        if end < start {
            return Err(VfsError::Backend(format!(
                "invalid line range for {path}: end {end} is before start {start}"
            )));
        }
        Ok(render(&lines[start - 1..end], start))
    }

    /// Poison-safe lock on the backend's access object, held per call.
    fn inner(&self) -> MutexGuard<'_, Box<dyn VfsAccess>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Poison-safe lock on the backend.
    fn backend(&self) -> MutexGuard<'_, Box<dyn Vfs>> {
        self.volume
            .backend
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Access")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for Access {
    fn drop(&mut self) {
        self.volume.claims.release(self.id);
        // Releasing the identity at the backend is best-effort: the
        // claims are already gone, so a backend failure here cannot
        // leave a conflict behind.
        let _ = self.backend().release(self.id);
    }
}

/// A handle is itself a backend: mounting a base handle under a child
/// router - which is how [`VfsRef::overlay`] shares one claims table
/// across two views of the same storage - routes operations through the
/// base's policy and claims under the caller's identity.
impl Vfs for VfsRef {
    fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError> {
        Ok(Box::new(HandleAccess(self.acquire_with(id))))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        // The vended session's Drop releases the identity and its
        // claims; nothing is registered at this level.
        let _ = id;
        Ok(())
    }

    fn read_only(&self) -> bool {
        self.backend().read_only()
    }
}

/// The session vended by a mounted handle: forwards every operation
/// through the base handle's capability, so its policy and claims apply
/// under the caller's identity. Paths arrive canonical, so the
/// capability's canonicalization at receipt is an idempotent re-check.
///
/// Byte-range reads and the POSIX extras keep their trait defaults: the
/// public capability exposes neither, so there is nothing to forward to.
struct HandleAccess(Access);

impl VfsAccess for HandleAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.0.read(path.as_str())
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.0.write(path.as_str(), contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.0.append(path.as_str(), contents)
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.0.remove(path.as_str(), recursive)
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.0.exists(path.as_str())
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.0.glob(pattern)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.0.list(path.as_str())
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.0.stat(path.as_str())
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.0.mkdir(path.as_str(), recursive)
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.rename(from.as_str(), to.as_str())
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.0.copy(from.as_str(), to.as_str())
    }

    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str) -> Result<(), VfsError> {
        self.0.str_replace(path.as_str(), old, new)
    }

    fn grep(&self, query: &GrepQuery) -> Result<GrepResults, VfsError> {
        self.0.grep(query)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use super::{Access, VfsRef};
    use crate::error::VfsError;
    use crate::path::VfsPath;
    use crate::traits::{ExecId, Op, Policy, Verdict, Vfs, VfsAccess};
    use crate::types::{Entry, Stat};

    /// Minimal in-memory backend shared between the `Vfs` and the access
    /// objects it vends. Releases are recorded so tests can observe the
    /// identity lifecycle.
    #[derive(Clone, Default)]
    struct StubFs {
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        released: Arc<Mutex<Vec<ExecId>>>,
    }

    impl StubFs {
        fn seeded(files: &[(&str, &str)]) -> StubFs {
            let stub = StubFs::default();
            for (name, text) in files {
                stub.files()
                    .insert((*name).to_owned(), text.as_bytes().to_vec());
            }
            stub
        }

        fn files(&self) -> MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
            self.files.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn released(&self) -> Vec<ExecId> {
            self.released
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Vfs for StubFs {
        fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError> {
            let _ = id;
            Ok(Box::new(StubAccess {
                files: Arc::clone(&self.files),
            }))
        }

        fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
            self.released
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(id);
            Ok(())
        }
    }

    struct StubAccess {
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    }

    impl StubAccess {
        fn files(&self) -> MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
            self.files.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl VfsAccess for StubAccess {
        fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
            self.files()
                .get(path.as_str())
                .cloned()
                .ok_or_else(|| VfsError::NotFound(path.to_string()))
        }

        fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
            self.files().insert(path.to_string(), contents.to_vec());
            Ok(())
        }

        fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
            self.files()
                .entry(path.to_string())
                .or_default()
                .extend_from_slice(contents);
            Ok(())
        }

        fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
            let _ = recursive;
            self.files()
                .remove(path.as_str())
                .map(|_| ())
                .ok_or_else(|| VfsError::NotFound(path.to_string()))
        }

        fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
            Ok(self.files().contains_key(path.as_str()))
        }

        fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
            let prefix = pattern.split('*').next().unwrap_or(pattern);
            Ok(self
                .files()
                .keys()
                .filter(|name| name.starts_with(prefix))
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
                .files()
                .remove(from.as_str())
                .ok_or_else(|| VfsError::NotFound(from.to_string()))?;
            self.files().insert(to.to_string(), bytes);
            Ok(())
        }

        fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
            let bytes = self
                .files()
                .get(from.as_str())
                .cloned()
                .ok_or_else(|| VfsError::NotFound(from.to_string()))?;
            self.files().insert(to.to_string(), bytes);
            Ok(())
        }
    }

    fn handle(stub: &StubFs) -> VfsRef {
        VfsRef::new(stub.clone())
    }

    /// Extracts the Conflict message or fails the test.
    fn conflict_message(result: Result<(), VfsError>) -> String {
        match result {
            Err(VfsError::Conflict(message)) => message,
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    #[test]
    fn an_access_reads_and_writes_through_the_handle() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let access = vfs.acquire();
        access.write("/notes/a.txt", b"hello")?;
        assert_eq!(access.read("/notes/a.txt")?, b"hello");
        assert!(access.exists("/notes/a.txt")?);
        Ok(())
    }

    #[test]
    fn every_acquire_vends_a_process_unique_identity() {
        let vfs = handle(&StubFs::default());
        let first = vfs.acquire();
        let second = vfs.acquire();
        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn one_identity_never_conflicts_with_itself() -> Result<(), VfsError> {
        // Borrow semantics: a blocking call chain uses the parent's
        // access, so sequential ops on one path by one identity stay
        // legal - no new identity, no false conflict.
        let vfs = handle(&StubFs::default());
        let access = vfs.acquire();
        access.write("/f.txt", b"one")?;
        access.write("/f.txt", b"two")?;
        access.append("/f.txt", b"!")?;
        assert_eq!(access.read("/f.txt")?, b"two!");
        Ok(())
    }

    #[test]
    fn a_write_conflicts_with_another_identitys_read_claim() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "data")]));
        let reader = vfs.acquire();
        let writer = vfs.acquire();
        reader.read("/f.txt")?;
        let message = conflict_message(writer.write("/f.txt", b"new"));
        assert!(message.contains("/f.txt"), "names the path: {message}");
        assert!(
            message.contains("read"),
            "names the standing claim kind: {message}"
        );
        assert!(
            message.contains("write"),
            "names the attempted kind: {message}"
        );
        assert!(
            message.contains(&format!("{:?}", reader.id())),
            "names the claimant: {message}"
        );
        assert!(
            message.contains(&format!("{:?}", writer.id())),
            "names the attempter: {message}"
        );
        Ok(())
    }

    #[test]
    fn a_read_conflicts_with_another_identitys_write_claim() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let writer = vfs.acquire();
        writer.write("/f.txt", b"x")?;
        let reader = vfs.acquire();
        match reader.read("/f.txt") {
            Err(VfsError::Conflict(_)) => {}
            other => panic!("expected a conflict, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn two_writes_by_two_identities_conflict() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let first = vfs.acquire();
        first.write("/f.txt", b"1")?;
        let second = vfs.acquire();
        let message = conflict_message(second.write("/f.txt", b"2"));
        assert!(message.contains("write claim"), "{message}");
        Ok(())
    }

    #[test]
    fn reads_by_two_identities_never_conflict() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "data")]));
        let first = vfs.acquire();
        let second = vfs.acquire();
        first.read("/f.txt")?;
        assert_eq!(second.read("/f.txt")?, b"data");
        Ok(())
    }

    #[test]
    fn a_copy_conflicts_with_another_identitys_write_on_the_source() -> Result<(), VfsError> {
        // Copy claims the source as a read, and a read booms on another
        // live identity's write claim.
        let vfs = handle(&StubFs::default());
        let writer = vfs.acquire();
        writer.write("/src.txt", b"data")?;
        let copier = vfs.acquire();
        let message = conflict_message(copier.copy("/src.txt", "/dst.txt"));
        assert!(message.contains("/src.txt"), "names the source: {message}");
        Ok(())
    }

    #[test]
    fn a_copy_shares_the_source_with_another_identitys_read() -> Result<(), VfsError> {
        // The source claim is a read, not a write: another identity's
        // read claim on the source must not block the copy. Were the
        // source claimed as a write, this copy would conflict.
        let vfs = handle(&StubFs::seeded(&[("/src.txt", "data")]));
        let reader = vfs.acquire();
        reader.read("/src.txt")?;
        let copier = vfs.acquire();
        copier.copy("/src.txt", "/dst.txt")?;
        assert_eq!(copier.read("/dst.txt")?, b"data");
        Ok(())
    }

    #[test]
    fn a_copy_conflicts_with_another_identitys_claim_on_the_destination() -> Result<(), VfsError> {
        // The destination is claimed as a write, so any other live
        // identity's claim on it blocks the copy.
        let vfs = handle(&StubFs::seeded(&[
            ("/src.txt", "data"),
            ("/dst.txt", "old"),
        ]));
        let reader = vfs.acquire();
        reader.read("/dst.txt")?;
        let copier = vfs.acquire();
        let message = conflict_message(copier.copy("/src.txt", "/dst.txt"));
        assert!(
            message.contains("/dst.txt"),
            "names the destination: {message}"
        );
        Ok(())
    }

    #[test]
    fn a_rename_conflicts_with_a_claim_on_the_source_path() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/from.txt", "data")]));
        let reader = vfs.acquire();
        reader.read("/from.txt")?;
        let renamer = vfs.acquire();
        let message = conflict_message(renamer.rename("/from.txt", "/to.txt"));
        assert!(message.contains("/from.txt"), "names the source: {message}");
        Ok(())
    }

    #[test]
    fn a_rename_conflicts_with_a_claim_on_the_destination_path() -> Result<(), VfsError> {
        // Both paths are claimed as writes; were the second gate dropped,
        // this rename would sail through against the standing claim.
        let vfs = handle(&StubFs::seeded(&[
            ("/from.txt", "data"),
            ("/to.txt", "old"),
        ]));
        let reader = vfs.acquire();
        reader.read("/to.txt")?;
        let renamer = vfs.acquire();
        let message = conflict_message(renamer.rename("/from.txt", "/to.txt"));
        assert!(
            message.contains("/to.txt"),
            "names the destination: {message}"
        );
        Ok(())
    }

    #[test]
    fn dropping_an_access_releases_its_identity_and_claims() -> Result<(), VfsError> {
        let stub = StubFs::default();
        let vfs = handle(&stub);
        let first = vfs.acquire();
        let first_id = first.id();
        first.write("/f.txt", b"1")?;
        drop(first);
        assert!(stub.released().contains(&first_id));
        let second = vfs.acquire();
        second.write("/f.txt", b"2")?;
        assert_eq!(second.read("/f.txt")?, b"2");
        Ok(())
    }

    #[test]
    fn spawn_deletes_the_parents_claims() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire();
        parent.write("/f.txt", b"1")?;
        let child = parent.spawn();
        assert_ne!(parent.id(), child.id());
        // The parent's pre-spawn write claim is retired: the child can
        // touch the same path without a false conflict.
        child.write("/f.txt", b"2")?;
        assert_eq!(child.read("/f.txt")?, b"2");
        Ok(())
    }

    #[test]
    fn sequential_fanout_arms_stay_legal() -> Result<(), VfsError> {
        // The pattern the claims model teaches: the parent spawns each
        // arm in turn; a dropped arm releases its claims, so the next arm
        // can merge onto the same path.
        let vfs = handle(&StubFs::default());
        let parent = vfs.acquire();
        let arm_one = parent.spawn();
        arm_one.write("/evidence.md", b"one\n")?;
        drop(arm_one);
        let arm_two = parent.spawn();
        arm_two.append("/evidence.md", b"two\n")?;
        assert_eq!(arm_two.read("/evidence.md")?, b"one\ntwo\n");
        Ok(())
    }

    #[test]
    fn transfer_of_control_moves_the_claims_with_the_access() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "data")]));
        let original = vfs.acquire();
        original.read("/f.txt")?;
        // Transfer of control moves the access object; the identity and
        // its claims move with it.
        let moved = original;
        let other = vfs.acquire();
        let message = conflict_message(other.write("/f.txt", b"new"));
        assert!(message.contains(&format!("{:?}", moved.id())));
        assert_eq!(moved.read("/f.txt")?, b"data");
        Ok(())
    }

    #[test]
    fn alias_spellings_of_one_file_land_on_one_claim_key() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/a/b.txt", "x")]));
        let reader = vfs.acquire();
        reader.read("/a/./b.txt")?;
        let writer = vfs.acquire();
        let message = conflict_message(writer.write("/a//b.txt", b"y"));
        assert!(message.contains("/a/b.txt"), "the canonical key: {message}");
        Ok(())
    }

    #[test]
    fn claims_are_shared_across_handle_clones() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let clone = vfs.clone();
        let first = vfs.acquire();
        first.write("/f.txt", b"1")?;
        let second = clone.acquire();
        let message = conflict_message(second.write("/f.txt", b"2"));
        assert!(message.contains("/f.txt"), "{message}");
        Ok(())
    }

    #[test]
    fn a_denied_operation_never_registers_a_claim() -> Result<(), VfsError> {
        /// A policy whose verdict flips through shared state mid-run.
        struct FlipPolicy {
            verdict: Arc<Mutex<Verdict>>,
        }

        impl Policy for FlipPolicy {
            fn check(&self, op: Op, path: &VfsPath) -> Verdict {
                let _ = (op, path);
                self.verdict
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            }
        }

        let verdict = Arc::new(Mutex::new(Verdict::Deny("writes are sealed".to_owned())));
        let vfs = VfsRef::with_policy(
            StubFs::default(),
            FlipPolicy {
                verdict: Arc::clone(&verdict),
            },
        );
        let denied = vfs.acquire();
        match denied.write("/f.txt", b"x") {
            Err(VfsError::PermissionDenied(reason)) => {
                assert_eq!(reason, "writes are sealed");
            }
            other => panic!("expected a denial, got {other:?}"),
        }
        // The host flips the policy mid-run through shared state.
        *verdict.lock().unwrap_or_else(PoisonError::into_inner) = Verdict::Allow;
        let allowed = vfs.acquire();
        // Had the denied attempt registered a write claim, this write
        // would conflict with it.
        allowed.write("/f.txt", b"x")?;
        assert_eq!(allowed.read("/f.txt")?, b"x");
        Ok(())
    }

    #[test]
    fn read_range_slices_lines_one_based_and_inclusive() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "one\ntwo\nthree\n")]));
        let access = vfs.acquire();
        assert_eq!(access.read_range("/f.txt", 2, None)?, "two\nthree");
        assert_eq!(access.read_range("/f.txt", 2, Some(99))?, "two\nthree");
        assert_eq!(access.read_range("/f.txt", 99, None)?, "");
        assert_eq!(access.read_range("/f.txt", 1, Some(1))?, "one");
        Ok(())
    }

    #[test]
    fn read_range_rejects_invalid_bounds() {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "one\ntwo\n")]));
        let access = vfs.acquire();
        assert!(access.read_range("/f.txt", 0, None).is_err());
        assert!(access.read_range("/f.txt", 2, Some(1)).is_err());
    }

    #[test]
    fn read_range_numbered_numbers_absolutely_from_start() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::seeded(&[("/f.txt", "one\ntwo\nthree\n")]));
        let access = vfs.acquire();
        assert_eq!(
            access.read_range_numbered("/f.txt", 1, None)?,
            "1| one\n2| two\n3| three"
        );
        assert_eq!(
            access.read_range_numbered("/f.txt", 2, Some(3))?,
            "2| two\n3| three"
        );
        assert_eq!(access.read_range_numbered("/f.txt", 99, None)?, "");
        Ok(())
    }

    #[test]
    fn read_range_numbered_pads_to_the_widest_emitted_number() -> Result<(), VfsError> {
        let lines: Vec<String> = (1..=10).map(|n| format!("line{n}")).collect();
        let text = lines.join("\n");
        let vfs = handle(&StubFs::seeded(&[("/f.txt", &text)]));
        let access = vfs.acquire();
        assert_eq!(
            access.read_range_numbered("/f.txt", 9, Some(10))?,
            " 9| line9\n10| line10"
        );
        Ok(())
    }

    #[test]
    fn read_string_rejects_non_utf8() -> Result<(), VfsError> {
        let vfs = handle(&StubFs::default());
        let access = vfs.acquire();
        access.write("/bin.dat", &[0xff, 0xfe])?;
        match access.read_string("/bin.dat") {
            Err(VfsError::Backend(_)) => {}
            other => panic!("expected a UTF-8 failure, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn the_handle_and_capability_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<VfsRef>();
        assert_send_sync::<Access>();
    }
}
