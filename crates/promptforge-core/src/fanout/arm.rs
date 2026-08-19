//! Execution of a single fanout arm: its owned payload, the terminal-observation
//! guard, and the linear arm body.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use serde_json::{Value, json};

use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::lua::{LuaBlockResult, LuaFanoutResult, LuaProgram, SectionVm, ToolBindings};
use crate::model::ModelBindings;
use crate::observe::{Observation, Observer, detail};
use crate::parser::Section;
use crate::store::StoreRef;
use crate::tools::SharedTools;
use crate::{Error, Result, cancel, subst};

use super::proxies::ProxyObserver;

/// Everything one spawned fanout arm owns for its independent execution.
pub(crate) struct ArmPayload {
    pub(crate) worker: Section,
    pub(crate) item: Value,
    pub(crate) index: usize,
    pub(crate) store: StoreRef,
    pub(crate) client: Option<GatewayClient>,
    pub(crate) args: String,
    pub(crate) execution: String,
    pub(crate) when: String,
    pub(crate) last_reply: Option<String>,
    pub(crate) shared: LuaProgram,
    pub(crate) bindings: ToolBindings,
    pub(crate) models: ModelBindings,
    pub(crate) analysis: crate::execute::ToolAnalysis,
    pub(crate) shared_tools: SharedTools,
    pub(crate) max_tool_iterations: usize,
    pub(crate) lua_memory_bytes: usize,
    pub(crate) lua_log_events: u32,
    pub(crate) parent_id: usize,
    pub(crate) section_count: usize,
    pub(crate) turns: Arc<AtomicU32>,
    pub(crate) observer: Arc<ProxyObserver>,
    pub(crate) debug: Option<Arc<dyn DebugCapture>>,
    /// Explicit cancellation handle carried across the spawn boundary, since a
    /// spawned arm does not inherit the parent task-local (PF-CANCEL-002).
    pub(crate) cancel: Option<cancel::CancelHandle>,
}

/// Emits exactly one distinct terminal observation per fanout arm.
///
/// The arm's normal exits call [`finish`](Self::finish) with the specific
/// terminal event (succeeded / exhausted / failed). If the arm's future is
/// instead dropped before finalizing - a sibling's hard error aborts it, or the
/// run is cancelled - `Drop` emits [`detail::FANOUT_ARM_CANCELLED`]. Exactly one
/// terminal event therefore fires for every arm (FANOUT-004).
pub(crate) struct ArmFinalizer {
    observer: Arc<ProxyObserver>,
    execution: String,
    section: String,
    finished: bool,
}

impl ArmFinalizer {
    pub(crate) fn new(observer: Arc<ProxyObserver>, execution: String, section: String) -> Self {
        Self {
            observer,
            execution,
            section,
            finished: false,
        }
    }

    pub(crate) fn finish(&mut self, event: Observation) {
        self.finished = true;
        (self.observer.as_ref() as &dyn Observer).observe(&self.execution, &self.section, event);
    }
}

impl Drop for ArmFinalizer {
    fn drop(&mut self) {
        if !self.finished {
            (self.observer.as_ref() as &dyn Observer).observe(
                &self.execution,
                &self.section,
                detail::FANOUT_ARM_CANCELLED,
            );
        }
    }
}

