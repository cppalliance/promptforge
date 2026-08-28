//! The live H1 pass: the ordered top-of-prompt block walk that resolves
//! capabilities and may short-circuit the whole run with a scalar return.

use crate::client::GatewayClient;
use crate::resolve::RuntimeResolution;
use crate::{Error, Result};

use super::block_walk::{BlockRunMode, SectionFlow};
use super::context::RunContext;
use super::gateway::ResolutionContext;
use super::section_context::SectionContext;
use super::section_vm::VmSetupMode;

pub(crate) struct LiveH1State {
    pub(crate) var: serde_json::Value,
    pub(crate) returned: Option<String>,
    pub(crate) reply: Option<String>,
}

pub(crate) async fn execute_live_h1(
    ctx: &RunContext,
    resolution: ResolutionContext<'_>,
    client: Option<&GatewayClient>,
) -> Result<LiveH1State> {
    let runtime = RuntimeResolution::new(
        resolution.picker,
        resolution.tools,
        resolution.models,
        ctx.tool_set(),
        ctx.model_set(),
    );
    // Construction and limits failures propagate bare, before any teardown
    // observation exists; every failure past this point drops the frame,
    // whose `Drop` tears the VM down exactly once.
    let mut h1_frame = SectionContext::new_live_h1(ctx, client, VmSetupMode::Legacy)?;
    // The pass owns its client slot: seeded from the run's client, created
    // lazily on the first prose block, whose construction error propagates
    // through `?` while the frame's drop owns the teardown.
    let mut active_client = client.cloned();
    let flow = h1_frame
        .run(
            ctx,
            &ctx.prompt().title,
            &ctx.prompt().h1_blocks,
            BlockRunMode::LiveH1(&runtime),
            &mut active_client,
        )
        .await?;
    let (returned, reply) = match flow {
        SectionFlow::Returned(value) => (Some(value), None),
        SectionFlow::FellThrough { reply } => (None, reply),
        // `run_live_h1_block` turns a recorded jump into an error, so live
        // mode never yields a jump flow.
        SectionFlow::Jumped { .. } => {
            return Err(Error::Internal("live H1 block walk reported a jump"));
        }
    };
    let var = h1_frame.read_var()?;
    // The bindings need no extraction: H1's binds already landed in the
    // run's shared sets, which the walk reads through the views. The H1
    // frame is never marked completed: SECTION_FINISHED is a walked
    // section's boundary, not the setup pass's.
    Ok(LiveH1State {
        var,
        returned,
        reply,
    })
}
