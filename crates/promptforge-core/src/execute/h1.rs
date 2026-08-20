//! The live H1 pass: the ordered top-of-prompt block walk that resolves
//! capabilities and may short-circuit the whole run with a scalar return.

use std::sync::Arc;

use crate::lua::{SectionVm, ToolBindings};
use crate::model::ModelBindings;
use crate::parser::Prompt;
use crate::resolve::RuntimeResolution;
use crate::tools::ToolRegistry;
use crate::{Error, Result};

use super::block_walk::{BlockRunMode, BlockWalkContext, SectionFlow, run_one_section_impl};
use super::engine::RunContext;
use super::gateway::{GatewaySource, ResolutionContext};
use super::scope::ToolAnalysis;
use super::support::{now_rfc3339_checked, sys_json};
use super::tools::attach_infer_hook;

pub(crate) struct LiveH1State {
    pub(crate) bindings: ToolBindings,
    pub(crate) models: ModelBindings,
    pub(crate) var: serde_json::Value,
    pub(crate) returned: Option<String>,
    pub(crate) reply: Option<String>,
}

#[expect(
    clippy::too_many_lines,
    reason = "the H1 shell is one linear pass: VM construction and limits, host setup, the infer hook, the shared block loop, and the state extraction"
)]
pub(crate) async fn execute_live_h1(
    prompt: &Prompt,
    resolution: ResolutionContext<'_>,
    registry: &ToolRegistry<'_>,
    frame: &RunContext<'_>,
) -> Result<LiveH1State> {
    let &RunContext {
        args,
        shared_tools,
        store,
        execution,
        observer,
        observer_arc,
        client,
        debug,
        debug_arc,
        limits,
        turns,
    } = frame;
    let default_max_tool_iterations = limits.tool_iterations().get() as usize;
    let max_tool_iterations = prompt
        .frontmatter
        .max_tool_iterations
        .resolve(default_max_tool_iterations);
    let runtime = RuntimeResolution::new(resolution.picker, registry, resolution.models);
    let now = now_rfc3339_checked()?;
    let sys = sys_json(
        &now,
        &now,
        0,
        &prompt.title,
        execution,
        prompt.sections.len(),
    );
    let mut vm = SectionVm::new(execution, observer, &prompt.title)?;
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
    // path, which the shared block loop builds and propagates its own error
    // for via `h1_try!`.
    h1_try!(vm.install_host_apis(observer_arc, &prompt.title));
    let mut active_client = client.clone();
    attach_infer_hook(
        &vm,
        GatewaySource::from_optional(active_client.clone(), limits),
        shared_tools,
        Arc::clone(observer_arc),
        debug_arc.cloned(),
        execution,
        &prompt.title,
        max_tool_iterations,
        turns,
        None,
        Some(runtime.producer()),
    );

    // The ordered block loop is the shared one (`block_walk`) running in live
    // H1 mode; this shell keeps only VM construction, limits, host setup, the
    // infer hook, the state extraction, and the teardown boundary. The
    // section-only context fields (frozen bindings, models, analysis) are
    // never read in live mode, so the shell fills them with empty
    // placeholders.
    let bindings_placeholder = ToolBindings::default();
    let models_placeholder = ModelBindings::from_parts(Vec::new(), None);
    let analysis_placeholder = ToolAnalysis::default();
    let block_ctx = BlockWalkContext {
        args,
        execution,
        observer,
        debug,
        bindings: &bindings_placeholder,
        models: &models_placeholder,
        analysis: &analysis_placeholder,
        shared_tools,
        max_tool_iterations,
        limits,
        turns: turns.as_ref(),
        item: None,
    };
    let flow = h1_try!(
        run_one_section_impl(
            &mut vm,
            &block_ctx,
            &prompt.title,
            &prompt.h1_blocks,
            BlockRunMode::LiveH1(&runtime),
            sys,
            None,
            &mut active_client,
        )
        .await
    );
    let (returned, reply) = match flow {
        SectionFlow::Returned(value) => (Some(value), None),
        SectionFlow::FellThrough { reply } => (None, reply),
        // `run_live_h1_block` turns a recorded jump into an error, so live
        // mode never yields a jump flow.
        SectionFlow::Jumped { .. } => {
            vm.teardown(observer, &prompt.title);
            return Err(Error::Internal("live H1 block walk reported a jump"));
        }
    };
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
