//! One agent run on the engine, driven by the tokio test driver: the
//! interim host of Workshop's agent sessions until the harness lands.
//!
//! The session is the engine's host here. It activates the prompt's
//! declared capabilities against the shared registry, prepares the run
//! context, refuses an unsatisfiable prompt with the engine's
//! model-readable notice, and drives the [`Run`] through
//! [`drive_tokio`] with three performers built as closures over the
//! session's resources: the gateway client (with the session's delta
//! stamp as the streaming hook), the activated tool table, and the
//! session's input broker. Every event the run reports goes through the
//! session's [`SessionSink`], which appends the transcript kinds to the
//! session's memory log and owns the side effects the session wires to
//! them.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_api::bridge::{
    CapabilityRegistry, GatewayClient as ModelClient, InputBroker, RunServices, ToolTable, activate,
};
use harness_api::cancel::CancelHandle;
use promptforge_api_runtime::test_support::{Performers, drive_tokio};
use promptforge_api_runtime::{
    Effect, EffectAnswer, Environment, Prompt, Run, RunContext, RunLimits, RunResult,
};
use promptforge_api_types::cancel::CancelHandle as CancelFlag;
use promptforge_api_types::models::ModelDescriptor;
use promptforge_api_types::timestamp::Timestamp;
use promptforge_api_types::wire::StreamDelta;

use super::session::SessionSink;
use super::supervisor::AgentRunError;

/// The owned pieces one run needs beyond its source: the session's sink
/// and broker, the launch-time host inputs, and the model client.
pub(super) struct RunParts {
    /// The session's event sink: the memory log and the side effects.
    pub(super) sink: SessionSink,
    /// The broker `UserInput` effects wait on: the session's wait registry
    /// behind [`SessionInputBroker`](crate::input::SessionInputBroker).
    pub(super) broker: Arc<dyn InputBroker>,
    /// The `ui()` snapshot taken at launch.
    pub(super) ui: serde_json::Value,
    /// The run's seed, drawn at launch from the OS CSPRNG.
    pub(super) seed: u64,
    /// The launch instant, every section's `sys.when`.
    pub(super) started_at: Timestamp,
    /// The live streaming-delta hook, stamped with the session's round.
    pub(super) on_delta: Arc<dyn Fn(StreamDelta) + Send + Sync>,
    /// The dropdown's current model resolved at launch, when one is
    /// selected.
    pub(super) model: Option<ModelDescriptor>,
    /// The run's execution identifier: the session id.
    pub(super) execution: String,
    /// The awaitable cancel token the session armed for this run.
    pub(super) cancel: CancelHandle,
}

/// Runs one Markdown agent prompt: parses it, activates the first-party
/// capabilities its frontmatter declares against `registry`, prepares the
/// context with the launch-time model, refuses an unsatisfiable prompt,
/// and drives the run on the tokio driver until it ends or the session's
/// cancel token fires.
///
/// # Errors
/// Returns [`AgentRunError::Interrupted`] when the run was cancelled and
/// [`AgentRunError::Failed`] for a parse failure, a refusal, or a run
/// failure, each with an operator-facing message.
pub(super) async fn run_markdown_agent(
    source: &str,
    parts: RunParts,
    client: ModelClient,
    registry: Arc<CapabilityRegistry>,
) -> Result<(), AgentRunError> {
    let RunParts {
        sink,
        broker,
        ui,
        seed,
        started_at,
        on_delta,
        model,
        execution,
        cancel: token,
    } = parts;
    // The parse-time events land in the session's log ahead of the run's,
    // whether or not the parse succeeds.
    let (prompt, parse_events) = Prompt::parse(source, &execution);
    let mut sink = sink.into_fn();
    for event in parse_events {
        sink(event);
    }
    let prompt = prompt.map_err(|error| AgentRunError::Failed {
        message: format!("the embedded Markdown agent failed to parse: {error}"),
        source: Some(Box::new(error)),
    })?;
    let (cancel, bridge) = bridge_cancel(token);
    let mut ctx = RunContext::new(execution, seed, started_at)
        .cancel(cancel.clone())
        .ui(ui);
    if let Some(model) = model {
        ctx = ctx.model(model);
    }
    // The host's activate-prepare-refuse path: the run's VFS is built
    // first so the capabilities' services and the run share one store;
    // the activated catalog is what prepare fills slots against, and the
    // implementations stay here for the tool performer.
    let env = Environment::new();
    let ctx = ctx.vfs(env.run_vfs());
    let services = RunServices::new(ctx.vfs_handle().clone(), ctx.cancel_handle());
    let activation = activate(Some(&registry), &prompt, &services);
    let env = env.tools(activation.catalog);
    let (ctx, mut requirements) = env.prepare(&prompt, ctx);
    requirements.merge(activation.requirements);
    let outcome = if let Some(refusal) = requirements.refusal() {
        RunResult::Failure(refusal)
    } else {
        let performers = performers(client, on_delta, activation.tools, broker);
        let run = Run::new(Arc::new(prompt), "", ctx);
        drive_tokio(run, performers, sink, cancel).await
    };
    // The run is over, so nothing reads the flag: the bridge ends with it.
    bridge.abort();
    match outcome {
        RunResult::Ok(_output) => Ok(()),
        RunResult::Cancelled => Err(AgentRunError::Interrupted),
        RunResult::Failure(error) => Err(AgentRunError::Failed {
            message: error.to_string(),
            source: Some(Box::new(error)),
        }),
    }
}

