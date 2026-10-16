//! The Engine's setup half: one section VM's lifecycle from Engine injection
//! through the store yield shims' install.
//!
//! Every section driver - the walk's section entry and the
//! fanout arm - runs the identical setup sequence: inject the Engine values,
//! install the persistent Engine globals, install the control globals, install
//! the Plugin preludes, install `require`, then install the store yield
//! shims.
//! Only the deltas live at the call site: the driver builds its own `sys`
//! JSON (both drivers take the next run-global `id`; the arm adds its
//! per-fanout `index`), picks the [`VmSeed`] (the walk's rolled-forward
//! `var`; an arm
//! adds its collection `item` to the caller's cloned `var`), and supplies
//! the `read` callback behind the `prompt` reads, resolved over its own
//! visible set, the library resolver behind `require`, resolved over its
//! own reach, and the renderer for library prose templates.
//!
//! VM construction and the Lua limits install stay with the driver. A
//! construction failure's handling differs (the walk propagates, the arm
//! finishes its finalizer), and a limits failure must keep
//! propagating as a bare `?` - routing it through teardown would emit
//! `LUA_TEARDOWN_STARTED`/`SUCCEEDED` events the walk never emitted. Setup
//! consumes the uninjected VM, so a setup failure tears it down here, the
//! one place a VM that never reached its frame is torn down.

use std::sync::Arc;

use promptforge_types::emitter::Emitter;
use promptforge_types::plugins::Prelude;

use crate::lua::{Library, LibraryKey, ProseState, SectionText, SectionVm, Uninjected};
use crate::{Error, Result};
use promptforge_vfs::Access;

/// What a section VM is seeded with beyond the shared Engine values.
///
/// Both fields install through the same serde bridge: `var` seeds the hidden
/// data table behind the guarded `var` proxy, and `item` installs as the
/// `item` global after the Engine globals, ahead of every block.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct VmSeed<'a> {
    /// The walk's current `var`: rolled forward across sections on one walk,
    /// cloned into a `call` chain or a fanout arm; `None` seeds an empty
    /// table.
    pub(crate) var: Option<&'a serde_json::Value>,
    /// The fanout arm's collection member; `None` outside an arm.
    pub(crate) item: Option<&'a serde_json::Value>,
}

/// The borrowed inputs one section VM setup shares.
///
/// Bundled so the walk and the fanout arm each thread one linear set of
/// borrows into [`setup_section_vm`] rather than a dozen parameters.
pub(crate) struct SectionVmSetup<'a> {
    /// The run's argument string, installed as the `args` global.
    pub(crate) args: &'a str,
    /// The run's `argv` (the parsed form of `args`; `None` installs nil).
    pub(crate) argv: Option<&'a serde_json::Value>,
    /// H1 only: `argv` installs writable, so the repair pattern can assign
    /// it; every other section gets the frozen value.
    pub(crate) argv_writable: bool,
    /// The `sys` JSON the driver built for this section or arm.
    pub(crate) sys: &'a serde_json::Value,
    /// The chain step's VFS access capability backing the Lua `store`
    /// table: the walk's own, a call chain's borrowed parent capability, or
    /// a fanout arm's spawned one. The store closures share it, so every
    /// store op is attributed to the chain step's identity.
    pub(crate) access: &'a Arc<Access>,
    /// The driver-specific seed: the walk's `var`, plus the collection
    /// `item` for an arm.
    pub(crate) seed: VmSeed<'a>,
    /// The chain's emitter: the persistent Engine globals (`log`, `store`)
    /// capture a clone, and `require`'s library loads report through it.
    pub(crate) emitter: &'a Emitter,
    /// The section name used in observations and error messages.
    pub(crate) section_name: &'a str,
    /// The run's resolved per-section tool-loop cap, captured by the
    /// `models.loop` shim as its round cap.
    pub(crate) max_tool_iterations: usize,
    /// The run's application-state snapshot, when the caller supplied one:
    /// its presence gives the section VM the `ui()` global and the
    /// raw-model-id `models.get` fallback. Shared through the run's `Arc`,
    /// so every section VM serializes the one tree.
    pub(crate) ui: Option<&'a Arc<serde_json::Value>>,
    /// The run's Plugin preludes, installed in order after the coroutine
    /// shims and before `require`.
    pub(crate) preludes: &'a [Prelude],
    /// Test-only: installs the raw `tools.call_as_model` shim, so a fixture
    /// section can yield one model-issued `tool_call`.
    #[cfg(test)]
    pub(crate) raw_shims: bool,
}