/// Runs one fanout arm to completion.
///
/// VM teardown and the terminal arm observation happen in ONE epilogue
/// (FANOUT-006): the fallible body runs against a borrowed VM without any inline
/// teardown, then the epilogue tears the VM down once and records the single
/// distinct terminal event via [`ArmFinalizer`].
#[expect(
    clippy::too_many_lines,
    reason = "the arm body is one cohesive linear sequence of fallible steps"
)]
pub(crate) async fn run_one_arm(payload: ArmPayload) -> Result<(usize, LuaFanoutResult)> {
    let ArmPayload {
        worker,
        item,
        index,
        store,
        client,
        args,
        execution,
        when,
        last_reply,
        shared,
        bindings,
        models,
        analysis,
        shared_tools,
        max_tool_iterations,
        lua_memory_bytes,
        lua_log_events,
        parent_id,
        section_count,
        turns,
        observer,
        debug,
        cancel,
    } = payload;

    let taskid = (index + 1).to_string();
    let observer_arc = observer;
    let observer = observer_arc.as_ref() as &dyn Observer;
    observer.observe(&execution, &worker.name, detail::FANOUT_ARM_STARTED);

    // The guard defaults to a CANCELLED terminal event; the epilogue below
    // upgrades it to the arm's real outcome unless the arm is aborted first.
    let mut finalizer = ArmFinalizer::new(
        Arc::clone(&observer_arc),
        execution.clone(),
        worker.name.clone(),
    );

    let mut vm =
        match SectionVm::new_for_section(&bindings, &models, &execution, observer, &worker.name) {
            Ok(vm) => vm,
            Err(error) => {
                finalizer.finish(detail::FANOUT_ARM_FAILED);
                return Err(error);
            }
        };

    // The body performs no teardown; every fallible step uses `?`. It returns the
    // arm result paired with its distinct terminal event.
    let body = async {
        vm.apply_lua_limits(lua_memory_bytes, lua_log_events)?;

        let now = crate::execute::now_rfc3339_checked()?;
        let sys = json!({
            "when": when,
            "now": now,
            "id": parent_id,
            "taskid": taskid,
            "section_name": worker.name,
            "execution": execution,
            "section_count": section_count,
        });

        vm.inject_host(&args, &sys, &store, last_reply.as_deref())?;
        vm.install_host_apis(&(observer_arc.clone() as Arc<dyn Observer>), &worker.name)?;
        vm.set_global_json("item", &item)?;

        // Arms get the same control globals as a walked section, but nested
        // execute/fanout have no walk to re-enter here, so both fail loudly.
        // `jump` records into the arm VM's slot and is rejected below.
        vm.install_control_globals(
            &[],
            |_, _| {
                Err(Error::Lua(
                    "execute() is not available inside a fanout arm".to_owned(),
                ))
            },
            |_, _| {
                Err(Error::Lua(
                    "fanout() is not available inside a fanout arm".to_owned(),
                ))
            },
        )?;

        // The shared library replays as the arm's first chunk with the full
        // host environment installed; the captured alias globals install only
        // after the replay, so a declared alias wins over a same-named shared
        // global.
        vm.replay_shared(&shared, observer, &worker.name)?;
        vm.install_captured_bindings()?;

        if let Some(program) = worker.prologue() {
            match vm.run_chunk(program, observer, &worker.name)? {
                LuaBlockResult::Returned(Some(value)) => {
                    return Ok((
                        LuaFanoutResult::success(item.clone(), value),
                        detail::FANOUT_ARM_SUCCEEDED,
                    ));
                }
                LuaBlockResult::Returned(None) => {}
                LuaBlockResult::Jump(heading) => {
                    return Err(Error::Lua(format!(
                        "jump({heading}) is not allowed inside a fanout arm"
                    )));
                }
            }
        }

        let scope = crate::lua::current_tool_bindings(&bindings, &vm.tool_runtime)?;
        let counts = Some(vm.install_tool_call_counts(&scope)?);
        let local_schemas = vm.local_tool_schemas();
        if let Some(counts) = counts.as_ref() {
            for schema in &local_schemas {
                counts.ensure(&schema.name)?;
            }
        }
        let model = crate::lua::resolve_model_binding(&models, &vm.model_runtime)?;

        let sys = if let Some(model_binding) = model.as_ref() {
            let current = vm.current_sys(&sys)?;
            let enriched = crate::lua::enrich_sys_model(&current, model_binding);
            vm.re_seal_sys(&enriched)?;
            enriched
        } else {
            sys
        };

        let var = vm.var()?;
        let prose = subst::substitute(
            worker.prose(),
            &args,
            last_reply.as_deref(),
            Some(&item),
            &var,
            &sys,
        )?;

        let mut arm_reply: Option<String> = None;
        if !prose.trim().is_empty() {
            let Some(model_binding) = model else {
                return Err(Error::ModelRequired {
                    section: worker.name.clone(),
                });
            };
            let completion_options = model_binding.completion_options();
            let registry = shared_tools.registry();
            let (schemas, dispatch) = crate::execute::prepare_effective_scope(
                &analysis,
                &scope,
                &local_schemas,
                &registry,
                &execution,
                observer,
                &worker.name,
            )?;
            if let Some(client) = client.as_ref() {
                let global_aliases = Some(&analysis.alias_to_id);
                let debug_ref = debug.as_deref();
                // Local tools are Lua functions on the arm's VM; route their
                // calls back into it rather than the registry.
                let local_dispatch =
                    |alias: &str, args: serde_json::Value| vm.call_local_tool(alias, &args);
                match crate::execute::run_tool_loop(
                    client,
                    &schemas,
                    &dispatch,
                    &registry,
                    prose,
                    max_tool_iterations,
                    crate::execute::SectionProgress {
                        execution: &execution,
                        observer,
                        section: &worker.name,
                        turns: turns.as_ref(),
                        debug: debug_ref,
                        completion_options: &completion_options,
                    },
                    counts.as_ref(),
                    global_aliases,
                    Some(&local_dispatch),
                )
                .await
                {
                    Ok((text, finish_reason)) => {
                        let current = vm.current_sys(&sys)?;
                        let enriched = crate::lua::enrich_sys_reply_finish_reason(
                            &current,
                            finish_reason.as_deref(),
                        );
                        vm.re_seal_sys(&enriched)?;
                        vm.bind_reply(&text, observer, &worker.name)?;
                        arm_reply = Some(text);
                    }
                    // One stuck arm must not kill sibling evidence facets.
                    Err(Error::ToolLoopExhausted) => {
                        let stub = format!(
                            "## {}\n\nUNKNOWN\n\n(section incomplete: tool loop exhausted)",
                            subst::render_item(&item)
                        );
                        return Ok((
                            LuaFanoutResult::exhausted_stub(item.clone(), stub),
                            detail::FANOUT_ARM_EXHAUSTED,
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        let epilog_return = if let Some(program) = worker.epilog() {
            match vm.run_chunk(program, observer, &worker.name)? {
                LuaBlockResult::Returned(value) => value,
                LuaBlockResult::Jump(heading) => {
                    return Err(Error::Lua(format!(
                        "jump({heading}) is not allowed inside a fanout arm"
                    )));
                }
            }
        } else {
            None
        };

        let text = epilog_return.or(arm_reply).unwrap_or_default();
        Ok((
            LuaFanoutResult::success(item, text),
            detail::FANOUT_ARM_SUCCEEDED,
        ))
    };

    // Re-install the explicit cancel handle on THIS arm's task so its Lua
    // instruction hook and tool loop observe cancellation cooperatively; a
    // spawned task never inherits the parent's task-local (PF-CANCEL-002).
    let outcome: Result<(LuaFanoutResult, Observation)> = match cancel {
        Some(handle) => cancel::scope(handle, body).await,
        None => body.await,
    };

    // Single epilogue: tear the VM down once, then record exactly one terminal
    // observation matching the arm's real outcome.
    vm.teardown(observer, &worker.name);
    match outcome {
        Ok((result, event)) => {
            finalizer.finish(event);
            Ok((index, result))
        }
        Err(error) => {
            finalizer.finish(detail::FANOUT_ARM_FAILED);
            Err(error)
        }
    }
}
