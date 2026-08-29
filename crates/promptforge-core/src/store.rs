//! Run-scoped virtual files, shared by Lua and the model.
//!
//! A prompt run keeps its bulk state in virtual files addressed by logical
//! string paths. [`Store`] is the backend contract, [`MemStore`] is an
//! in-memory backend, and [`StoreRef`] is the cheaply cloneable, thread-safe
//! handle the runtime hands to both the Lua VM and (later) the model's file
//! tools. [`StoreRef::read`] returns verbatim contents for trusted handoff,
//! [`StoreRef::read_range`] slices a 1-based inclusive line range out of the
//! same verbatim contents, and [`StoreRef::read_range_numbered`] numbers such
//! a slice absolutely (with no bounds it numbers the whole file from 1). For
//! model-facing re-injection the caller wraps a verbatim read in an
//! untrusted guard envelope (the `untrusted` Lua global).
//! Edits are anchor-based ([`Store::str_replace`]) rather than offset-based,
//! the shape that works for a model.
//!
//! The implementation lives in the `promptforge-store` crate and is
//! re-exported here unchanged, so existing `promptforge_core::store::*` paths
//! keep working.

pub use promptforge_store::{
    FileStore, MemStore, PathReason, Store, StoreError, StoreErrorKind, StoreRef,
};

pub(crate) use promptforge_store::WriteScope;
