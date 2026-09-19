//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; an always-on `store` table gives the block the run's virtual
//! files; and an every-Nth-instruction hook polls the run's cancel flag, so
//! even an unbounded loop aborts promptly once the host cancels.
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
//! [`Store`] facade over the run's VFS access capability, threaded in from
//! the executor, so every section
//! in a run shares one set of virtual files even though contexts clear on each
//! transition. A failed store op raises a Lua error, which surfaces from
//! `SectionVm::run_chunk` as [`Error::Lua`].
//!
//! Most of this crate is a `#[doc(hidden)]` cross-crate seam for
//! `promptforge-api-runtime`'s executor, which drives the VM and the coroutine
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
    StdLib, Thread, UserData, UserDataMethods, Value, VmState,
};
pub(crate) use serde_json::Value as Json;

pub(crate) use promptforge_api_types::observe::{Observation, Observer, detail};
pub(crate) use promptforge_api_types::tools::{Tool, ToolId};
pub(crate) use promptforge_api_types::untrusted::GuardNonce;
pub(crate) use promptforge_model_client::model::{ModelBinding, ModelSet, ModelView};
pub(crate) use promptforge_store::{Access, Store};

pub(crate) use crate::compactors::install_compactors;
pub(crate) use crate::error::Result;
pub(crate) use crate::messages::install_messages;
pub(crate) use crate::models::install_models;
pub(crate) use crate::models::{LuaModelHandle, ModelsInferHook};

#[doc(hidden)]
pub use crate::error::{Error, SharedSource};

/// How many instructions between hook firings.
pub(crate) const HOOK_INTERVAL: u32 = 10_000;
/// Hook-firing trip budget, effectively unlimited.
///
/// Long-running and infinite loops are legal: no instruction ceiling aborts a
/// block, so the hook's job is the cancellation poll and the run's
/// `CancelHandle` is the kill switch for a runaway loop. The typed quota
/// errors remain for the memory and log budgets.
pub(crate) const HOOK_BUDGET: u64 = u64::MAX;
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

mod alias;
mod argv;
mod collection;
mod compactors;
mod error;
#[path = "error-value.rs"]
mod error_value;
#[doc(hidden)]
pub use error_value::{ErrorKind, ErrorValue, Raised, error_table};
mod hardening;
pub(crate) use hardening::{InstructionBudget, harden, install_instruction_budget, scalar_return};
mod coro;
pub(crate) use coro::{block_guard, install_shim_prelude, take_failure};
mod dispatch;
mod sys;
pub(crate) use sys::{guarded_var, seal_sys, var_snapshot_table, var_to_json};
mod host;
#[doc(hidden)]
pub use host::install_ui;
pub(crate) use host::{install_log, install_store_table, install_untrusted};
mod tools;
pub(crate) use tools::{LuaToolHandle, install_tool_call_counts, install_tools};
mod handles;
mod messages;
mod program;
mod projection;
mod prose;
mod scope;
mod vm;
pub(crate) use handles::resolve_section_target;
mod models;
mod protocol;
mod runtime_events;

// The executor-facing surface: every item `promptforge-api-runtime` names crosses
// here. These are `#[doc(hidden)]` cross-crate seams, not host API;
// `LuaProgram` is the documented exception.
#[doc(hidden)]
pub use crate::argv::Argv;
#[doc(hidden)]
pub use collection::render_item;
#[doc(hidden)]
pub use compactors::{Compactor, OverflowReason, is_context_overflow, precheck};
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub use coro::install_model_tool_call_shim;
#[doc(hidden)]
pub use coro::{
    install_agent_chat_shim, install_section_loop_shim, install_section_user_input_shim,
    install_store_shims,
};
#[doc(hidden)]
pub use dispatch::{
    ModelReport, ScriptReport, ToolDispatch, prepare_dispatch, prepare_model_dispatch,
};
#[doc(hidden)]
pub use handles::{LuaBlockResult, ToolBinding, ToolOutputKind, ToolSet, ToolView};
#[doc(hidden)]
pub use host::run_store_op;
#[doc(hidden)]
pub use models::ModelRuntime;
#[doc(hidden)]
pub use projection::project_messages;
#[doc(hidden)]
pub use prose::ProseState;
#[doc(hidden)]
pub use protocol::{
    Answer, ChatResult, ContentPart, MessageContent, MessageRecord, MessageRole, Request, StoreOp,
    StoreOutcome, TaskDelivery, TaskStatus, ToolCallOutcome, ToolCallRecord, UserInputOutcome,
    YieldParse,
};
#[doc(hidden)]
pub use runtime_events::{EventsSnapshot, install_runtime_events};
#[doc(hidden)]
pub use scope::{TaskAllowlist, ToolCallCounts, ToolRuntime};
#[doc(hidden)]
pub use sys::enrich_sys_model;
#[doc(hidden)]
pub use vm::{CoroStep, SectionVm, current_tool_bindings, resolve_model_binding};

pub use program::LuaProgram;

#[cfg(test)]
mod tests;
