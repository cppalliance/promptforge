//! The `model:infer` tool bag, its generation cache, and the infer hook that
//! installs a section VM's nested-inference bridge.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

use crate::cancel;
use crate::client::{CompletionResult, Message, ToolSchema};
use crate::debug::{DebugCapture, DebugEvent};
use crate::lua::{
    LiveBindingProducer, LocalTools, ModelBindings, ModelInferHook, ModelRuntime, ModelsInferHook,
    SectionVm, ToolBinding, ToolBindings, ToolCallCounts, ToolRuntime, current_tool_bindings,
    install_lua_tool_calls, resolve_model_binding,
};
use crate::model::ModelBinding;
use crate::observe::{Observer, detail};
use crate::tools::{SharedTools, ToolId, ToolRegistry};
use crate::{Error, Result};

use super::gateway::GatewaySource;
use super::scope::{ToolAnalysis, prepare_scoped_tools, validate_effective_scope_inner};
use super::support::{advance_turn, bridge_blocking};
use super::tool_loop::{SectionProgress, run_tool_loop};

/// Cached schemas/dispatch for one tool-bag generation.
struct CachedToolState {
    generation: u64,
    bindings: Vec<ToolBinding>,
    schemas: Vec<ToolSchema>,
    dispatch: BTreeMap<String, ToolId>,
}

/// Result of preparing the model-visible tool set for one `model:infer` call.
pub(crate) struct PreparedTools {
    /// Effective bindings in model-advertisement order.
    pub(crate) bindings: Vec<ToolBinding>,
    /// Schemas advertised to the model for this infer.
    pub(crate) schemas: Vec<ToolSchema>,
    /// Alias-to-identity dispatch map for this infer.
    pub(crate) dispatch: BTreeMap<String, ToolId>,
    /// Whether schemas and dispatch came from the generation cache. Test-only
    /// diagnostic: it exists solely to let cache tests assert reuse.
    #[cfg(test)]
    pub(crate) reused: bool,
}

/// Effective tool set with a generation-tracked schema/dispatch cache.
///
/// Mutations via `tools.add` or `tools.add_local` bump [`ToolRuntime::generation`].
/// Each [`Self::prepare`] call rebuilds schemas and dispatch only when that
/// generation no longer matches the cache. Used by `model:infer`; the
/// implicit prose path still builds scope through `prepare_effective_scope`.
pub(crate) struct ToolBag {
    bindings: ToolBindings,
    runtime: Arc<Mutex<ToolRuntime>>,
    local: LocalTools,
    cached: Option<CachedToolState>,
}

impl ToolBag {
    /// Creates a bag over frozen bindings, the live H2 addition runtime, and
    /// the section VM's local-tool registry.
    #[must_use]
    pub(crate) fn new(
        bindings: ToolBindings,
        runtime: Arc<Mutex<ToolRuntime>>,
        local: LocalTools,
    ) -> Self {
        Self {
            bindings,
            runtime,
            local,
            cached: None,
        }
    }

    /// Returns frozen prompt-level bindings for diagnostics and `tools.calls`.
    #[must_use]
    pub(crate) fn bindings(&self) -> &ToolBindings {
        &self.bindings
    }

    /// Snapshot-reads the live bag; rebuilds schemas/dispatch on generation mismatch.
    ///
    /// # Errors
    /// Returns tool-scope or registry errors from snapshot/validation/schema build.
    pub(crate) fn prepare(&mut self, registry: &ToolRegistry<'_>) -> Result<PreparedTools> {
        let generation = {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
            runtime.generation()
        };
        if let Some(cached) = &self.cached
            && cached.generation == generation
        {
            return Ok(PreparedTools {
                bindings: cached.bindings.clone(),
                schemas: cached.schemas.clone(),
                dispatch: cached.dispatch.clone(),
                #[cfg(test)]
                reused: true,
            });
        }

        let bindings = current_tool_bindings(&self.bindings, &self.runtime)?;
        let (schemas, dispatch) = prepare_scoped_tools(&bindings, &self.local.schemas(), registry)?;
        self.cached = Some(CachedToolState {
            generation,
            bindings: bindings.clone(),
            schemas: schemas.clone(),
            dispatch: dispatch.clone(),
        });
        Ok(PreparedTools {
            bindings,
            schemas,
            dispatch,
            #[cfg(test)]
            reused: false,
        })
    }
}

/// Shared context for `model:infer` from Lua.
///
/// Carries the gateway client, tool pool, observer, and the live tool bag so
/// each infer call can snapshot-read the current effective set.
pub(crate) struct InferContext {
    client: GatewaySource,
    shared_tools: SharedTools,
    observer: Arc<dyn Observer>,
    /// Owned debug sink so nested `model:infer` capture is not lost (F4).
    debug: Option<Arc<dyn DebugCapture>>,
    execution: String,
    section: String,
    max_tool_iterations: usize,
    turns: Arc<AtomicU32>,
    analysis: Option<ToolAnalysis>,
    live_bindings: Option<LiveBindingProducer>,
    tool_bag: Mutex<ToolBag>,
    /// The section VM's local-tool registry, called back into on the `lua`
    /// state the infer hook receives when the model invokes a local tool.
    local_tools: LocalTools,
    counts_slot: Arc<Mutex<Option<ToolCallCounts>>>,
    /// Live sealed `sys` JSON so infer can publish `reply_finish_reason`.
    sys_live: Arc<Mutex<Option<serde_json::Value>>>,
    /// Frozen prompt-level model bindings for `models.infer` resolution.
    model_bindings: ModelBindings,
    /// The section's `models.use` selection runtime for `models.infer`.
    model_runtime: Arc<Mutex<ModelRuntime>>,
}

