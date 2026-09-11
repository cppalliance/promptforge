use std::time::Duration;

#[cfg(feature = "test-fixtures")]
use std::sync::Arc;
#[cfg(feature = "test-fixtures")]
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::StreamExt as _;

use super::Session;
use super::query;
use super::registry::{RegisterError, SessionRegistry};
use super::result_mailbox::MailboxError;
use super::session::SessionError;
use super::wire::{ClientError, ClientEvent, ServerEvent, parse_client_event};
use crate::audio::AudioError;
use crate::generation::{GenerationLease, GenerationState};

const SEND_DEADLINE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
struct RouteState {
    generation: GenerationState,
    sessions: SessionRegistry,
    policy: RoutePolicy,
}

#[cfg(feature = "test-fixtures")]
#[derive(Clone, Copy, Debug)]
pub(crate) enum ForcedPrecommitFailure {
    FinalSegmentOverload,
    Transcription,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RoutePolicy {
    #[cfg(feature = "test-fixtures")]
    blocked_send: Option<Arc<BlockedSend>>,
    #[cfg(feature = "test-fixtures")]
    forced_precommit_failure: Option<ForcedPrecommitFailure>,
}

#[cfg(feature = "test-fixtures")]
#[derive(Debug)]
struct BlockedSend {
    after: usize,
    attempted: AtomicUsize,
}

impl RoutePolicy {
    #[cfg(feature = "test-fixtures")]
    pub(crate) fn blocking_after(after: usize) -> Self {
        Self {
            blocked_send: Some(Arc::new(BlockedSend {
                after,
                attempted: AtomicUsize::new(0),
            })),
            forced_precommit_failure: None,
        }
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn force_precommit_failure(&mut self, failure: ForcedPrecommitFailure) {
        self.forced_precommit_failure = Some(failure);
    }

    fn precommit_failure(&self) -> Option<&'static str> {
        #[cfg(feature = "test-fixtures")]
        if let Some(failure) = self.forced_precommit_failure {
            return Some(match failure {
                ForcedPrecommitFailure::FinalSegmentOverload => "final segment capacity is reached",
                ForcedPrecommitFailure::Transcription => {
                    "final transcription worker is unavailable"
                }
            });
        }
        None
    }

    fn blocks_next(&self) -> bool {
        #[cfg(feature = "test-fixtures")]
        if let Some(blocked) = &self.blocked_send {
            return blocked.attempted.fetch_add(1, Ordering::Relaxed) >= blocked.after;
        }
        false
    }
}

pub(crate) fn routes(
    generation: GenerationState,
    sessions: SessionRegistry,
    policy: RoutePolicy,
) -> Router {
    Router::new()
        .route("/v1/realtime", get(upgrade))
        .with_state(RouteState {
            generation,
            sessions,
            policy,
        })
}

async fn upgrade(
    State(state): State<RouteState>,
    uri: Uri,
    websocket: WebSocketUpgrade,
) -> Response {
    if query::validate(uri.query()).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(generation) = state
        .generation
        .active()
        .filter(GenerationLease::has_final_pass)
    else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let registration = match state.sessions.register() {
        Ok(registration) => registration,
        Err(RegisterError::AtCapacity) => return StatusCode::TOO_MANY_REQUESTS.into_response(),
    };
    let session = Session::new(registration, Some(generation.clone()));
    let policy = state.policy.clone();
    websocket
        .on_upgrade(move |socket| run_socket(socket, session, generation, policy))
        .into_response()
}

async fn run_socket(
    mut socket: WebSocket,
    mut session: Session,
    generation: GenerationLease,
    policy: RoutePolicy,
) {
    if !send_event(&mut socket, &session.created_event(), &policy).await {
        return;
    }
    let mut completions = tokio::time::interval(Duration::from_millis(10));
    completions.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut interims = tokio::time::interval_at(
        tokio::time::Instant::now() + generation.interval(),
        generation.interval(),
    );
    interims.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            () = generation.cancelled() => {
                // The one runtime is shutting down; the server drain closes
                // the socket.
                return;
            }
            _ = completions.tick() => {
                if let Err(error) = session.reap_canceled().await {
                    let error = session_error(&error, None);
                    if !send_client_error(&mut socket, &session, error, &policy).await {
                        return;
                    }
                }
                if session.interim_finished() {
                    match session.finish_interim().await {
                        Ok(Some(event)) => {
                            if !send_event(&mut socket, &event, &policy).await {
                                return;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let error = session_error(&error, None);
                            if !send_client_error(&mut socket, &session, error, &policy).await {
                                return;
                            }
                        }
                    }
                }
                let Ok(events) = session.finish_ready().await else {
                    return;
                };
                if !send_events(&mut socket, &events, &policy).await {
                    return;
                }
            }
            _ = interims.tick() => {
                if session.schedule_interim().is_err() {
                    return;
                }
            }
            incoming = socket.next() => {
                let Some(Ok(message)) = incoming else {
                    return;
                };
                if !handle_message(&mut socket, &mut session, message, &policy).await {
                    return;
                }
            }
        }
    }
}

