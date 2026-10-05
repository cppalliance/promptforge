//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; an always-on `store` table gives the block the run's virtual
//! files; and an every-Nth-instruction hook polls the run's cancel flag, so
//! even an unbounded loop aborts promptly once the Host cancels.
//!
//! The implementation sits in the `promptforge-lua` crate; this module is
//! the crate-internal import surface for it.

// The store operation behind `execute::perform_vfs_op`, the entry point
// the Harness's effect loop answers a `Vfs` effect through, and the
// model-facing message renderer a store failure carries.
pub(crate) use promptforge_lua::{
    Argv, CoroStep, LuaBlockResult, LuaProgram, MessageRecord, ModelReport, OverflowReason,
    ProseState, ScriptReport, SectionVm, TaskAllowlist, ToolBinding, ToolCallCounts, ToolSet,
    ToolView, UsageAnchor, current_tool_bindings, enrich_sys_model, install_preludes,
    install_section_loop_shim, install_store_shims, install_ui, output_reserve, precheck,
    prepare_dispatch, prepare_model_dispatch, project_messages, render_item, resolve_model_binding,
};
pub(crate) use promptforge_lua::{run_store_op, store_error_message};

#[cfg(test)]
mod tests;
