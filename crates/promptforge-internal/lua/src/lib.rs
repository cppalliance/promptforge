//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; an always-on `store` table gives the block the run's virtual
//! files; and an every-Nth-instruction hook polls the run's cancel flag, so
//! even an unbounded loop aborts promptly once the Host cancels.
//! Direct `print` and `warn` are unavailable. A persistent `log(message)`
//! callback accepts one bounded, single-line UTF-8 string and reports it
//! through the run's emitter as an `Event::Lua` checkpoint.
//!
//! The chunk's top-level return value becomes the section's result (the finish
//! case of the exit rule). The `var` table is read back afterward as JSON for
//! prose substitution.
//!
//! The `store` table is a deterministic Engine global (like `var`), always
//! present and independent of tool scoping. Its methods are backed by the
//! store view derived from the run's VFS access capability, threaded in
//! from the executor, so every section
//! in a run shares one set of virtual files even though contexts clear on each
//! transition. A failed store op raises a structured error value of kind
//! `store` carrying its `reason` and fields, which surfaces from
//! `SectionVm::run_chunk` as a Lua-category error.
//!
//! In scheduler mode the VM also holds `tasks`, the shims for spawning,
//! waiting on, checking, noting, and cancelling tasks. Capability preludes
//! are the Lua source an activated capability contributes: each runs once
//! per VM in an environment of its own before the shared library replays,
//! and its globals are checked against the reserved-name list and raw-set
//! into `_G`. The `input` table that `promptforge/user-input` defines is
//! one: `input.ask()` is an ordinary tool call to that capability's ask
//! tool.
//!
//! Most of this crate's public items exist for `promptforge-engine`'s
//! executor, which drives the VM and the coroutine protocol; the facade
//! re-exports only the store protocol ([`VfsOp`], [`VfsOutcome`]).
//!
//! ## Invariants
//!
//! - May depend on: `promptforge-types`, `promptforge-model-client`, and
//!   `promptforge-vfs`. `cargo test -p build-xtask` enforces the product and
//!   container boundaries.
//! - Scheduler state changes only through a typed `Request` yielded by the
//!   installed shim; a direct or malformed yield fails the block without
//!   changing scheduler state.
//! - Engine globals are namespace functions over plain values (`models.*`,
//!   `tools.*`, `store.*`). Handles are frozen, inspectable, and
//!   methodless, and an optional leading handle argument selects an
//!   explicit binding. The chainable `messages.new()` builders are the one
//!   deliberate exception.

// The shared surface that child modules pull with a single `use super::*;`.
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use mlua::thread::ThreadStatus;
use mlua::{
    Function, HookTriggers, IntoLuaMulti, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Thread,
    Value, VmState,
};
use serde_json::Value as Json;

use promptforge_model_client::model::{ModelBinding, ModelSet, ModelView};
use promptforge_types::emitter::Emitter;
use promptforge_types::event::lifecycle;
use promptforge_types::tools::ToolId;
use promptforge_types::untrusted::GuardNonce;
use promptforge_vfs::Access;

use crate::compactors::install_compactors;
use crate::error::Result;
use crate::messages::install_messages;
use crate::models::LuaModelHandle;
use crate::models::install_models;

pub use crate::error::{Error, SharedSource};

/// How many instructions between hook firings.
const HOOK_INTERVAL: u32 = 10_000;
/// Hook-firing trip budget, effectively unlimited.
///
/// Long-running and infinite loops are legal: no instruction ceiling aborts a
/// block, so the hook's job is the cancellation poll and the run's
/// `CancelHandle` is the kill switch for a runaway loop. The typed quota
/// errors remain for the memory and log budgets.
const HOOK_BUDGET: u64 = u64::MAX;
/// Maximum number of Unicode scalar values accepted by `log`.
const LUA_LOG_CHARACTER_LIMIT: usize = 256;
/// Default per-VM Lua heap ceiling, matching the executor's `RunLimits`.
const DEFAULT_LUA_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Default per-VM `log()` event budget, matching the executor's `RunLimits`.
const DEFAULT_LUA_LOG_EVENTS: u32 = 1024;

/// Cumulative `log()` byte ceiling derived from the event budget.
///
/// Bounds total log volume (bytes) even when each event is under the per-event
/// character ceiling. Derived as `events * LUA_LOG_CHARACTER_LIMIT` so it scales
/// with the configured event budget.
fn log_byte_budget(log_events: u32) -> usize {
    (log_events as usize).saturating_mul(LUA_LOG_CHARACTER_LIMIT)
}

mod alias;
mod argv;
mod collection;
mod compactors;
pub mod detail;
mod error;
#[path = "error-value.rs"]
mod error_value;
pub use error_value::{
    ErrorField, ErrorKind, ErrorValue, Raised, error_table, store_error_fields, store_error_reason,
    store_error_value_fields,
};
mod globals;
mod hardening;
use hardening::{InstructionBudget, harden, install_instruction_budget, scalar_return};
mod coro;
mod iteration;
use coro::{block_guard, install_shim_prelude, take_failure};
use iteration::install_deterministic_iteration;
mod dispatch;
mod sys;
use sys::{guarded_var, seal_sys, var_snapshot_table, var_to_json};
mod engine_globals;
pub use engine_globals::install_ui;
use engine_globals::{install_log, install_store_table, install_untrusted, route_store_to_shims};
mod tools;
use tools::{LuaToolHandle, install_tool_call_counts, install_tools};
mod handles;
mod messages;
mod prelude;
mod program;
mod projection;
mod prose;
mod proxy;
mod scope;
mod vm;
use handles::resolve_section_target;
mod models;
mod protocol;

// The executor-facing surface: every item `promptforge-engine` names crosses
// here. The facade re-exports only `VfsOp` and `VfsOutcome`.
pub use crate::argv::Argv;
pub use collection::render_item;
pub use compactors::{Compactor, OverflowReason, UsageAnchor, output_reserve, precheck};
#[cfg(feature = "test-support")]
pub use coro::install_model_tool_call_shim;
pub use coro::{install_section_loop_shim, install_store_shims};
pub use dispatch::{
    ModelReport, ScriptReport, ToolDispatch, prepare_dispatch, prepare_model_dispatch,
};
pub use engine_globals::{run_store_op, store_error_message};
pub use globals::{RESERVED_NAMES, Reserved, reserved_name};
pub use handles::{LuaBlockResult, ToolBinding, ToolOutputKind, ToolSet, ToolView};
pub use models::ModelRuntime;
pub use prelude::install_preludes;
pub use projection::project_messages;
pub use prose::ProseState;
pub use protocol::{
    Answer, ChatResult, ContentPart, LocalToolOutcome, MessageContent, MessageRecord, MessageRole,
    Request, TaskDelivery, TaskStatus, ToolCallOutcome, ToolCallRecord, VfsOp, VfsOutcome,
    YieldParse,
};
pub use scope::{TaskAllowlist, ToolCallCounts, ToolRuntime};
pub use sys::enrich_sys_model;
pub use vm::{CoroStep, SectionVm, current_tool_bindings, resolve_model_binding};
#[cfg(feature = "test-support")]
pub use vm::{reset_section_vm_peak, section_vm_peak};

pub use program::LuaProgram;

#[cfg(test)]
mod tests;
