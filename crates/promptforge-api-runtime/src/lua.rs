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
//! here unchanged, so existing `promptforge_api_runtime::lua::*` paths keep working.

#[cfg(test)]
pub(crate) use promptforge_lua::ToolOutputKind;
#[cfg(any(test, feature = "test-support"))]
pub(crate) use promptforge_lua::run_store_op;
pub(crate) use promptforge_lua::{
    Argv, CoroStep, LuaBlockResult, LuaProgram, MessageRecord, ModelReport, OverflowReason,
    ProseState, ScriptReport, SectionVm, TaskAllowlist, ToolBinding, ToolCallCounts, ToolSet,
    ToolView, UserInputOutcome, current_tool_bindings, enrich_sys_model, install_section_loop_shim,
    install_section_user_input_shim, install_store_shims, install_ui, is_context_overflow,
    precheck, prepare_dispatch, prepare_model_dispatch, project_messages, render_item,
    resolve_model_binding,
};

#[cfg(test)]
mod tests;
