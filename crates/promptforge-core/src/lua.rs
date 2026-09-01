//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; an always-on `store` table gives the block the run's virtual
//! files; and an every-Nth-instruction hook polls the run's cancel flag, so
//! even an unbounded loop aborts promptly once the host cancels.
//!
//! The implementation lives in the `promptforge-lua` crate and is re-exported
//! here unchanged, so existing `promptforge_core::lua::*` paths keep working.

pub(crate) use promptforge_lua::{
    CoroStep, LiveBindingProducer, LuaBlockResult, LuaFanoutResult, LuaProgram, ScriptReport,
    SectionVm, ToolBinding, ToolCallCounts, ToolResolver, ToolSet, ToolView, current_tool_bindings,
    dispatch_tool, enrich_sys_model, enrich_sys_reply_finish_reason, install_live_h1_shim_base,
    resolve_model_binding, shim_live_h1_models,
};

#[cfg(test)]
pub(crate) use promptforge_lua::ToolOutputKind;

#[cfg(test)]
pub(crate) use promptforge_lua::{Conflict, ToolRuntime};

#[cfg(test)]
mod coro_tests;
