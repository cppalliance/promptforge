//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; an always-on `store` table gives the block the run's virtual
//! files; and an instruction-count hook aborts a runaway block.
//! Direct `print` and `warn` are unavailable. During each executable Lua phase,
//! a borrowed `log(message)` callback accepts one bounded, single-line UTF-8
//! string and reports it through the run's [`Observer`] as `Lua: <message>`.
//! The callback expires at the end of that phase and is never retained across
//! a model await.
//!
//! The chunk's top-level return value becomes the section's result (the finish
//! case of the exit rule). The `var` table is read back afterward as JSON for
//! prose substitution.
//!
//! The `store` table is a deterministic host capability (like `var`), always
//! present and independent of tool scoping. Its methods are backed by the
//! run-scoped [`StoreRef`] handle threaded in from the executor, so every section
//! in a run shares one set of virtual files even though contexts clear on each
//! transition. A failed store op raises a Lua error, which surfaces from
//! [`run_chunk`] as [`Error::Lua`].

// These imports are re-exported `pub(crate)` so the `lua` child modules can pull
// the full shared surface with a single `use super::*;`. The `lua` module itself
// is `pub(crate)`, so none of these re-exports widen the crate's public API.
pub(crate) use std::collections::BTreeMap;
pub(crate) use std::num::NonZeroU32;
pub(crate) use std::sync::Arc;
pub(crate) use std::sync::Mutex;
pub(crate) use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub(crate) use mlua::{
    Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, MetaMethod, MultiValue, Scope, StdLib,
    UserData, UserDataFields, UserDataMethods, Value, Variadic, VmState,
};
pub(crate) use serde_json::Value as Json;
pub(crate) use serde_json::json;

pub(crate) use crate::lua_models::{LuaModelHandle, ModelInferHook};
pub(crate) use crate::lua_models::{
    ModelBindingState, ModelRuntime, close_model_scope, install_h2_models, install_live_models,
};
pub(crate) use crate::model::{ModelBinding, ModelBindings, ModelResolver};
pub(crate) use crate::observe::{Observation, Observer, detail};
pub(crate) use crate::resolve::RuntimeResolution;
pub(crate) use crate::store::StoreRef;
pub(crate) use crate::tools::{ToolId, ToolRegistry};
pub(crate) use crate::{Error, Result};

/// How many instructions between hook firings.
const HOOK_INTERVAL: u32 = 10_000;
/// Maximum number of hook firings before a block is aborted (~1e7 instructions).
const HOOK_BUDGET: u64 = 1_000;
/// Maximum number of Unicode scalar values accepted by `log`.
const LUA_LOG_CHARACTER_LIMIT: usize = 256;
/// Default per-VM Lua heap ceiling, matching [`crate::execute::RunLimits`].
const DEFAULT_LUA_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Default per-VM `log()` event budget, matching [`crate::execute::RunLimits`].
const DEFAULT_LUA_LOG_EVENTS: u32 = 1024;

/// Cumulative `log()` byte ceiling derived from the event budget.
///
/// Bounds total log volume (bytes) even when each event is under the per-event
/// character ceiling. Derived as `events * LUA_LOG_CHARACTER_LIMIT` so it scales
/// with the configured event budget.
fn default_log_byte_budget(log_events: u32) -> usize {
    (log_events as usize).saturating_mul(LUA_LOG_CHARACTER_LIMIT)
}

mod hardening;
pub(crate) use hardening::*;
mod sys;
pub(crate) use sys::*;
mod host;
pub(crate) use host::*;
mod tools_bridge;
pub(crate) use tools_bridge::*;
mod vm;
pub(crate) use vm::*;
mod live;
pub(crate) use live::*;
mod program;
// `pub use` (not `pub(crate) use`) so the publicly re-exported `LuaProgram`
// keeps its `pub` visibility for the `crate::LuaProgram` root re-export; the
// glob preserves each other item's `pub(crate)` visibility unchanged.
pub use program::*;
mod scope;
pub(crate) use scope::*;
mod handles;
pub(crate) use handles::*;
