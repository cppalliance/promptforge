//! Test support: the configurable test backend, the handle and access
//! shortcuts, and the path, context, and origin helpers.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::VfsError;
use crate::handle::{Access, Scope, VfsRef};
use crate::memory::MemoryBackend;
use crate::observe::Origin;
use crate::path::{VfsPath, canonicalize_absolute};
use crate::stat::{Entry, Stat};
use crate::traits::{AcquireContext, ExecId, Op, Vfs, VfsAccess};

/// One call the test backend received, in arrival order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Call {
    Acquire(ExecId),
    Release(ExecId),
    /// An operation on one path or pattern. A two-path operation logs
    /// its source, then its destination.
    Op(Op, String),
}

type AcquireGate = dyn Fn(&AcquireContext) -> Result<(), VfsError> + Send + Sync;
type ReleaseGate = dyn Fn(ExecId) -> Result<(), VfsError> + Send + Sync;
type RemoveOverride = dyn Fn(&VfsPath, bool) -> Result<(), VfsError> + Send + Sync;
type CallLog = Arc<Mutex<Vec<Call>>>;

/// A backend whose storage is a [`MemoryBackend`], whose every call lands
/// in a shared log, and whose operations a closure can replace. Clones
/// share storage, log, and closures, so a test keeps one clone to inspect
/// after mounting another.
#[derive(Clone, Default)]
pub(crate) struct TestFs {
    memory: MemoryBackend,
    calls: CallLog,
    read_only: bool,
    on_acquire: Option<Arc<AcquireGate>>,
    on_release: Option<Arc<ReleaseGate>>,
    on_remove: Option<Arc<RemoveOverride>>,
}

impl TestFs {
    /// A backend holding `files`, written straight to storage, so the
    /// log starts empty.
    #[track_caller]
    pub(crate) fn seeded(files: &[(&str, &str)]) -> TestFs {
        let fs = TestFs::default();
        let mut storage = fs.storage();
        for (name, text) in files {
            if let Err(err) = path(name).and_then(|at| storage.write(&at, text.as_bytes())) {
                panic!("seeding {name}: {err}");
            }
        }
        fs
    }

    /// Reports the backend read-only, so the router refuses mutations.
    #[track_caller]
    pub(crate) fn with_read_only(mut self) -> TestFs {
        self.read_only = true;
        self
    }

    /// Runs `gate` before each acquire: an error refuses the session, and
    /// `Ok` opens one on storage.
    #[track_caller]
    pub(crate) fn on_acquire(
        mut self,
        gate: impl Fn(&AcquireContext) -> Result<(), VfsError> + Send + Sync + 'static,
    ) -> TestFs {
        self.on_acquire = Some(Arc::new(gate));
        self
    }

    /// Runs `gate` after each release reaches storage: an error is the
    /// release's failure.
    #[track_caller]
    pub(crate) fn on_release(
        mut self,
        gate: impl Fn(ExecId) -> Result<(), VfsError> + Send + Sync + 'static,
    ) -> TestFs {
        self.on_release = Some(Arc::new(gate));
        self
    }

    /// Answers every remove with `remove` in place of storage.
    #[track_caller]
    pub(crate) fn on_remove(
        mut self,
        remove: impl Fn(&VfsPath, bool) -> Result<(), VfsError> + Send + Sync + 'static,
    ) -> TestFs {
        self.on_remove = Some(Arc::new(remove));
        self
    }

    /// Every call received so far.
    #[track_caller]
    pub(crate) fn calls(&self) -> Vec<Call> {
        lock(&self.calls).clone()
    }

    /// Every stored file and its bytes, read straight from storage.
    #[track_caller]
    pub(crate) fn files(&self) -> BTreeMap<String, Vec<u8>> {
        let storage = self.storage();
        let names = storage.glob("/**").expect("the match-all pattern is valid");
        let mut files = BTreeMap::new();
        for name in names {
            match path(&name).and_then(|at| storage.read(&at)) {
                Ok(bytes) => {
                    files.insert(name, bytes);
                }
                // The match-all glob also returns directories.
                Err(VfsError::IsADirectory { .. }) => {}
                Err(err) => panic!("reading {name}: {err}"),
            }
        }
        files
    }

