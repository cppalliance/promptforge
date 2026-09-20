//! One run of a session's program on the effect loop: resolve the
//! client's current model, arm the run's cancel flag, build the session's
//! performers, prepare the run (opening its row in the log), and drive it
//! to its end.
//!
//! Every event the run reports goes through the session core's sink once
//! the log has recorded it, so the live broadcast and the transcript read
//! from the log agree index for index. A run that ends before the loop
//! sees it - a parse failure or a refusal - has its parse-time events in
//! the log already; they are replayed into the sink from there, so the
//! two sides agree for that run too.

use std::sync::Arc;

use harness_capabilities::CapabilityRegistry;
use harness_log::{RunId as LogRunId, RunOutcome};
use harness_models::{GatewayChatPerformer, GatewayClient};
use harness_runner::effect_loop::{DriveError, drive_run};
use harness_runner::prepare::{PrepareError, Services, prepare_source};
use promptforge_api_runtime::RunLimits;
use promptforge_api_types::event::Event;

use crate::discovery::AgentSource;
use crate::environment::{
    CatalogBinding, CurrentModelError, GatewayResources, HostSnapshot, current_model,
};
use crate::input::SessionInputBroker;
use crate::transition::RunId;

use super::SessionCore;

/// Why one run produced no outcome of the engine's.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RunFailure {
    /// The client's selected model could not be resolved; the resolution
    /// failure is the source.
    #[error("the chat cannot launch")]
    Model(#[source] CurrentModelError),
    /// The run could not be prepared: the prompt does not parse, the
    /// environment cannot satisfy it, or the log refused it. Renders and
    /// sources as the preparation error does.
    #[error(transparent)]
    Prepare(PrepareError),
    /// The effect loop stopped without an outcome. Renders and sources as
    /// the drive error does.
    #[error(transparent)]
    Drive(DriveError),
}

/// What one run needs beyond the session: the frozen bindings the reducer
/// selected for it.
pub(crate) struct RunInputs {
    /// The reducer's identity for the run.
    pub(crate) run: RunId,
    /// The gateway generation the run is frozen to.
    pub(crate) gateway: Arc<GatewayResources>,
    /// The model client built for that generation.
    pub(crate) client: GatewayClient,
    /// The capability registry built for that generation, when the
    /// binding could build one.
    pub(crate) registry: Option<Arc<CapabilityRegistry>>,
    /// The catalog generation the run is frozen to.
    pub(crate) catalog: Option<CatalogBinding>,
    /// The host snapshot read at launch.
    pub(crate) host: HostSnapshot,
}

/// Runs the session's program once under `inputs` and reports how it
/// ended.
pub(crate) async fn run_once(
    core: Arc<SessionCore>,
    inputs: RunInputs,
) -> Result<RunOutcome, RunFailure> {
    let RunInputs {
        run,
        gateway,
        client,
        registry,
        catalog,
        host,
    } = inputs;
    let model = current_model(&host, catalog.as_ref(), gateway.binding())
        .await
        .map_err(RunFailure::Model)?;
    let cancel = core.arm_cancel(run);
    let limits = RunLimits::new();
    let client = client.with_request_limits(limits.timeout(), limits.response_bytes());
    let services = Services {
        registry,
        vfs: shared_vfs::VfsRef::builder().build(),
        cancel: cancel.clone(),
        log: Arc::clone(&core.log),
        chat: Arc::new(GatewayChatPerformer::new(client, core.delta_source.clone())),
        input: Arc::new(SessionInputBroker::new(
            Arc::clone(&core.waits),
            core.wait_frames.clone(),
        )),
        session_id: core.id.as_str().to_owned(),
        agent: core.agent.clone(),
        model,
        ui: Some(host.ui()),
    };
    let AgentSource::Markdown(source) = &core.source;
    let prepared = match prepare_source(source, &core.prompt_path, &core.args, services).await {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Some(run_id) = opened_run(&error) {
                core.record_run(run_id);
                replay_recorded(&core, run_id).await;
            }
            return Err(RunFailure::Prepare(error));
        }
    };
    core.record_run(prepared.run_id);
    for event in &prepared.parse_events {
        core.observe(event);
    }
    let sink = {
        let core = Arc::clone(&core);
        move |event: Event| core.observe(&event)
    };
    drive_run(
        prepared.run,
        prepared.performers,
        Arc::clone(&core.log),
        prepared.run_id,
        cancel,
        sink,
    )
    .await
    .map_err(RunFailure::Drive)
}

/// The row a failed preparation opened and closed, when it opened one.
fn opened_run(error: &PrepareError) -> Option<LogRunId> {
    match error {
        PrepareError::Parse { run_id, .. } | PrepareError::Refused { run_id, .. } => Some(*run_id),
        // `Read`, `Log`, or a variant `harness-runner` adds behind its
        // `#[non_exhaustive]` `PrepareError`: none of them opened a row.
        _ => None,
    }
}

/// Hands the events the log already holds for `run_id` to the sink: the
/// parse-time events of a run that ended before the loop saw it.
async fn replay_recorded(core: &SessionCore, run_id: LogRunId) {
    let records = match core.log.lock().await.transcript(run_id).await {
        Ok(records) => records,
        Err(error) => {
            tracing::error!(%error, run = %run_id, "the run log refused a transcript read");
            return;
        }
    };
    for stored in records {
        match serde_json::from_value::<Event>(stored.record.payload) {
            Ok(event) => core.observe(&event),
            Err(error) => {
                tracing::error!(%error, run = %run_id, "a stored event payload does not parse");
            }
        }
    }
}

#[cfg(test)]
#[path = "run-tests.rs"]
mod tests;
