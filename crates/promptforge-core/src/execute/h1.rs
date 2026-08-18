//! The live H1 pass: the ordered top-of-prompt block walk that resolves
//! capabilities and may short-circuit the whole run with a scalar return.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use serde_json::json;

use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::lua::{SectionVm, ToolBinding, ToolBindings, ToolCallCounts};
use crate::model::ModelBindings;
use crate::observe::Observer;
use crate::parser::{Block, Prompt};
use crate::resolve::RuntimeResolution;
use crate::store::StoreRef;
use crate::subst;
use crate::tools::{SharedTools, ToolRegistry};
use crate::{Error, Result};

use super::config::RunLimits;
use super::gateway::{GatewaySource, ResolutionContext, env_client_with_limits};
use super::scope::prepare_scoped_tools;
use super::support::now_rfc3339_checked;
use super::tool_loop::{ProseMode, SectionProgress, run_prose_inference};
use super::tools::attach_infer_hook;

pub(crate) struct LiveH1State {
    pub(crate) bindings: ToolBindings,
    pub(crate) models: ModelBindings,
    pub(crate) var: serde_json::Value,
    pub(crate) returned: Option<String>,
    pub(crate) reply: Option<String>,
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "H1 mirrors the ordered section block walk over explicit run context"
)]
pub(crate) async fn execute_live_h1(
    prompt: &Prompt,
    args: &str,
    resolution: ResolutionContext<'_>,
    registry: &ToolRegistry<'_>,
    shared_tools: &SharedTools,
    store: &StoreRef,
    execution: &str,
    observer: &dyn Observer,
    observer_arc: &Arc<dyn Observer>,
    client: Option<&GatewayClient>,
    debug: Option<&dyn DebugCapture>,
    debug_arc: Option<&Arc<dyn DebugCapture>>,
    limits: RunLimits,
    turns: Arc<AtomicU32>,
) -> Result<LiveH1State> {
    let default_max_tool_iterations = limits.tool_iterations().get() as usize;
    let runtime = RuntimeResolution::new(resolution.picker, registry, resolution.models);
    let now = now_rfc3339_checked()?;
    let sys = json!({
        "when": now.clone(),
        "now": now,
        "id": 0,
        "section_name": prompt.title,
        "execution": execution,
        "section_count": prompt.sections.len(),
    });
    let mut vm = SectionVm::new(None, execution, observer, &prompt.title)?;
    vm.apply_lua_limits(limits.lua_memory().get(), limits.lua_logs().get())?;
    vm.inject_host(args, &sys, store, None)?;
    macro_rules! h1_try {
        ($expression:expr) => {
            match $expression {
                Ok(value) => value,
                Err(error) => {
                    vm.teardown(observer, &prompt.title);
                    return Err(error);
                }
            }
        };
    }
    // The infer hook carries a lazy client source so a nested `model:infer`
    // surfaces a concrete construction error on first use instead of the setup
    // swallowing it (F5). `active_client` stays lazy for the direct H1 prose
    // path below, which builds and propagates its own error via `h1_try!`.
    h1_try!(vm.install_host_apis(observer_arc, &prompt.title));
    let active_client = client.cloned();
    attach_infer_hook(
        &vm,
        GatewaySource::from_optional(active_client.clone(), limits),
        shared_tools,
        Arc::clone(observer_arc),
        debug_arc.cloned(),
        execution,
        &prompt.title,
        prompt
            .frontmatter
            .max_tool_iterations
            .resolve(default_max_tool_iterations),
        &turns,
        None,
        Some(runtime.producer()),
    );
    let mut active_client = active_client;

    let mut conversation = Vec::new();
    let mut reply: Option<String> = None;
    let mut returned = None;
    for block in &prompt.h1_blocks {
        match block {
            Block::Lua(program) => {
                if let Some(value) =
                    h1_try!(vm.run_live_h1_block(program, &runtime, observer, &prompt.title))
                {
                    returned = Some(value);
                    break;
                }
            }
            Block::Prose { text, loop_capable } => {
                let (tool_bindings, model_bindings) = h1_try!(runtime.bindings());
                let Some(alias) = model_bindings.default() else {
                    vm.teardown(observer, &prompt.title);
                    return Err(Error::ModelRequired {
                        section: prompt.title.clone(),
                    });
                };
                let Some(model) = model_bindings.binding(alias) else {
                    vm.teardown(observer, &prompt.title);
                    return Err(Error::ModelRequired {
                        section: prompt.title.clone(),
                    });
                };
                let mut scope: Vec<ToolBinding> = Vec::new();
                for alias in tool_bindings.always() {
                    if let Some(binding) = tool_bindings
                        .bindings()
                        .iter()
                        .find(|binding| binding.alias() == alias)
                    {
                        scope.push(binding.clone());
                    }
                }
                // H1 registers no local tools; the list is always empty here.
                let (schemas, dispatch) =
                    h1_try!(prepare_scoped_tools(&scope, &vm.local_tool_schemas(), registry));
                let var = h1_try!(vm.var());
                let prose = h1_try!(subst::substitute(
                    text,
                    args,
                    reply.as_deref(),
                    None,
                    &var,
                    &sys
                ));
                if prose.trim().is_empty() {
                    continue;
                }
                if active_client.is_none() {
                    active_client = Some(h1_try!(env_client_with_limits(limits)));
                }
                let Some(active_client) = active_client.as_ref() else {
                    vm.teardown(observer, &prompt.title);
                    return Err(Error::Lua(
                        "gateway client was not initialized for H1 prose".to_owned(),
                    ));
                };
                let counts = ToolCallCounts::new(
                    scope.iter().map(|binding| binding.alias().to_owned()),
                );
                let mode = if *loop_capable {
                    ProseMode::Loop {
                        max_tool_iterations: prompt
                            .frontmatter
                            .max_tool_iterations
                            .resolve(default_max_tool_iterations),
                    }
                } else {
                    ProseMode::SingleShot
                };
                let completion_options = model.completion_options();
                let outcome = h1_try!(
                    run_prose_inference(
                        active_client,
                        &schemas,
                        &dispatch,
                        registry,
                        &mut conversation,
                        prose,
                        mode,
                        SectionProgress {
                            execution,
                            observer,
                            section: &prompt.title,
                            turns: turns.as_ref(),
                            debug,
                            completion_options: &completion_options,
                        },
                        Some(&counts),
                        None,
                        // H1 registers no local tools, so there is no local
                        // dispatcher to thread through.
                        None,
                    )
                    .await
                );
                if let Some(text) = outcome.text {
                    h1_try!(vm.set_global_string("reply", &text));
                    reply = Some(text);
                }
            }
        }
    }
    let var = h1_try!(vm.var());
    let (bindings, models) = h1_try!(runtime.bindings());
    vm.teardown(observer, &prompt.title);
    Ok(LiveH1State {
        bindings,
        models,
        var,
        returned,
        reply,
    })
}
