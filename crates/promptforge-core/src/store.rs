//! Run-scoped virtual files, shared by Lua and the model.
//!
//! A prompt run keeps its bulk state in virtual files addressed by logical
//! string paths. The run's [`VfsRef`] handle carries the store mount; the
//! [`Store`] facade (behind the [`StoreExt`] extension trait's
//! `vfs.store(&access)` call shape) scopes logical paths onto it, and every
//! operation is attributed to the [`Access`] capability's identity, so a
//! conflicting operation by a second live identity surfaces as
//! [`StoreError::WriteRace`]. [`Store::read`] returns verbatim contents for
//! trusted handoff, [`Store::read_range`] slices a 1-based inclusive line
//! range out of the same verbatim contents, and
//! [`Store::read_range_numbered`] numbers such a slice absolutely (with no
//! bounds it numbers the whole file from 1). For model-facing re-injection
//! the caller wraps a verbatim read in an untrusted guard envelope (the
//! `untrusted` Lua global). Edits are anchor-based ([`Store::str_replace`])
//! rather than offset-based, the shape that works for a model.
//!
//! The implementation lives in the `promptforge-store` crate and is
//! re-exported here unchanged, so existing `promptforge_core::store::*`
//! paths keep working.

pub use promptforge_store::{PathReason, Store, StoreError, StoreErrorKind, StoreExt};
pub use shared_vfs::{Access, VfsRef};
