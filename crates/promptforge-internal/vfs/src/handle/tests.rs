//! Tests for the handle, the capability, and the claims ledger, split by
//! topic. The stub backend, the flipping policy, and the conflict helper
//! live here because every topic module uses them.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::claims::PRUNE_AT;
use super::{Access, VfsRef};
use crate::error::VfsError;
use crate::observe::{OpEvent, Origin};
use crate::path::VfsPath;
use crate::stat::{Entry, FileType, Stat};
use crate::traits::{AcquireContext, ExecId, Op, Policy, Verdict, Vfs, VfsAccess};

mod claims;
mod happens_before;
mod operations;
mod refusals;
mod regions;
/// The rooted-path, idempotent-remove, and split-glob semantics of
/// the public capability, exercised over the memory backend.
mod semantics;
mod sink;

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
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        let _ = cx;
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
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })
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
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })
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
        Err(VfsError::Unsupported {
            path: path.to_string(),
            detail: "the stub does not list".into(),
        })
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        // Every stored key is a file; the stub holds no directories.
        let bytes = self
            .files()
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })?;
        Ok(Stat {
            file_type: FileType::File,
            size: bytes.len() as u64,
            mode: None,
            modified: None,
            created: None,
        })
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let _ = (path, recursive);
        Ok(())
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let bytes = self
            .files()
            .remove(from.as_str())
            .ok_or_else(|| VfsError::NotFound {
                path: from.to_string(),
            })?;
        self.files().insert(to.to_string(), bytes);
        Ok(())
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let bytes = self
            .files()
            .get(from.as_str())
            .cloned()
            .ok_or_else(|| VfsError::NotFound {
                path: from.to_string(),
            })?;
        self.files().insert(to.to_string(), bytes);
        Ok(())
    }
}

fn handle(stub: &StubFs) -> VfsRef {
    VfsRef::new(stub.clone())
}

/// The claims tests ignore origins, so they acquire under one blanket
/// label; the observability tests use specific labels.
fn test_origin() -> Origin {
    Origin::new("handle test")
}

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

/// Extracts the Conflict message or fails the test.
fn conflict_message<T: std::fmt::Debug>(result: Result<T, VfsError>) -> String {
    match result {
        Err(VfsError::Conflict { detail, .. }) => detail,
        other => panic!("expected a conflict, got {other:?}"),
    }
}
