//! Tests for the handle, the capability, and the claims ledger, split by
//! topic. The flipping policy and the conflict helper live here because
//! every topic module uses them.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use super::claims::PRUNE_AT;
use super::{Access, VfsRef};
use crate::error::VfsError;
use crate::observe::{OpEvent, Origin};
use crate::path::VfsPath;
use crate::test_support::{Call, TestFs, access, handle, test_origin};
use crate::traits::{AcquireContext, Op, Policy, Verdict};

mod claims;
mod happens_before;
mod operations;
mod refusals;
mod regions;
/// The rooted-path, idempotent-remove, and split-glob semantics of
/// the public capability, exercised over the memory backend.
mod semantics;
mod sink;

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
