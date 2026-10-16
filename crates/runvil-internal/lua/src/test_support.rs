//! Test support: what this crate's own tests and `runvil-engine`'s
//! tests and benches reach beyond the production surface. Built for this
//! crate's tests and under the `test-support` feature, which only
//! dev-dependencies and bench `required-features` enable.

use std::sync::{Arc, Mutex};

use mlua::{Function, Lua, MultiValue, Table};
use runvil_model_client::model::ModelSet;
use runvil_types::emitter::Emitter;
use runvil_types::event::lifecycle;
use runvil_types::tools::ToolDescriptor;

use crate::coro::MODEL_TOOL_CALL_REGISTRY;
use crate::error::{Error, Result};
use crate::error_value::ErrorValue;
use crate::handles::{LuaBlockResult, ToolBinding, ToolSet};
use crate::models::ModelRuntime;
use crate::program::LuaProgram;
use crate::protocol::Answer;
use crate::scope::ToolRuntime;
use crate::vm::{SectionVm, scalar_return};

/// A per-thread tally of live section VMs.
///
/// The Engine's scheduler suite drives one run on the test's own thread
/// (the serial `Scenario` and the rig step it there), and Rust's built-in
/// test harness gives each test its own thread, so thread-locals keep
/// concurrent tests independent. A test resets the peak, drives a run,
/// and reads back the most VMs alive at any moment.
mod vm_tally {
    use std::cell::Cell;

    thread_local! {
        pub(super) static LIVE: Cell<usize> = const { Cell::new(0) };
        pub(super) static PEAK: Cell<usize> = const { Cell::new(0) };
    }
}

/// One section VM's count in the tally: entering counts it, dropping it
/// releases the count.
#[derive(Debug)]
struct VmTally;

impl VmTally {
    fn enter() -> Self {
        vm_tally::LIVE.with(|live| {
            let now = live.get() + 1;
            live.set(now);
            vm_tally::PEAK.with(|peak| peak.set(peak.get().max(now)));
        });
        Self
    }
}

impl Drop for VmTally {
    fn drop(&mut self) {
        vm_tally::LIVE.with(|live| live.set(live.get() - 1));
    }
}

/// Counts a section VM's fresh Lua state as one live VM until the state
/// closes, which happens when the VM drops: the VM holds the state's only
/// strong handle.
pub(crate) fn count_live_vm(lua: &Lua) {
    lua.set_app_data(VmTally::enter());
}

/// Resets the live-section-VM peak tally, so a test measures the one run
/// it drives next.
pub fn reset_section_vm_peak() {
    vm_tally::PEAK.with(|peak| peak.set(0));
}

/// The most section VMs alive at once since the last
/// [`reset_section_vm_peak`]: the admission test's cost pin, since a
/// queued task holds no VM.
#[must_use]
pub fn section_vm_peak() -> usize {
    vm_tally::PEAK.with(std::cell::Cell::get)
}

/// Installs the model-issued `tool_call` form as `tools.call_as_model` on a
/// VM whose shim prelude already ran, so a fixture section can yield a
/// `tool_call` with a `call_id` straight at the driver's dispatch arm. In
/// production `tools.call_as_model` never exists in any VM - not stubbed,
/// simply absent.
///
/// # Errors
/// Returns [`Error::LuaRuntime`] if the shim prelude was never installed
/// on this VM, the `tools` table is absent, or the install fails.
pub fn install_model_tool_call_shim(lua: &Lua) -> Result<()> {
    let model_tool_call: Function = lua
        .named_registry_value(MODEL_TOOL_CALL_REGISTRY)
        .map_err(Error::lua)?;
    let tools: Table = lua.globals().raw_get("tools").map_err(Error::lua)?;
    tools
        .raw_set("call_as_model", model_tool_call)
        .map_err(Error::lua)
}

