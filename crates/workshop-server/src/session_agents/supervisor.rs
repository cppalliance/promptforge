//! Agent-run supervision across cancellation and catalog generations.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use promptforge_agent::{AgentConfig, AgentError, AgentLimits, run_agent_with_client};
use promptforge_core_support::observe::Observer;
use promptforge_store::StoreRef;
use promptforge_tools::{Tool, ToolCatalog};

use crate::gateway_binding::GatewayBinding;
use crate::input::UserInputTool;
use crate::protocol::Activity;

use super::{
    AgentSession, AgentSessions, CancelOrigin, SessionHost, SessionObserver, build_model_catalog,
    delta_stamp, ui_provider,
};

mod catalog;
use catalog::{wait_for_chat_catalog, wait_for_replacement_catalog};

/// Spawns one session supervisor. Each run freezes one usable chat
/// catalog; cancellation or a genuinely new usable generation relaunches
/// over the retained event log.
pub(super) fn spawn(
    session: Arc<AgentSession>,
    registry: AgentSessions,
    host: SessionHost,
    gateway: GatewayBinding,
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
        let store = StoreRef::memory();
        let observer = observer(&session, &host);
        let on_delta = delta_stamp(&session, &host.push);
        let ui = ui_provider(&host.menu, &host.workspace);
        let mut catalog_generation = host.catalog.subscribe_chat_generation();
        let mut gateway_generation = gateway.subscribe();
        loop {
            let Some(chat_catalog) =
                wait_for_chat_catalog(&session, &host.catalog, &mut catalog_generation).await
            else {
                break;
            };
            let active_generation = chat_catalog.generation;
            let active_models = chat_catalog.models;
            let models = build_model_catalog(Some(active_models.clone()));
            let gateway_snapshot = gateway.snapshot();
            let active_gateway_generation = gateway_snapshot.generation();
            let Some(client) = gateway_snapshot.model_client() else {
                let message = "the replacement Gateway credentials cannot make a model client";
                let _ = session.errors.send(message.to_owned());
                host.push
                    .push_failure("Agent failed", message, Activity::General);
                break;
            };
            let run_cancel = session.arm_cancel();
            let config = AgentConfig {
                name: session.agent.clone(),
                execution: session.id.clone(),
                observer: Arc::clone(&observer),
                cancel: run_cancel.clone(),
                event_log: Some(Arc::clone(&session.log) as _),
                on_delta: Some(Arc::clone(&on_delta)),
                ui: Some(Arc::clone(&ui)),
                limits: AgentLimits::default(),
            };
            let run = run_agent_with_client(
                &session.source,
                &tools,
                &models,
                &store,
                config,
                Some(client.clone()),
            );
            tokio::pin!(run);
            let result = tokio::select! {
                result = &mut run => result,
                replacement = wait_for_replacement_catalog(
                    &host.catalog,
                    &mut catalog_generation,
                    active_generation,
                    &active_models,
                ) => {
                    if replacement.is_none() {
                        run.await
                    } else {
                        loop {
                            if session.cancel_for_catalog() {
                                break run.await;
                            }
                            tokio::select! {
                                result = &mut run => break result,
                                () = session.wait_until_turn_settled() => {}
                            }
                        }
                    }
                }
                replaced = wait_for_gateway_replacement(
                    &mut gateway_generation,
                    active_gateway_generation,
                ) => {
                    if replaced {
                        session.cancel_for_gateway();
                    }
                    run.await
                }
            };
            if run_finished(result, &session, &host) {
                break;
            }
        }
        registry.forget(&session.id);
    });
}

/// Reports one run ending and answers whether the supervisor is finished.
fn run_finished(
    result: Result<(), AgentError>,
    session: &AgentSession,
    host: &SessionHost,
) -> bool {
    match (result, session.cancel_origin()) {
        (Err(AgentError::Interrupted), _) if !session.closing.load(Ordering::SeqCst) => {
            report_cancel_origin(session);
            false
        }
        (Err(AgentError::Interrupted) | Ok(()), _) => true,
        (Err(error), _) => {
            tracing::warn!(
                %error,
                session = %session.id,
                agent = %session.agent,
                "agent run failed"
            );
            let _ = session.errors.send(error.to_string());
            host.push
                .push_failure("Agent failed", error.to_string(), Activity::General);
            true
        }
    }
}

/// Builds the observer shared by every generation of one session.
fn observer(session: &AgentSession, host: &SessionHost) -> Arc<dyn Observer> {
    Arc::new(SessionObserver {
        log: Arc::clone(&session.log),
        rounds: Arc::clone(&session.rounds),
        push: host.push.clone(),
        backoff: host.backoff.clone(),
        errors: session.errors.clone(),
        lifecycle: Arc::clone(&session.lifecycle),
    })
}

/// Records catalog retirement separately from explicit operator cancellation.
fn report_cancel_origin(session: &AgentSession) {
    match session.cancel_origin() {
        Some(CancelOrigin::Operator) => {}
        Some(CancelOrigin::Catalog) => tracing::debug!(
            session = %session.id,
            "agent run retired for a new catalog generation"
        ),
        Some(CancelOrigin::Gateway) => tracing::debug!(
            session = %session.id,
            "agent run retired for a new gateway generation"
        ),
        None => tracing::debug!(
            session = %session.id,
            "agent run interrupted without a supervisor cancellation origin"
        ),
    }
}

/// Waits until the host publishes a different Gateway generation.
async fn wait_for_gateway_replacement(
    generation: &mut tokio::sync::watch::Receiver<u64>,
    active: u64,
) -> bool {
    loop {
        if *generation.borrow_and_update() != active {
            return true;
        }
        if generation.changed().await.is_err() {
            return false;
        }
    }
}
