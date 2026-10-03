//! One run of a session's program on the effect loop: resolve the
//! client's current model through the Host's broker, arm the run's cancel
//! flag, prepare the run over the broker and the session's delta callback
//! (beginning it at the Host's recorder and staging the declared input
//! file), drive it to its end, and read the declared output file once it
//! completes.
//!
//! Every event the run reports goes through the session core's sink once
//! the recorder has taken it, so the recorder, the live broadcast, and the
//! transcript agree event for event. A run that ends before the loop sees
//! it - a parse failure or a refusal - carries the events it recorded in
//! its error; the session observes them from there, so the transcript
//! holds them too.

use std::sync::Arc;

use harness_runner::effect_loop::{DriveError, drive_run};
use harness_runner::performers::OnDelta;
use harness_runner::prepare::{PrepareError, Services, prepare_source};
use harness_runner::recorder::{RunOutcome, RunRecorder};
use promptforge::event::Event;
use promptforge::model::StreamDelta;
use tokio::sync::mpsc;

use crate::discovery::AgentSource;
use crate::environment::{CurrentModelError, HostSnapshot, current_model};
use crate::input::SessionInputBroker;
use crate::transition::RunId;

use super::SessionCore;

/// Why one run produced no outcome of the Engine's.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RunFailure {
    /// The client's selected model could not be resolved; the resolution
    /// failure is the source.
    #[error("the chat cannot launch")]
    Model(#[source] CurrentModelError),
    /// The run could not be prepared: the prompt does not parse, the
    /// environment cannot satisfy it, or the recorder refused it. Renders
    /// and sources as the preparation error does.
    #[error(transparent)]
    Prepare(Box<PrepareError>),
    /// The effect loop stopped without an outcome. Renders and sources as
    /// the drive error does.
    #[error(transparent)]
    Drive(DriveError),
}

/// What one run needs beyond the session: the reducer's identity for it
/// and the Host state read at launch.
pub(crate) struct RunInputs {
    /// The reducer's identity for the run.
    pub(crate) run: RunId,
    /// The Host snapshot read at launch.
    pub(crate) host: HostSnapshot,
}

/// Runs the session's program once under `inputs` and reports how it
/// ended.
pub(crate) async fn run_once(
    core: Arc<SessionCore>,
    inputs: RunInputs,
) -> Result<RunOutcome, RunFailure> {
    let RunInputs { run, host } = inputs;
    let model = current_model(&host, &*core.broker)
        .await
        .map_err(RunFailure::Model)?;
    let cancel = core.arm_cancel(run);
    let on_delta = delta_callback(core.delta_source.clone());
    let vfs = core.files.run_vfs();
    let recorder: Arc<dyn RunRecorder> = Arc::clone(&core.recorder);
    let services = Services {
        registry: Some(Arc::clone(&core.capabilities)),
        services: core.services.clone(),
        vfs: vfs.clone(),
        input_text: core.files.input_text(),
        cancel: cancel.clone(),
        recorder: Arc::clone(&recorder),
        broker: Arc::clone(&core.broker),
        on_delta,
        input: Some(Arc::new(SessionInputBroker::new(
            Arc::clone(&core.waits),
            core.wait_frames.clone(),
        ))),
        session_id: core.id.as_str().to_owned(),
        agent: core.agent.clone(),
        model,
        ui: Some(host.ui()),
    };
    let AgentSource::Markdown(source) = &core.source;
    let prepared = match prepare_source(source, &core.prompt_path, &core.args, services).await {
        Ok(prepared) => prepared,
        Err(error) => {
            observe_failed_prepare(&core, &error);
            return Err(RunFailure::Prepare(Box::new(error)));
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
    let outcome = drive_run(
        prepared.run,
        prepared.performers,
        recorder,
        prepared.run_id,
        cancel,
        sink,
    )
    .await
    .map_err(RunFailure::Drive)?;
    if matches!(outcome, RunOutcome::Completed { .. }) {
        core.files
            .collect(&core.agent, vfs, prepared.output_path)
            .await;
    }
    Ok(outcome)
}

/// The run's delta callback: each delta goes to the session's delta
/// channel.
fn delta_callback(deltas: mpsc::UnboundedSender<StreamDelta>) -> OnDelta {
    // A closed receiver is a session that stopped listening, not a
    // failure: the completed reply travels in the effect's answer.
    Arc::new(move |delta| {
        let _ = deltas.send(delta);
    })
}

/// Notes the run a failed preparation began and ended, and hands the sink
/// the events that run recorded: the parse-time events of a run that ended
/// before the loop saw it.
fn observe_failed_prepare(core: &SessionCore, error: &PrepareError) {
    let (run_id, events) = match error {
        PrepareError::Parse { run_id, events, .. }
        | PrepareError::Input { run_id, events, .. }
        | PrepareError::Refused { run_id, events, .. } => (*run_id, events),
        // `Read`, `Recorder`, or a variant `harness-runner` adds behind its
        // `#[non_exhaustive]` `PrepareError`: none of them names a run.
        _ => return,
    };
    core.record_run(run_id);
    for event in events {
        core.observe(event);
    }
}

#[cfg(test)]
#[path = "run-tests.rs"]
mod tests;