/// The session's performers: a `Chat` runs on the gateway client under
/// the default request limits with the session's delta stamp as the
/// streaming hook, a `ToolCall` resolves its id in the activated table,
/// and a `UserInput` waits on the session's broker.
fn performers(
    client: ModelClient,
    on_delta: Arc<dyn Fn(StreamDelta) + Send + Sync>,
    tools: ToolTable,
    broker: Arc<dyn InputBroker>,
) -> Performers {
    let limits = RunLimits::new();
    let client = client.with_request_limits(limits.timeout(), limits.response_bytes());
    Performers {
        chat: Box::new(move |effect| {
            let client = client.clone();
            let on_delta = Arc::clone(&on_delta);
            Box::pin(async move {
                let Effect::Chat {
                    messages,
                    tools,
                    options,
                    stream,
                    ..
                } = effect
                else {
                    return EffectAnswer::Dropped;
                };
                // Only a section's own rounds stream to the SPA; a nested
                // infer's chunks have no consumer and drop at the leaf.
                let tool_arg = (!tools.is_empty()).then_some(tools.as_slice());
                let result = client
                    .complete(&messages, tool_arg, &options, |delta| {
                        if stream {
                            on_delta(delta);
                        }
                    })
                    .await
                    .map(Box::new);
                EffectAnswer::Chat(result)
            })
        }),
        tool_call: Box::new(move |effect| {
            let tools = tools.clone();
            Box::pin(async move {
                let Effect::ToolCall { tool, args, .. } = effect else {
                    return EffectAnswer::Dropped;
                };
                let Some(tool) = tools.get(&tool) else {
                    return EffectAnswer::ToolCall(Err(
                        promptforge_api_types::tools::ToolError::message(
                            "the tool the call names is not among the session's activated \
                             capabilities",
                        ),
                    ));
                };
                EffectAnswer::ToolCall(tool.call(args).await)
            })
        }),
        user_input: Box::new(move |effect| {
            let broker = Arc::clone(&broker);
            Box::pin(async move {
                let Effect::UserInput { execution, section } = effect else {
                    return EffectAnswer::Dropped;
                };
                EffectAnswer::UserInput(broker.user_input(&execution, &section).await)
            })
        }),
    }
}

/// The system clock now as the engine's `Timestamp`: the host's stamp for
/// a run's `started_at`, since the engine reads no clock of its own. A
/// clock before the epoch or beyond `i64` milliseconds (neither reachable
/// on a real host) saturates to the epoch rather than refusing the launch.
pub(super) fn now_timestamp() -> Timestamp {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .map_or(Timestamp::UNIX_EPOCH, Timestamp::from_unix_millis)
}

/// Bridges the session's awaitable cancel token to the synchronous flag
/// the engine polls and the driver awaits: the flag is set the moment the
/// token fires. The caller aborts the returned bridge task once the run is
/// over, so a run that finishes uncancelled leaves no task waiting on a
/// token nobody will fire.
fn bridge_cancel(token: CancelHandle) -> (CancelFlag, tokio::task::JoinHandle<()>) {
    let flag = CancelFlag::new();
    let bridged = flag.clone();
    let bridge = tokio::spawn(async move {
        token.cancelled().await;
        bridged.cancel();
    });
    (flag, bridge)
}

#[cfg(test)]
#[path = "run-tests.rs"]
mod tests;