async fn handle_message(
    socket: &mut WebSocket,
    session: &mut Session,
    message: Message,
    policy: &RoutePolicy,
) -> bool {
    match message {
        Message::Text(text) => handle_text(socket, session, text.as_str(), policy).await,
        Message::Binary(_) => {
            send_client_error(
                socket,
                session,
                ClientError::request(
                    "invalid_frame",
                    "Realtime client events must be JSON text",
                    None,
                    None,
                ),
                policy,
            )
            .await
        }
        Message::Ping(payload) => send_message(socket, Message::Pong(payload), policy).await,
        Message::Pong(_) => true,
        Message::Close(_) => false,
    }
}

async fn handle_text(
    socket: &mut WebSocket,
    session: &mut Session,
    text: &str,
    policy: &RoutePolicy,
) -> bool {
    let event = match parse_client_event(text) {
        Ok(event) => event,
        Err(error) => return send_client_error(socket, session, error, policy).await,
    };
    let client_event_id = event_id(&event);
    let result = match event {
        ClientEvent::SessionUpdate { .. } => session
            .update_text(text)
            .map(|()| vec![session.updated_event()]),
        ClientEvent::Append { audio, .. } => append_events(session, &audio, policy)
            .map_err(|error| session_error(&error, client_event_id)),
        ClientEvent::Clear { .. } => session
            .clear()
            .map(|()| vec![session.cleared_event()])
            .map_err(|error| session_error(&error, client_event_id)),
        ClientEvent::Commit { .. } => commit_events(session)
            .await
            .map_err(|error| session_error(&error, client_event_id)),
    };
    match result {
        Ok(events) => send_events(socket, &events, policy).await,
        Err(error) => send_client_error(socket, session, error, policy).await,
    }
}

fn append_events(
    session: &mut Session,
    audio: &str,
    policy: &RoutePolicy,
) -> Result<Vec<ServerEvent>, SessionError> {
    session.ensure_interim_capacity()?;
    session.append_base64(audio)?;
    if let Some(failure) = policy.precommit_failure() {
        session.record_pending_failure(failure.to_owned())?;
    }
    Ok(Vec::new())
}

async fn commit_events(session: &mut Session) -> Result<Vec<ServerEvent>, SessionError> {
    let ready_interim = if session.interim_finished() {
        session.finish_interim().await?
    } else {
        None
    };
    let receipt = session.commit()?;
    let item_id = receipt.item_id().to_owned();
    let mut events = ready_interim.into_iter().collect::<Vec<_>>();
    events.extend(session.committed_events(&receipt));
    events.extend(session.take_pending_interim(&item_id));
    events.extend(session.drain_events());
    Ok(events)
}