impl InferContext {
    fn prepare_tools(
        &self,
        registry: &ToolRegistry<'_>,
    ) -> mlua::Result<(PreparedTools, Vec<String>)> {
        if let Some(live) = &self.live_bindings {
            let bindings = live.bindings().map_err(mlua::Error::external)?.0;
            // H1 has no local tools (`tools.add_local` is H2-only), so the local
            // schema list stays empty on the live path.
            let scope: Vec<ToolBinding> = bindings
                .always()
                .iter()
                .filter_map(|alias| {
                    bindings
                        .bindings()
                        .iter()
                        .find(|binding| binding.alias() == alias)
                        .cloned()
                })
                .collect();
            let (schemas, dispatch) =
                prepare_scoped_tools(&scope, &[], registry).map_err(mlua::Error::external)?;
            let declared = bindings
                .bindings()
                .iter()
                .map(|binding| binding.alias().to_owned())
                .collect();
            return Ok((
                PreparedTools {
                    bindings: scope,
                    schemas,
                    dispatch,
                    #[cfg(test)]
                    reused: false,
                },
                declared,
            ));
        }

        let mut bag = self
            .tool_bag
            .lock()
            .map_err(|_| mlua::Error::external("tool bag mutex was poisoned"))?;
        let prepared = bag.prepare(registry).map_err(mlua::Error::external)?;
        if let Some(analysis) = &self.analysis {
            validate_effective_scope_inner(analysis, &prepared.bindings)
                .map_err(mlua::Error::external)?;
        }
        let declared = bag
            .bindings()
            .bindings()
            .iter()
            .map(|binding| binding.alias().to_owned())
            .collect();
        Ok((prepared, declared))
    }

    /// Snapshot-reads the tool bag, runs the tool loop, sets `reply`, returns text.
    fn infer(
        self: &Arc<Self>,
        lua: &mlua::Lua,
        binding: &ModelBinding,
        prompt: &str,
    ) -> mlua::Result<String> {
        let registry = self.shared_tools.registry();
        let (prepared, declared) = self.prepare_tools(&registry)?;
        let counts = {
            let mut slot = self
                .counts_slot
                .lock()
                .map_err(|_| mlua::Error::external("tool call counts mutex was poisoned"))?;
            // Seed from the dispatch keys so both registry and local tool
            // aliases are countable.
            if let Some(existing) = slot.as_ref() {
                for alias in prepared.dispatch.keys() {
                    existing.ensure(alias).map_err(mlua::Error::external)?;
                }
                existing.clone()
            } else {
                let created = ToolCallCounts::new(prepared.dispatch.keys().cloned());
                *slot = Some(created.clone());
                created
            }
        };
        install_lua_tool_calls(lua, &counts, &declared).map_err(mlua::Error::external)?;

        let completion_options = binding.completion_options();
        // Resolve the client on first use so a construction failure surfaces
        // here, at the first attempted inference, rather than being swallowed at
        // setup (F5).
        let client = self.client.resolve().map_err(mlua::Error::external)?;
        // Local tools are Lua functions on this VM's state - the very `lua`
        // the hook was invoked on - so route their calls back into it.
        let local_tools = self.local_tools.clone();
        let local_dispatch =
            move |alias: &str, args: serde_json::Value| local_tools.call(lua, alias, args);
        let (text, finish_reason) = bridge_blocking(run_tool_loop(
            &client,
            &prepared.schemas,
            &prepared.dispatch,
            &registry,
            prompt.to_owned(),
            self.max_tool_iterations,
            SectionProgress {
                execution: &self.execution,
                observer: self.observer.as_ref(),
                section: &self.section,
                turns: self.turns.as_ref(),
                // The run's owned debug sink reaches nested inference so its
                // request/response capture is not lost (F4).
                debug: self.debug.as_deref(),
                completion_options: &completion_options,
            },
            Some(&counts),
            Some(&prepared.dispatch),
            Some(&local_dispatch),
        ))
        .map_err(mlua::Error::external)?;

        lua.globals()
            .raw_set("reply", text.as_str())
            .map_err(mlua::Error::external)?;
        {
            let mut live = self
                .sys_live
                .lock()
                .map_err(|_| mlua::Error::external("sys live slot was poisoned"))?;
            if let Some(sys) = live.as_mut() {
                *sys = crate::lua::enrich_sys_reply_finish_reason(sys, finish_reason.as_deref());
                let table = crate::lua::seal_sys(lua, sys).map_err(mlua::Error::external)?;
                lua.globals()
                    .raw_set("sys", table)
                    .map_err(mlua::Error::external)?;
            }
        }
        Ok(text)
    }