/// Runs one section VM's setup sequence against a constructed, limited VM.
///
/// The sequence is fixed and shared: Engine injection with the driver's
/// [`VmSeed`], [`SectionVm::install_engine_globals`], the `item` global when the
/// seed includes one, the control surface
/// ([`SectionVm::install_scheduler_control_globals`] for `jump` and the
/// `prompt` reads, plus [`SectionVm::install_coro_shims`] for the
/// suspending calls, which the scheduler drives as yield shims), the run's
/// Plugin preludes, [`SectionVm::install_require`] over `resolve` and
/// `render`, and the store yield shims. The caller applies
/// the Lua limits itself before calling, so a limits failure propagates
/// without touching the VM's teardown observation path.
///
/// # Errors
/// Returns the [`Error`] of whichever step failed, after tearing the VM
/// down under the setup's section: Engine injection, Engine global
/// install, `item` install, control-global install, the prelude install (a
/// prelude that fails to load, or whose global collides, or a store
/// conflict a prelude's store call recorded), the `require` install, or
/// the store shims' install.
pub(crate) fn setup_section_vm<R, L, P>(
    mut vm: SectionVm<Uninjected>,
    setup: &SectionVmSetup<'_>,
    read: R,
    resolve: L,
    render: P,
) -> Result<SectionVm>
where
    R: Fn(String) -> std::result::Result<Option<SectionText>, Error> + Send + Sync + 'static,
    L: Fn(String, Option<LibraryKey>) -> std::result::Result<Library, Error>
        + Send
        + Sync
        + 'static,
    P: Fn(&str, ProseState) -> mlua::Result<String> + Send + Sync + 'static,
{
    // The raw-model-id fallback reads its flag during Engine injection, so
    // the snapshot's opt-in lands first.
    if setup.ui.is_some() {
        vm.allow_raw_model_ids();
    }
    let argv = if setup.argv_writable {
        crate::lua::Argv::Writable(setup.argv)
    } else {
        crate::lua::Argv::Frozen(setup.argv)
    };
    let mut vm = vm
        .inject_values_with_var(setup.args, setup.sys, setup.access, setup.seed.var, argv)
        .map_err(|failure| Error::from(failure.teardown(setup.emitter, setup.section_name)))?;
    match install_after_injection(&mut vm, setup, read, resolve, render) {
        Ok(()) => Ok(vm),
        Err(error) => {
            vm.teardown(setup.emitter, setup.section_name);
            Err(error)
        }
    }
}

/// The setup steps after Engine injection, in [`setup_section_vm`]'s order.
fn install_after_injection<R, L, P>(
    vm: &mut SectionVm,
    setup: &SectionVmSetup<'_>,
    read: R,
    resolve: L,
    render: P,
) -> Result<()>
where
    R: Fn(String) -> std::result::Result<Option<SectionText>, Error> + Send + Sync + 'static,
    L: Fn(String, Option<LibraryKey>) -> std::result::Result<Library, Error>
        + Send
        + Sync
        + 'static,
    P: Fn(&str, ProseState) -> mlua::Result<String> + Send + Sync + 'static,
{
    vm.install_engine_globals(setup.emitter, setup.section_name)?;
    if let Some(snapshot) = setup.ui {
        crate::lua::install_ui(vm.lua(), Arc::clone(snapshot))?;
    }
    if let Some(item) = setup.seed.item {
        vm.set_global_json("item", item)?;
    }
    vm.install_scheduler_control_globals(read)?;
    vm.install_coro_shims(setup.max_tool_iterations)?;
    crate::lua::install_section_loop_shim(vm.lua())?;
    // The preludes read `tools` and `store` from `_G` as they install, so
    // they follow the yield shims.
    let installed = crate::lua::install_preludes(vm.lua(), setup.preludes);
    // A store conflict the direct closures recorded while a prelude
    // installed ends the run with a determinism violation when the install
    // returns - caught by `pcall` or not - so the conflict wins over the
    // install's own outcome.
    if let Some(detail) = vm.take_store_conflict() {
        return Err(Error::Determinism(detail));
    }
    installed?;
    // `require` follows the preludes, so a prelude that calls it while it
    // installs finds none.
    vm.install_require(resolve, render, setup.emitter, setup.section_name)?;
    #[cfg(test)]
    if setup.raw_shims {
        promptforge_lua::test_support::install_model_tool_call_shim(vm.lua())?;
    }
    // The store yield shims install after the preludes: a prelude runs as a
    // main chunk, not a coroutine, so its store calls while it installs
    // must hit the direct closures (which capture the same Arc<Access>,
    // leaving claims attribution unchanged). Installing earlier would make
    // a prelude's top-level `store.write` yield from outside a coroutine.
    // The install also switches any store function a prelude captured, so
    // none of them runs directly after this point.
    crate::lua::install_store_shims(vm.lua()).map_err(Error::from)
}
