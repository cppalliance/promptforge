//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; an always-on `store` table gives the block the run's virtual
//! files; and an every-Nth-instruction hook polls the run's cancel flag, so
//! even an unbounded loop aborts promptly once the run is cancelled.
//!
//! The implementation sits in the `runvil-lua` crate; this module is
//! the crate-internal import surface for it.

// The store operation behind `execute::perform_vfs_op`, the entry point
// a caller answers a `Vfs` effect through, and the
// model-facing message renderers a store failure carries.
pub(crate) use runvil_lua::{
    Argv, CoroStep, Library, LibraryBlock, LibraryKey, LuaBlockResult, MessageList, ModelReport,
    NonEmpty, OverflowReason, ProseState, QuotaResource, ScriptReport, SectionText, SectionVm,
    TaskAllowlist, ToolBinding, ToolCallCounts, ToolSet, Uninjected, UsageAnchor,
    current_tool_bindings, enrich_sys_model, install_preludes, install_section_loop_shim,
    install_store_shims, install_ui, output_reserve, precheck, prepare_dispatch,
    prepare_model_dispatch, project_messages, render_item, resolve_model_binding,
};
pub(crate) use runvil_lua::{run_store_op, store_error_message, store_probe_message};

#[cfg(test)]
mod tests;