    /// A session straight on storage, past the log and the closures.
    #[track_caller]
    fn storage(&self) -> Box<dyn VfsAccess> {
        self.memory
            .clone()
            .acquire(&context())
            .expect("the memory backend always acquires")
    }
}

impl Vfs for TestFs {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        lock(&self.calls).push(Call::Acquire(cx.id()));
        if let Some(gate) = &self.on_acquire {
            gate(cx)?;
        }
        Ok(Box::new(TestAccess {
            storage: self.memory.acquire(cx)?,
            calls: Arc::clone(&self.calls),
            on_remove: self.on_remove.clone(),
        }))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        lock(&self.calls).push(Call::Release(id));
        self.memory.release(id)?;
        match &self.on_release {
            Some(gate) => gate(id),
            None => Ok(()),
        }
    }

    fn read_only(&self) -> bool {
        self.read_only
    }
}

/// One session with a [`TestFs`]. It implements only the required
/// methods, so every provided method runs its `VfsAccess` default.
struct TestAccess {
    storage: Box<dyn VfsAccess>,
    calls: CallLog,
    on_remove: Option<Arc<RemoveOverride>>,
}

impl TestAccess {
    fn log(&self, op: Op, target: &str) {
        lock(&self.calls).push(Call::Op(op, target.to_owned()));
    }
}

impl VfsAccess for TestAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.log(Op::Read, path.as_str());
        self.storage.read(path)
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.log(Op::Write, path.as_str());
        self.storage.write(path, contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.log(Op::Append, path.as_str());
        self.storage.append(path, contents)
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.log(Op::Delete, path.as_str());
        match &self.on_remove {
            Some(remove) => remove(path, recursive),
            None => self.storage.remove(path, recursive),
        }
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.log(Op::Exists, path.as_str());
        self.storage.exists(path)
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.log(Op::Glob, pattern);
        self.storage.glob(pattern)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.log(Op::List, path.as_str());
        self.storage.list(path)
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.log(Op::Stat, path.as_str());
        self.storage.stat(path)
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.log(Op::Mkdir, path.as_str());
        self.storage.mkdir(path, recursive)
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.log(Op::Rename, from.as_str());
        self.log(Op::Rename, to.as_str());
        self.storage.rename(from, to)
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.log(Op::Copy, from.as_str());
        self.log(Op::Copy, to.as_str());
        self.storage.copy(from, to)
    }
}

/// Poison-safe lock on a call log.
fn lock(calls: &Mutex<Vec<Call>>) -> MutexGuard<'_, Vec<Call>> {
    calls.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `s` canonicalized as an absolute virtual path.
#[track_caller]
pub(crate) fn path(s: &str) -> Result<VfsPath, VfsError> {
    canonicalize_absolute(s)
}

/// A fresh identity in a fresh scope, for acquiring a backend directly.
#[track_caller]
pub(crate) fn context() -> AcquireContext {
    AcquireContext::new(ExecId::vend(), Scope::start())
}

/// The origin for tests that ignore origins, positioned at the calling
/// test's line.
#[track_caller]
pub(crate) fn test_origin() -> Origin {
    Origin::new("vfs test")
}

/// A handle over `backend` with the store declared at `/`, the shape
/// `VfsRef::default` builds over memory.
#[track_caller]
pub(crate) fn handle(backend: impl Vfs + 'static) -> VfsRef {
    VfsRef::builder().store("/", backend).build()
}

/// Acquires `vfs` under the test origin.
#[track_caller]
pub(crate) fn access(vfs: &VfsRef) -> Result<Access, VfsError> {
    vfs.acquire(test_origin())
}
