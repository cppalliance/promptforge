//! What a conversation hands its run's Harness, and how it takes the
//! run's report: the recorder tee, the run's own services holding the
//! conversation's input broker, the live pieces of the run's own rounds,
//! and the drive that ends the conversation with its run.

use std::sync::Arc;

use harness::plugin::HostServices;
use harness::record::{RunOutcome, RunRecorder};
use harness::{Harness, HarnessError, RunReport, RunRequest, display_chain};
use plugin_user_input::{INPUT_BROKER, InputBroker};
use promptforge::ids::RoundId;
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
    /// `inner`, and each event `inner` accepted reaches this
    /// conversation.
    #[must_use]
    pub fn recorder(&self, inner: Arc<dyn RunRecorder>) -> Arc<dyn RunRecorder> {
        Arc::new(ConversationRecorder::new(self.clone(), inner))
    }

    /// The run's own services: this conversation's input broker, supplied
    /// under [`INPUT_BROKER`], and nothing else.
    #[must_use]
    pub fn run_services(&self) -> HostServices {
        let frames = self
            .channels()
            .map_or_else(|| broadcast::channel(1).0, |channels| channels.waits);
        let broker: Arc<dyn InputBroker> = Arc::new(SessionInputBroker::new(
            Arc::clone(&self.core.waits),
            frames,
        ));
        let mut services = HostServices::new();
        if let Err(error) = services.provide(&INPUT_BROKER, broker) {
            // An empty map takes the broker's valid literal id, so this
            // refusal is defensive.
            tracing::warn!(
                conversation = %self.core.id,
                %error,
                "the conversation's input broker was refused; the run has no operator"
            );
        }
        services
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

    /// Broadcasts one live piece of the run's round `round` as a
    /// [`Delta`] of `kind`, stamped with the round's id, the id the
    /// round's reply event carries. No client listening is not a failure:
    /// the completed reply travels in the round's answer.
    pub fn publish_delta(&self, round: RoundId, kind: DeltaKind, content: String) {
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
