//! The live H1 pass: the ordered top-of-prompt block walk that resolves
//! capabilities and may short-circuit the whole run with a scalar return.

use std::sync::Arc;

use crate::client::GatewayClient;
use crate::lua::{SectionVm, ToolBindings};
use crate::model::ModelBindings;
use crate::parser::Prompt;
use crate::resolve::RuntimeResolution;
use crate::tools::ToolRegistry;
use crate::{Error, Result};

use super::block_walk::{BlockRunMode, SectionFlow, run_one_section_impl};
use super::engine::RunFrame;
use super::gateway::{GatewaySource, ResolutionContext};
use super::support::{now_rfc3339_checked, sys_json};
use super::tools::attach_infer_hook;

pub(crate) struct LiveH1State {
    pub(crate) bindings: ToolBindings,
    pub(crate) models: ModelBindings,
    pub(crate) var: serde_json::Value,
    pub(crate) returned: Option<String>,
    pub(crate) reply: Option<String>,
}

pub(crate) async fn execute_live_h1(
    prompt: &Prompt,
    resolution: ResolutionContext<'_>,
    registry: &ToolRegistry<'_>,
    client: Option<&GatewayClient>,
    frame: &RunFrame<'_>,
) -> Result<LiveH1State> {
    let &RunFrame {
        args,
        store,
        execution,
        observer,
        debug,
        limits,
        turns,
        ..
    } = frame;
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
    let mut vm = SectionVm::new(execution, observer.as_ref(), &prompt.title)?;
    vm.apply_lua_limits(limits.lua_memory().get(), limits.lua_logs().get())?;
    macro_rules! h1_try {
        ($expression:expr) => {
            match $expression {
                Ok(value) => value,
                Err(error) => {
                    vm.teardown(observer.as_ref(), &prompt.title);
                    return Err(error);
                }
            }
        };
    }
    h1_try!(vm.inject_host(args, &sys, store, None));
    // The infer hook carries a lazy client source so a nested `model:infer`
    // surfaces a concrete construction error on first use instead of the setup
    // swallowing it (F5). `active_client` stays lazy for the direct H1 prose
    // path, which the shared block loop builds and propagates its own error
    // for via `h1_try!`.
    h1_try!(vm.install_host_apis(observer, &prompt.title));
    let mut active_client = client.cloned();
    attach_infer_hook(
        &vm,
        GatewaySource::from_optional(active_client.clone(), limits),
        Arc::clone(observer),
        debug.cloned(),
        execution,
        &prompt.title,
        turns,
        Some(runtime.producer()),
    );

    // The ordered block loop is the shared one (`block_walk`) running in live
    // H1 mode on the run frame directly: the frame's walk-only fields carry
    // empty defaults here and live mode never reads them. This shell keeps
    // only VM construction, limits, host setup, the infer hook, the state
    // extraction, and the teardown boundary.
    let flow = h1_try!(
        run_one_section_impl(
            &mut vm,
            frame,
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
            vm.teardown(observer.as_ref(), &prompt.title);
            return Err(Error::Internal("live H1 block walk reported a jump"));
        }
    };
    let var = h1_try!(vm.var());
    let (bindings, models) = h1_try!(runtime.bindings());
    vm.teardown(observer.as_ref(), &prompt.title);
    Ok(LiveH1State {
        bindings,
        models,
        var,
        returned,
        reply,
    })
}
