//! The live H1 pass: the ordered top-of-prompt block walk that resolves
//! capabilities and may short-circuit the whole run with a scalar return.

use crate::client::GatewayClient;
use crate::model::ModelBindings;
use crate::resolve::RuntimeResolution;
use crate::{Error, Result};

use super::block_walk::{BlockRunMode, SectionFlow};
use super::context::RunContext;
use super::gateway::ResolutionContext;
use super::section_context::SectionContext;

pub(crate) struct LiveH1State {
    pub(crate) models: ModelBindings,
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
    );
    // Construction and limits failures propagate bare, before any teardown
    // observation exists; every failure past this point tears the frame's
    // VM down exactly once.
    let mut h1_frame = SectionContext::new_live_h1(ctx, &runtime, client)?;
    macro_rules! h1_try {
        ($expression:expr) => {
            match $expression {
                Ok(value) => value,
                Err(error) => {
                    h1_frame.teardown(&ctx.prompt().title);
                    return Err(error);
                }
            }
        };
    }
    // The pass owns its client slot: seeded from the run's client, created
    // lazily on the first prose block, whose construction error propagates
    // through `h1_try!`.
    let mut active_client = client.cloned();
    let flow = h1_try!(
        h1_frame
            .run(
                ctx,
                &ctx.prompt().title,
                &ctx.prompt().h1_blocks,
                BlockRunMode::LiveH1(&runtime),
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
            h1_frame.teardown(&ctx.prompt().title);
            return Err(Error::Internal("live H1 block walk reported a jump"));
        }
    };
    let var = h1_try!(h1_frame.read_var());
    // The tool bindings need no extraction: H1's binds already landed in
    // the run's shared tool set, which the walk reads through the view.
    let models = h1_try!(runtime.models());
    h1_frame.teardown(&ctx.prompt().title);
    Ok(LiveH1State {
        models,
        var,
        returned,
        reply,
    })
}
