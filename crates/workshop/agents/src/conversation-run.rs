//! What a conversation hands its run's Harness, and how it takes the
//! run's report: the recorder tee, the per-run services with the
//! conversation's input broker, the delta sender for each of the run's
//! own rounds, and the drive that ends the conversation with its run.

use std::sync::Arc;

use harness::capability::{HostServices, INPUT_BROKER, InputBroker};
use harness::record::{RunOutcome, RunRecorder};
use harness::{Harness, HarnessError, OnDelta, RunReport, RunRequest, display_chain};
use promptforge::ids::RoundId;
use promptforge::model::StreamDelta;
use tokio::sync::broadcast;

use super::{Conversation, lock};
use crate::input::SessionInputBroker;
use crate::protocol::{Delta, DeltaKind};
use crate::state::{FailureKind, SessionState};
use crate::tee::ConversationRecorder;

/// The report of a run a close or an uncaught stop cut short.
const INTERRUPTED: &str = "the agent run was interrupted";

impl Conversation {
    /// The recorder the run writes through: every call goes on to
    /// `inner`, the run's metadata names this conversation's agent, and
    /// each event `inner` accepted reaches this conversation.
    #[must_use]
    pub fn recorder(&self, inner: Arc<dyn RunRecorder>) -> Arc<dyn RunRecorder> {
        Arc::new(ConversationRecorder::new(self.clone(), inner))
    }

    /// The run's services: a clone of `base` with this conversation's
    /// input broker supplied under [`INPUT_BROKER`]. `base` leaves
    /// [`INPUT_BROKER`] empty, because a provider refuses a duplicate; a
    /// `base` that holds one keeps it, and the refusal is logged.
    #[must_use]
    pub fn services(&self, base: &HostServices) -> HostServices {
        let frames = self
            .channels()
            .map_or_else(|| broadcast::channel(1).0, |channels| channels.waits);
        let broker: Arc<dyn InputBroker> = Arc::new(SessionInputBroker::new(
            Arc::clone(&self.core.waits),
            frames,
        ));
        let mut services = base.clone();
        if let Err(error) = services.provide(&INPUT_BROKER, broker) {
            tracing::warn!(
                conversation = %self.core.id,
                %error,
                "the base services already hold an input broker; the conversation's is not supplied"
            );
        }
        services
    }

    /// The live-piece callback for the run's round `round`: each piece
    /// goes out as a [`Delta`] stamped with the round's id, the id the
    /// round's reply event carries. No client listening is not a failure:
    /// the completed reply travels in the round's answer.
    #[must_use]
    pub fn delta_sender(&self, round: RoundId) -> OnDelta {
        let conversation = self.clone();
        Arc::new(move |piece| conversation.publish_delta(round, piece))
    }

    /// Drives the conversation's one run to its end: holds `harness`'s
    /// control, so a stop or a close reaches the run, awaits the run over
    /// `request`, reports how it ended, and ends the conversation.
    pub async fn run(&self, harness: Harness, request: RunRequest) {
        {
            let mut control = lock(&self.core.control);
            let held = control.insert(harness.control());
            if self.state() != SessionState::Alive {
                held.cancel();
            }
        }
        let result = harness.run(request).await;
        self.take_report(result);
        self.end();
    }

    /// Notes the report's run and reports a run that failed or was cut
    /// short; a completed run reports nothing.
    fn take_report(&self, result: Result<RunReport, HarnessError>) {
        let message = match result {
            Ok(report) => {
                if let Some(run) = report.run_id {
                    self.note_run(run);
                }
                match report.outcome {
                    RunOutcome::Completed { .. } => return,
                    RunOutcome::Cancelled => {
                        self.report(FailureKind::Interrupted, INTERRUPTED.to_owned());
                        return;
                    }
                    RunOutcome::Failed { message, .. } => message,
                }
            }
            Err(error) => display_chain(&error),
        };
        tracing::warn!(
            conversation = %self.core.id,
            agent = %self.core.agent,
            %message,
            "agent run failed"
        );
        self.report(FailureKind::RunFailed, message);
    }

    /// Stamps one live piece with its round and broadcasts it.
    fn publish_delta(&self, round: RoundId, piece: StreamDelta) {
        let (kind, content) = match piece {
            StreamDelta::Text(text) => (DeltaKind::Text, text),
            StreamDelta::Reasoning(text) => (DeltaKind::Reasoning, text),
            // The enum is non-exhaustive across the crate seam; a future
            // side channel has no delta kind yet and stays live-only.
            _ => return,
        };
        if let Some(channels) = self.channels() {
            // No receiver means no client is attached; deltas are
            // ephemeral and the completed-reply event is the repair.
            let _ = channels.deltas.send(Delta {
                kind,
                content,
                reply: round.get(),
            });
        }
    }
}
