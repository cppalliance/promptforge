//! Agent-run supervision across cancellation and catalog generations.

use std::sync::Arc;

use promptforge_tools::{Tool, ToolCatalog};
use tokio::sync::mpsc;

use crate::gateway_binding::GatewayBinding;
use crate::input::UserInputTool;

use super::{AgentSession, AgentSessions, SessionHost};
mod catalog;
mod effects;
mod events;
pub(super) mod transition;
use effects::{EffectExecutor, EffectOutcome};
use events::{CollectedEvent, EventCollector};
use transition::{SupervisorEvent, SupervisorState, transition};

/// Spawns one session supervisor. Each run freezes one usable chat
/// catalog; cancellation or a genuinely new usable generation relaunches
/// over the retained event log.
pub(super) fn spawn(
    session: Arc<AgentSession>,
    registry: AgentSessions,
    host: SessionHost,
    gateway: GatewayBinding,
    lifecycle: mpsc::UnboundedReceiver<SupervisorEvent>,
) {
    tokio::spawn(async move {
        let tool: Arc<dyn Tool> = Arc::new(UserInputTool::new(
            Arc::clone(&session.waits),
            session.input_frames.clone(),
        ));
        let tools = match ToolCatalog::new(&[tool]) {
            Ok(tools) => tools,
            Err(error) => {
                tracing::error!(%error, session = %session.id, "agent tool catalog refused");
                registry.forget(&session.id);
                return;
            }
        };
        let (mut collector, initial_catalog, initial_gateway) =
            EventCollector::new(lifecycle, host.catalog.clone(), gateway);
        let mut executor = EffectExecutor::new(
            Arc::clone(&session),
            host,
            tools,
            initial_catalog.snapshot,
            Arc::clone(&initial_gateway),
        );
        let mut state = SupervisorState::new(initial_gateway.generation());
        let mut pending_event = Some(initial_catalog.event);

        loop {
            let collected = match pending_event.take() {
                Some(event) => CollectedEvent::Supervisor(event),
                None => executor.next_event(&mut collector).await,
            };
            let event = executor.event_from(collected);
            let next = transition(state, event);
            state = next.state;
            match executor.execute(next.effect) {
                EffectOutcome::Continue => {}
                EffectOutcome::Event(event) => pending_event = Some(event),
                EffectOutcome::Close => break,
            }
        }
        registry.forget(&session.id);
    });
}
