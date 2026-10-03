//! One run of a session's program: arm the run's cancel flag, wait for the
//! Host's broker to list its models and read the Host snapshot once it
//! answers (a cancel while the broker holds its list ends the run as
//! cancelled), then run a Harness of its own over the session's recorder
//! tee, the session's broker, the tokio timer, a clone of the Host's
//! registry, and a clone of the Host's services with the session's input
//! broker supplied. The session's cancel flag reaches the run through its
//! `RunControl::cancel`.
//!
//! Every event the run records reaches the session core through the tee
//! once the Host's recorder has taken it, so the recorder, the live
//! broadcast, and the transcript agree event for event. A section's own
//! model rounds publish their live pieces to the session through the
//! session's broker as they arrive. The run is polled inside the
//! supervisor's task, and a round's pieces arrive before its answer is
//! applied, so each piece is published ahead of the reply event that
//! supersedes it.

use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};

use harness_capabilities::{HostServices, INPUT_BROKER, InputBroker};
use harness_runner::performers::{BoxFuture, InferenceBroker, OnDelta};
use harness_runner::recorder::RunOutcome;
use harness_runner::{Harness, HarnessError, RunRequest};
use promptforge::effect::Round;
use promptforge::event::ReplyOrigin;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};

use crate::discovery::AgentSource;
use crate::environment::Bindings;
use crate::input::SessionInputBroker;
use crate::timer::TokioTimer;

use super::SessionCore;
use super::tee::SessionRecorder;

/// Runs the session's program once and reports how it ended. `bindings`
/// is read for the Host snapshot once the broker has listed its models,
/// so a broker that holds its list until it has a model to offer starts
/// the run under the selection made while it waited, and a selection or a
/// revoke reaches the next relaunch.
pub(crate) async fn run_once(
    core: Arc<SessionCore>,
    bindings: Arc<Bindings>,
) -> Result<RunOutcome, HarnessError> {
    let cancel = core.arm_cancel();
    let listed = tokio::select! {
        biased;
        () = cancel.cancelled() => return Ok(RunOutcome::Cancelled),
        listed = core.broker.models() => listed,
    };
    let host = bindings.host();
    let harness = Harness::new(
        Arc::new(SessionRecorder::new(Arc::clone(&core))),
        Arc::new(SessionBroker {
            inner: Arc::clone(&core.broker),
            core: Arc::clone(&core),
            listed: Mutex::new(Some(listed)),
        }),
        Arc::new(TokioTimer),
        (*core.capabilities).clone(),
        run_services(&core),
    );
    let control = harness.control();
    let AgentSource::Markdown(source) = &core.source;
    let request = RunRequest {
        name: core.id.as_str().to_owned(),
        source: source.clone(),
        args: core.args.clone(),
        input_text: core.files.input_text(),
        vfs: core.files.run_vfs(),
        host,
    };
    let mut run = pin!(harness.run(request));
    let finished = tokio::select! {
        biased;
        () = cancel.cancelled() => None,
        report = run.as_mut() => Some(report),
    };
    let report = if let Some(report) = finished {
        report
    } else {
        control.cancel();
        run.await
    }?;
    core.files.collect(report.output);
    Ok(report.outcome)
}

/// The run's services: a clone of the Host's with the session's input
/// broker supplied under [`INPUT_BROKER`]. A Host whose own services
/// already hold an input broker keeps it, and the refusal is logged.
fn run_services(core: &SessionCore) -> HostServices {
    let mut services = core.services.clone();
    let broker: Arc<dyn InputBroker> = Arc::new(SessionInputBroker::new(
        Arc::clone(&core.waits),
        core.wait_frames.clone(),
    ));
    if let Err(error) = services.provide(&INPUT_BROKER, broker) {
        tracing::warn!(
            session = %core.id,
            %error,
            "the Host's services already hold an input broker; the session's is not supplied"
        );
    }
    services
}

/// The session's broker over the Host's: a section's own round publishes
/// its live pieces to the session, the run's model list is the one the
/// session already waited for, and every other call goes through as it
/// came.
struct SessionBroker {
    inner: Arc<dyn InferenceBroker>,
    core: Arc<SessionCore>,
    /// The listing the session read the Host snapshot after, taken by the
    /// run's own listing so the Host's broker is asked once per run.
    listed: Mutex<Option<Result<ModelCatalog, CompletionError>>>,
}

impl InferenceBroker for SessionBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let listed = self
            .listed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match listed {
            Some(listed) => Box::pin(async move { listed }),
            None => self.inner.models(),
        }
    }

    fn chat(
        &self,
        binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        round: Round,
        _on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        // Only a section's own round has a live consumer; a nested
        // `models.infer` round is read whole from its answer.
        let on_delta =
            (round.origin == ReplyOrigin::Chat).then(|| delta_callback(Arc::clone(&self.core)));
        self.inner
            .chat(binding, messages, tools, options, round, on_delta)
    }
}

/// The run's delta callback: each delta is stamped with the session's
/// current round and broadcast. No client listening is not a failure:
/// the completed reply travels in the effect's answer.
fn delta_callback(core: Arc<SessionCore>) -> OnDelta {
    Arc::new(move |delta| core.publish_delta(delta))
}

#[cfg(test)]
#[path = "run-tests.rs"]
mod tests;