impl ToolBinding {
    /// Builds a binding for a test double: the identity and schema come
    /// from the descriptor, and the description is `description`.
    #[must_use]
    pub fn for_test(alias: &str, description: &str, descriptor: &ToolDescriptor) -> Self {
        let mut described = descriptor.clone();
        description.clone_into(&mut described.description);
        Self::from_descriptor(alias, &described)
    }
}

impl<E: ErrorValue> Answer<E> {
    /// Renders the `(ok, result)` resume envelope the driver resumes a
    /// block with, for a test that resumes a coroutine by hand, handing
    /// back the typed error the answer retains without the error table
    /// that identifies its raise.
    ///
    /// # Errors
    /// Returns an `mlua` error if a Lua string, userdata, or table cannot be
    /// created on `lua`.
    pub fn into_envelope(self, lua: &Lua) -> mlua::Result<(MultiValue, Option<E>)> {
        let (envelope, retained) = self.render(lua)?;
        Ok((envelope, retained.map(|retained| retained.error)))
    }
}

impl LuaProgram {
    /// Maps a Lua runtime failure of a chunk that ran with no library
    /// programs loaded beside it: `map_runtime_error_among` with none
    /// loaded.
    #[must_use]
    pub fn map_runtime_error(&self, error: &mlua::Error) -> Error {
        self.map_runtime_error_among(error, &[])
    }
}

impl SectionVm {
    /// Executes a compiled Lua chunk in this VM's persistent environment.
    ///
    /// The direct, scheduler-free path for running a section's Lua blocks;
    /// the scheduler drives blocks through
    /// [`start_block_coro`](Self::start_block_coro) instead. Store and
    /// `log` reports go to the emitter captured by
    /// [`install_engine_globals`](Self::install_engine_globals); a nil or
    /// absent top-level return produces [`LuaBlockResult::Returned`]`(None)`.
    /// A recorded jump wins over the chunk's own error, which is the jump's
    /// transfer marker. When the chunk may call `call` or `jump`, those
    /// must already be installed.
    ///
    /// # Errors
    /// Returns the chunk's own failure when execution fails or the program
    /// returns a non-scalar value, or [`Error::Internal`] if the jump slot
    /// is poisoned.
    pub fn run_chunk(
        &self,
        program: &LuaProgram,
        emitter: &Emitter,
        section: &str,
    ) -> Result<LuaBlockResult> {
        emitter.report(section, lifecycle::LUA_CHUNK_STARTED);
        let result = self.run_loaded(program);
        emitter.report(
            section,
            if result.is_ok() {
                lifecycle::LUA_CHUNK_SUCCEEDED
            } else {
                lifecycle::LUA_CHUNK_FAILED
            },
        );
        result
    }

    fn run_loaded(&self, program: &LuaProgram) -> Result<LuaBlockResult> {
        self.clear_jump()?;
        let result = program.load(self.lua())?.call(());
        if let Some(heading) = self.take_jump()? {
            return Ok(LuaBlockResult::Jump(heading));
        }
        let returned = result.map_err(|error| self.map_chunk_failure(program, &error))?;
        Ok(LuaBlockResult::Returned(scalar_return(returned)?))
    }

    /// Returns the shared tool set and the live section addition runtime.
    #[must_use]
    pub fn tool_bag_handles(&self) -> (Arc<ToolSet>, Arc<Mutex<ToolRuntime>>) {
        (
            Arc::clone(&self.bound_tools),
            Arc::clone(self.tool_runtime()),
        )
    }

    /// Returns a snapshot of the shared model set and the live section
    /// selection runtime. Production reads the run's shared set through the
    /// model view instead.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if the shared model set's mutex is poisoned.
    pub fn model_bag_handles(&self) -> Result<(ModelSet, Arc<Mutex<ModelRuntime>>)> {
        let models = self
            .bound_models
            .lock()
            .map_err(|_| Error::Internal("model set mutex was poisoned".into()))?
            .clone();
        Ok((models, Arc::clone(self.model_runtime())))
    }
}
