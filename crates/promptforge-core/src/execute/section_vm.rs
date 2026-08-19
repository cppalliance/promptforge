//! The engine's setup half: one section VM's lifecycle from host injection
//! through the captured alias bindings.
//!
//! Every driver of the shared engine - the walk's `run_one_section` and the
//! fanout arm - runs the identical setup sequence: inject the host values,
//! install the persistent host APIs, install the control globals, replay the
//! shared library as the section's first chunk, then install the captured
//! alias bindings (so a declared alias wins over a same-named shared global).
//! Only the deltas live at the call site: the driver builds its own `sys`
//! JSON (the walk's chain-position `id`; the arm's parent `id` plus
//! `taskid`), picks the [`VmSeed`] (`initial_var` for the walk, the
//! collection `item` for the arm), and supplies its own three control-global
//! callbacks.
//!
//! VM construction and the Lua limits install stay with the driver. A
//! construction failure's handling differs (the walk propagates, the arm
//! finishes its finalizer), the arm's VM must outlive the cancel-scoped body
//! so the epilogue can tear it down, and a limits failure must keep
//! propagating as a bare `?` - routing it through teardown would emit
//! `LUA_TEARDOWN_STARTED`/`SUCCEEDED` events the walk never emitted. For the
//! same reason a failed setup returns the VM untorn-down: each driver owns
//! its teardown boundary (the walk tears down inline, the arm's epilogue
//! tears down once).

use std::sync::Arc;

use mlua::Value as LuaValue;

use crate::lua::{LuaFanoutResult, LuaProgram, LuaSectionHandle, SectionVm};
use crate::observe::Observer;
use crate::store::StoreRef;
use crate::{Error, Result};

/// What a section VM is seeded with beyond the shared host contract.
#[derive(Clone, Copy, Debug)]
pub(crate) enum VmSeed<'a> {
    /// A walked section: `var` carried in from an earlier VM (the top-level
    /// H1 hand-off); `None` for a contained chain.
    InitialVar(Option<&'a serde_json::Value>),
    /// A fanout arm: the collection member, installed as the `item` global
    /// after the host APIs so [`SectionVm::replay_shared`] - whose top-level
    /// code may read `item` - sees it.
    Item(&'a serde_json::Value),
}

/// The borrowed inputs one section VM setup shares.
///
/// Bundled so the walk and the fanout arm each thread one linear set of
/// borrows into [`setup_section_vm`] rather than a dozen parameters.
pub(crate) struct SectionVmSetup<'a> {
    /// The run's argument string, installed as the `args` global.
    pub(crate) args: &'a str,
    /// The `sys` JSON the driver built for this section or arm.
    pub(crate) sys: &'a serde_json::Value,
    /// The run-scoped store backing the Lua `store` table.
    pub(crate) store: &'a StoreRef,
    /// The model reply visible to this section's first prose.
    pub(crate) last_reply: Option<&'a str>,
    /// The driver-specific seed: `initial_var` for the walk, `item` for an
    /// arm.
    pub(crate) seed: VmSeed<'a>,
    /// The observer `Arc`: the persistent host APIs (`log`, `store`) capture
    /// it, and the shared-library replay reports through it.
    pub(crate) observer_arc: &'a Arc<dyn Observer>,
    /// The section name used in observations and error messages.
    pub(crate) section_name: &'a str,
    /// The run's section handles backing the `tasks` table; empty for an
    /// arm.
    pub(crate) task_handles: &'a [LuaSectionHandle],
    /// The shared library replayed as the section's first chunk.
    pub(crate) shared: &'a LuaProgram,
}

/// Runs one section VM's setup sequence against a constructed, limited VM.
///
/// The sequence is fixed and shared: host injection carrying the driver's
/// [`VmSeed`], [`SectionVm::install_host_apis`], the `item` global when the
/// seed is [`VmSeed::Item`], [`SectionVm::install_control_globals`] with the
/// driver's callbacks, [`SectionVm::replay_shared`], and
/// [`SectionVm::install_captured_bindings`]. The caller applies the Lua
/// limits itself before calling, so a limits failure propagates without
/// touching the VM's teardown observation path.
///
/// On failure the VM is left for the caller to tear down, so each driver's
/// own teardown boundary (the walk's inline teardown, the arm's single
/// epilogue) stays the one place a teardown happens.
///
/// # Errors
/// Returns the [`Error`] of whichever step failed: host injection, host API
/// install, `item` install, control-global install, the shared replay, or
/// the captured-binding install.
pub(crate) fn setup_section_vm<E, F, L>(
    vm: &mut SectionVm,
    setup: &SectionVmSetup<'_>,
    execute_callback: E,
    fanout_callback: F,
    list_callback: L,
) -> Result<()>
where
    E: Fn(LuaValue, Option<String>) -> std::result::Result<String, Error> + Send + 'static,
    F: Fn(String, Vec<serde_json::Value>) -> std::result::Result<Vec<LuaFanoutResult>, Error>
        + Send
        + 'static,
    L: Fn(String) -> std::result::Result<Vec<String>, Error> + Send + 'static,
{
    let (initial_var, item) = match setup.seed {
        VmSeed::InitialVar(initial_var) => (initial_var, None),
        VmSeed::Item(item) => (None, Some(item)),
    };
    vm.inject_host_with_var(
        setup.args,
        setup.sys,
        setup.store,
        setup.last_reply,
        initial_var,
    )?;
    vm.install_host_apis(setup.observer_arc, setup.section_name)?;
    if let Some(item) = item {
        vm.set_global_json("item", item)?;
    }
    vm.install_control_globals(
        setup.task_handles,
        execute_callback,
        fanout_callback,
        list_callback,
    )?;
    vm.replay_shared(
        setup.shared,
        setup.observer_arc.as_ref(),
        setup.section_name,
    )?;
    vm.install_captured_bindings()
}
