//! Engine injection: the one step that turns an uninjected section VM into
//! an injected one, installing `args`, `argv`, `sys`, `var`, and the
//! `tools`, `plugins`, `models`, `messages`, and `compactors` tables.

use std::sync::Arc;

use runvil_types::emitter::Emitter;
use runvil_vfs::Access;
use serde_json::Value as Json;

use super::{Injected, SYS_LIVE_POISONED, SectionVm, Uninjected};
use crate::argv::Argv;
use crate::compactors::install_compactors;
use crate::error::{Error, Result};
use crate::messages::install_messages;
use crate::models::install_models;
use crate::plugins::install_plugins;
use crate::sys::{guarded_var, seal_sys};
use crate::tools::install_tools;

/// A refused Engine injection: the error, beside the uninjected VM it was
/// refused on, which the caller tears down at its own boundary.
#[derive(Debug)]
pub struct InjectFailure {
    vm: Box<SectionVm<Uninjected>>,
    error: Error,
}

impl InjectFailure {
    /// Why the injection failed.
    #[must_use]
    pub fn error(&self) -> &Error {
        &self.error
    }

    /// Tears the refused VM down under `section` and returns why the
    /// injection failed.
    #[must_use]
    pub fn teardown(self, emitter: &Emitter, section: &str) -> Error {
        self.vm.teardown(emitter, section);
        self.error
    }
}

impl SectionVm<Uninjected> {
    /// Installs the section's Engine values, ahead of the Plugin preludes,
    /// with an empty `var` and a frozen nil `argv`.
    ///
    /// # Errors
    /// Returns the [`InjectFailure`] holding the bridging failure if Engine
    /// values cannot be bridged.
    pub fn inject_values(
        self,
        args: &str,
        sys: &Json,
        access: &Arc<Access>,
    ) -> std::result::Result<SectionVm, InjectFailure> {
        self.inject_values_with_var(args, sys, access, None, Argv::Frozen(None))
    }

    /// Installs Engine values while seeding `var` from an earlier VM, and
    /// gives back the injected VM.
    ///
    /// Injection consumes the uninjected VM, so it runs once. The store
    /// callbacks own a clone of the run-scoped store. `log` and `store` are
    /// installed once for the section's whole lifecycle by
    /// [`install_engine_globals`](SectionVm::install_engine_globals), which
    /// captures a clone of the emitter rather than a per-chunk borrow.
    ///
    /// The `var` global is a guarded proxy (see `guarded_var`): writes are
    /// validated for JSON-representability at the assigning line, and the
    /// hidden data table behind it is what [`var`](SectionVm::var) reads
    /// back. `access` is the chain step's VFS capability: the `store`
    /// table's closures share it, so a fanout arm's store ops are attributed
    /// to the arm's spawned identity, and a conflicting access from an
    /// identity not ordered after the arm's surfaces as a write race.
    ///
    /// `argv` is the parsed form of the args string, installed per its
    /// [`Argv`] mode: writable for the H1 pass (whose repaired value the
    /// executor reads back with [`argv_json`](SectionVm::argv_json)), frozen
    /// for every other section.
    ///
    /// # Errors
    /// Returns the [`InjectFailure`] holding the bridging failure if Engine
    /// values cannot be bridged.
    #[expect(
        clippy::similar_names,
        reason = "args and argv are the spec'd global names; the pair is intentional"
    )]
    pub fn inject_values_with_var(
        self,
        args: &str,
        sys: &Json,
        access: &Arc<Access>,
        initial_var: Option<&Json>,
        argv: Argv<'_>,
    ) -> std::result::Result<SectionVm, InjectFailure> {
        if let Err(error) = self.install_values(args, sys, initial_var, argv) {
            return Err(InjectFailure {
                vm: Box::new(self),
                error,
            });
        }
        Ok(self.injected(Arc::clone(access)))
    }

    #[expect(
        clippy::similar_names,
        reason = "args and argv are the spec'd global names; the pair is intentional"
    )]
    fn install_values(
        &self,
        args: &str,
        sys: &Json,
        initial_var: Option<&Json>,
        argv: Argv<'_>,
    ) -> Result<()> {
        let globals = self.lua.globals();
        globals.raw_set("args", args).map_err(Error::lua)?;
        match argv {
            Argv::Writable(value) => crate::argv::install_writable(&self.lua, value)?,
            Argv::Frozen(value) => crate::argv::install_frozen(&self.lua, value)?,
        }
        let sys_table = seal_sys(&self.lua, sys)?;
        globals.raw_set("sys", sys_table).map_err(Error::lua)?;
        {
            let mut live = self
                .sys_live
                .lock()
                .map_err(|_| Error::Internal(SYS_LIVE_POISONED.into()))?;
            *live = Some(sys.clone());
        }
        let var = guarded_var(&self.lua, initial_var)?;
        globals.raw_set("var", var).map_err(Error::lua)?;
        install_tools(
            &self.lua,
            &globals,
            &self.bound_tools,
            &self.tool_runtime,
            &self.local_tools,
        )?;
        install_plugins(&self.lua, &globals, &self.bound_tools, &self.tool_runtime)?;
        install_models(
            &self.lua,
            &globals,
            &self.bound_models,
            &self.model_runtime,
            self.raw_model_ids,
        )?;
        install_messages(&self.lua, &globals)?;
        install_compactors(&self.lua, &globals)
    }

    /// The same VM at the injected stage, holding `access`.
    fn injected(self, access: Arc<Access>) -> SectionVm {
        let SectionVm {
            lua,
            bound_tools,
            bound_models,
            tool_runtime,
            model_runtime,
            jump_slot,
            local_handler_depth,
            library_load_depth,
            library_programs,
            sys_live,
            store_conflicts,
            log_budget,
            log_byte_budget,
            local_tools,
            instruction_budget,
            raw_model_ids,
            state: Uninjected,
        } = self;
        SectionVm {
            lua,
            bound_tools,
            bound_models,
            tool_runtime,
            model_runtime,
            jump_slot,
            local_handler_depth,
            library_load_depth,
            library_programs,
            sys_live,
            store_conflicts,
            log_budget,
            log_byte_budget,
            local_tools,
            instruction_budget,
            raw_model_ids,
            state: Injected { access },
        }
    }
}