fn event_id(event: &ClientEvent) -> Option<String> {
    match event {
        ClientEvent::SessionUpdate { event_id, .. }
        | ClientEvent::Append { event_id, .. }
        | ClientEvent::Commit { event_id }
        | ClientEvent::Clear { event_id } => event_id.clone(),
    }
}
fn session_error(error: &SessionError, client_event_id: Option<String>) -> ClientError {
    match error {
        SessionError::Audio(AudioError::InvalidBase64(_)) => ClientError::request(
            "invalid_base64_audio",
            "Audio must be valid Base64",
            Some("audio"),
            client_event_id,
        ),
        SessionError::Audio(AudioError::AppendTooLarge { .. }) => ClientError::request(
            "audio_append_too_large",
            "Decoded audio exceeds the 15 MiB append limit",
            Some("audio"),
            client_event_id,
        ),
        SessionError::Audio(AudioError::IncompletePcm16Sample) => ClientError::request(
            "invalid_pcm_audio",
            "PCM16 audio ends with an incomplete sample",
            Some("audio"),
            client_event_id,
        ),
        SessionError::Audio(AudioError::BufferTooLong { .. }) => ClientError::overload(
            "too_much_unfinalized_audio",
            "Unfinalized audio exceeds 30 seconds",
            Some("audio"),
            client_event_id,
        ),
        SessionError::Audio(AudioError::CommitTooShort { .. }) => ClientError::request(
            "audio_too_short",
            "A commit requires at least 100 ms of audio",
            Some("audio"),
            client_event_id,
        ),
        SessionError::CommittedItemsAtCapacity => ClientError::overload(
            "too_many_committed_items",
            "At most four committed items may finalize concurrently",
            None,
            client_event_id,
        ),
        SessionError::InterimAtCapacity => ClientError::overload(
            "result_queue_overload",
            "The session result queue is full",
            None,
            client_event_id,
        ),
        #[cfg(any(test, feature = "test-fixtures"))]
        SessionError::Mailbox(MailboxError::ResultAtCapacity) => {
            session_error(&SessionError::InterimAtCapacity, client_event_id)
        }
        SessionError::PendingPrecommitFailure(_) => ClientError::request(
            "precommit_transcription_failed",
            "Further appends are rejected after accurate precommit failure",
            Some("audio"),
            client_event_id,
        ),
        SessionError::CancelJoinAtCapacity => ClientError::overload(
            "audio_queue_lag",
            "Audio queue lag exceeds two seconds",
            Some("audio"),
            client_event_id,
        ),
        SessionError::NoInput => ClientError::request(
            "input_audio_buffer_empty",
            "The input audio buffer is empty",
            Some("audio"),
            client_event_id,
        ),
        SessionError::EpochExhausted
        | SessionError::CanceledTaskFailed
        | SessionError::GenerationUnavailable
        | SessionError::Inference(_)
        | SessionError::Finalization(_)
        | SessionError::Mailbox(MailboxError::TerminalAlreadySet | MailboxError::UnknownItem) => {
            ClientError::server(
                "internal_error",
                "Transcription failed",
                None,
                client_event_id,
            )
        }
    }
}
async fn send_events(socket: &mut WebSocket, events: &[ServerEvent], policy: &RoutePolicy) -> bool {
    for event in events {
        if !send_event(socket, event, policy).await {
            return false;
        }
    }
    true
}
async fn send_client_error(
    socket: &mut WebSocket,
    session: &Session,
    error: ClientError,
    policy: &RoutePolicy,
) -> bool {
    send_json(
        socket,
        error.into_server_event(&session.next_event_id()),
        policy,
    )
    .await
}
async fn send_event(socket: &mut WebSocket, event: &ServerEvent, policy: &RoutePolicy) -> bool {
    match serde_json::to_value(event) {
        Ok(value) => send_json(socket, value, policy).await,
        Err(_) => false,
    }
}

async fn send_json(socket: &mut WebSocket, value: serde_json::Value, policy: &RoutePolicy) -> bool {
    send_message(socket, Message::Text(value.to_string().into()), policy).await
}

async fn send_message(socket: &mut WebSocket, message: Message, policy: &RoutePolicy) -> bool {
    tokio::time::timeout(SEND_DEADLINE, async {
        if policy.blocks_next() {
            std::future::pending::<()>().await;
        }
        socket.send(message).await
    })
    .await
    .is_ok_and(|result| result.is_ok())
}