    /// Resolves the section's current model binding for `models.infer`.
    ///
    /// On the live H1 path the bindings are still being recorded, so the
    /// snapshot comes from the run's producer; on the H2 path the VM's frozen
    /// bindings are used. The current alias is the H2 `models.use` selection,
    /// else the prompt-wide `models.default` baseline.
    fn current_model_binding(&self) -> mlua::Result<ModelBinding> {
        let bindings = if let Some(live) = &self.live_bindings {
            live.bindings().map_err(mlua::Error::external)?.1
        } else {
            self.model_bindings.clone()
        };
        resolve_model_binding(&bindings, &self.model_runtime)
            .map_err(mlua::Error::external)?
            .ok_or_else(|| {
                mlua::Error::external(Error::ModelRequired {
                    section: self.section.clone(),
                })
            })
    }

    /// Runs `models.infer`: one direct, tool-free gateway round on a fresh
    /// conversation with the section's current model.
    ///
    /// Unlike [`Self::infer`], this is not a tool loop: no schemas are
    /// advertised, and neither `reply` nor `sys.reply_finish_reason` is
    /// touched. Turn counting, observation, and debug capture match a single
    /// prose round so nested inference is not lost (observe F1, F4).
    fn infer_direct(&self, prompt: &str) -> mlua::Result<String> {
        let binding = self.current_model_binding()?;
        let completion_options = binding.completion_options();
        let client = self.client.resolve().map_err(mlua::Error::external)?;
        let conversation = [Message::user(prompt)];
        let completion = bridge_blocking(async {
            tokio::select! {
                biased;
                () = cancel::wait_cancelled() => Err(Error::Interrupted),
                result = client.complete(&conversation, None, &completion_options) => {
                    result.map_err(Error::from)
                }
            }
        })
        .map_err(mlua::Error::external);
        if completion.is_err() {
            self.observer
                .observe(&self.execution, &self.section, detail::MODEL_TURN_FAILED);
        }
        let completion = completion?;

        let turn = advance_turn(&self.turns);
        if let Some(capture) = self.debug.as_deref() {
            capture.on_event(
                &self.execution,
                &self.section,
                turn,
                DebugEvent::Request {
                    body: completion.request_body,
                },
            );
            capture.on_event(
                &self.execution,
                &self.section,
                turn,
                DebugEvent::Response {
                    body: completion.response_body.clone(),
                    finish_reason: completion.finish_reason.clone(),
                    reasoning_content: completion.reasoning_content.clone(),
                },
            );
        }
        self.observer
            .observe(&self.execution, &self.section, detail::MODEL_TURN_COMPLETED);

        match completion.result {
            CompletionResult::Text(text) => {
                if completion.finish_reason.as_deref() == Some("length") {
                    self.observer.observe(
                        &self.execution,
                        &self.section,
                        detail::MODEL_TURN_TRUNCATED,
                    );
                }
                Ok(text)
            }
            // No tools were advertised, so a tool-call turn is a backend
            // protocol violation rather than something to dispatch.
            CompletionResult::ToolCalls(_) => Err(mlua::Error::external(
                "models.infer received tool calls but no tools were advertised",
            )),
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "infer hook installation threads the same borrowed run context fanout already carries"
)]
pub(crate) fn attach_infer_hook(
    vm: &SectionVm,
    client: GatewaySource,
    shared_tools: &SharedTools,
    observer: Arc<dyn Observer>,
    debug: Option<Arc<dyn DebugCapture>>,
    execution: &str,
    section: &str,
    max_tool_iterations: usize,
    turns: &Arc<AtomicU32>,
    analysis: Option<&ToolAnalysis>,
    live_bindings: Option<LiveBindingProducer>,
) {
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    let (model_bindings, model_runtime) = vm.model_bag_handles();
    let local_tools = vm.local_tools_handle();
    let ctx = Arc::new(InferContext {
        client,
        shared_tools: shared_tools.clone(),
        // The run's owned observer reaches the nested `model:infer` hook, so
        // observations from nested inference are not lost (observe F1).
        observer,
        // The run's owned debug sink likewise reaches nested inference (F4).
        debug,
        execution: execution.to_owned(),
        section: section.to_owned(),
        max_tool_iterations,
        turns: Arc::clone(turns),
        analysis: analysis.cloned(),
        live_bindings,
        tool_bag: Mutex::new(ToolBag::new(tool_bindings, tool_runtime, local_tools.clone())),
        local_tools,
        counts_slot: vm.counts_slot(),
        sys_live: vm.sys_live_handle(),
        model_bindings,
        model_runtime,
    });
    let direct = Arc::clone(&ctx);
    let hook: ModelInferHook =
        Arc::new(move |lua, binding, prompt| ctx.infer(lua, binding, prompt));
    vm.set_infer_hook(hook);
    let models_hook: ModelsInferHook = Arc::new(move |_, prompt| direct.infer_direct(prompt));
    vm.set_models_infer_hook(models_hook);
}
