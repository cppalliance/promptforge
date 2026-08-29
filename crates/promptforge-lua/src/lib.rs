//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; an always-on `store` table gives the block the run's virtual
//! files; and an instruction-count hook aborts a runaway block.
//! Direct `print` and `warn` are unavailable. A persistent `log(message)`
//! callback accepts one bounded, single-line UTF-8 string and reports it
//! through the run's [`Observer`] as `Lua: <message>`.
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
//! `SectionVm::run_chunk` as [`Error::Lua`].
//!
//! Most of this crate is a `#[doc(hidden)]` cross-crate seam for
//! `promptforge-core`'s executor, which drives the VM and the coroutine
//! protocol; [`LuaProgram`] is the documented exception.

// These imports are re-exported `pub(crate)` so the child modules can pull
// the full shared surface with a single `use super::*;`.
pub(crate) use std::collections::BTreeMap;
pub(crate) use std::num::NonZeroU32;
pub(crate) use std::sync::Arc;
pub(crate) use std::sync::Mutex;
pub(crate) use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub(crate) use mlua::thread::ThreadStatus;
pub(crate) use mlua::{
    Function, HookTriggers, IntoLuaMulti, Lua, LuaOptions, LuaSerdeExt, MetaMethod, MultiValue,
    StdLib, Thread, UserData, UserDataFields, UserDataMethods, Value, Variadic, VmState,
};
pub(crate) use serde_json::Value as Json;
pub(crate) use serde_json::json;

pub(crate) use promptforge_core_support::observe::{Observation, Observer, detail};
pub(crate) use promptforge_core_support::untrusted::GuardNonce;
pub(crate) use promptforge_gateway_client::model::{
    ModelBinding, ModelResolver, ModelSet, ModelView,
};
pub(crate) use promptforge_store::{StoreRef, WriteScope};
pub(crate) use promptforge_tools::{Tool, ToolCatalog, ToolId};

pub(crate) use crate::error::Result;
pub(crate) use crate::models::{LuaModelHandle, ModelInferHook, ModelsInferHook};
pub(crate) use crate::models::{install_h2_models, install_live_models};

#[doc(hidden)]
pub use crate::error::{Error, SharedSource};

/// How many instructions between hook firings.
pub(crate) const HOOK_INTERVAL: u32 = 10_000;
/// Maximum number of hook firings before a block is aborted (~1e7 instructions).
pub(crate) const HOOK_BUDGET: u64 = 1_000;
/// Maximum number of Unicode scalar values accepted by `log`.
pub(crate) const LUA_LOG_CHARACTER_LIMIT: usize = 256;
/// Default per-VM Lua heap ceiling, matching the executor's `RunLimits`.
pub(crate) const DEFAULT_LUA_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Default per-VM `log()` event budget, matching the executor's `RunLimits`.
pub(crate) const DEFAULT_LUA_LOG_EVENTS: u32 = 1024;

/// Cumulative `log()` byte ceiling derived from the event budget.
///
/// Bounds total log volume (bytes) even when each event is under the per-event
/// character ceiling. Derived as `events * LUA_LOG_CHARACTER_LIMIT` so it scales
/// with the configured event budget.
pub(crate) fn log_byte_budget(log_events: u32) -> usize {
    (log_events as usize).saturating_mul(LUA_LOG_CHARACTER_LIMIT)
}

mod collection;
mod error;
mod hardening;
pub(crate) use hardening::{InstructionBudget, harden, install_instruction_budget, scalar_return};
mod coro;
pub(crate) use coro::{install_shim_prelude, wrap_shimmed_handle};
mod sys;
pub(crate) use sys::{guarded_var, seal_sys, var_snapshot_table, var_to_json};
mod host;
pub(crate) use host::{install_log, install_store_table, install_untrusted};
mod tools_bridge;
pub(crate) use tools_bridge::{install_h2_tools, install_lua_tool_calls};
mod vm;
pub(crate) use vm::{LocalTools, pack_sequence};
#[cfg(test)]
pub(crate) use vm::{LuaOutcome, run_chunk};
mod live;
pub(crate) use live::validate_alias;
mod handles;
mod program;
mod scope;
pub(crate) use handles::{LuaToolHandle, resolve_section_target};
mod models;
mod protocol;

// The executor-facing surface: every item `promptforge-core` names crosses
// here. These are `#[doc(hidden)]` cross-crate seams, not host API;
// `LuaProgram` is the documented exception.
#[doc(hidden)]
pub use coro::{install_live_h1_shim_base, shim_live_h1_models};
#[doc(hidden)]
pub use handles::{
    Conflict, LuaBlockResult, LuaFanoutResult, ToolBinding, ToolResolver, ToolSet, ToolView,
};
#[doc(hidden)]
pub use live::LiveBindingProducer;
#[doc(hidden)]
pub use models::ModelRuntime;
#[doc(hidden)]
pub use protocol::{Answer, Request, YieldParse};
#[doc(hidden)]
pub use scope::{ToolCallCounts, ToolRuntime};
#[doc(hidden)]
pub use sys::{enrich_sys_model, enrich_sys_reply_finish_reason};
#[doc(hidden)]
pub use vm::{CoroStep, SectionVm, current_tool_bindings, resolve_model_binding};

pub use program::LuaProgram;

#[cfg(test)]
mod tests;
