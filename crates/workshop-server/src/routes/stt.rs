//! Same-origin relay for the gateway-owned speech-to-text routes.

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{Message as BrowserMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt as _, StreamExt as _};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message as GatewayMessage;

use crate::app::AppState;
use crate::error::AppError;
use crate::gateway::GatewaySttSocket;
use crate::{Activity, Push, origin_allowed};

/// The same-origin STT routes consumed by the Workshop UI.
pub(crate) fn routes(state: AppState) -> Router {
    Router::new()
        .route("/stt/capability", get(capability))
        .route("/stt", get(upgrade))
        .with_state(state)
}

async fn capability(State(state): State<AppState>) -> Result<Response, AppError> {
    let forwarded = state
        .gateway_client()
        .forward(reqwest::Method::GET, "/stt/capability", None)
        .await
        .map_err(AppError::Gateway)?;
    let status = StatusCode::from_u16(forwarded.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::new(Body::from(forwarded.body));
    *response.status_mut() = status;
    if let Some(content_type) = forwarded.content_type
        && let Ok(value) = content_type.parse()
    {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    Ok(response)
}

async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match state.gateway_client().connect_stt().await {
        Ok(gateway) => {
            let push = state.push();
            ws.on_upgrade(move |browser| relay(browser, gateway, push))
        }
        Err(error) => {
            tracing::warn!(%error, "could not connect the Workshop STT relay to the gateway");
            state.push().push_failure(
                "Dictation connection failed",
                error.to_string(),
                Activity::General,
            );
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct RelayedStatusFrame {
    #[serde(rename = "type")]
    kind: String,
    label: String,
    description: String,
    severity: String,
}

fn consume_status(text: &str, push: &Push) -> bool {
    let Ok(status) = serde_json::from_str::<RelayedStatusFrame>(text) else {
        return false;
    };
    if status.kind != "workshop_status" {
        return false;
    }
    match status.severity.as_str() {
        "info" => push.push_status_update(status.label, status.description, Activity::General),
        "debug" => push.push_activity(status.label, status.description, Activity::General),
        "error" => push.push_failure(status.label, status.description, Activity::General),
        severity => tracing::warn!(severity, "gateway sent an unknown STT status severity"),
    }
    true
}

async fn relay(mut browser: WebSocket, mut gateway: GatewaySttSocket, push: Push) {
    loop {
        tokio::select! {
            browser_frame = browser.recv() => {
                let Some(Ok(frame)) = browser_frame else {
                    break;
                };
                let outgoing = match frame {
                    BrowserMessage::Text(text) => GatewayMessage::Text(text.to_string().into()),
                    BrowserMessage::Binary(bytes) => GatewayMessage::Binary(bytes.to_vec().into()),
                    BrowserMessage::Ping(bytes) => GatewayMessage::Ping(bytes.to_vec().into()),
                    BrowserMessage::Pong(bytes) => GatewayMessage::Pong(bytes.to_vec().into()),
                    BrowserMessage::Close(_) => break,
                };
                if gateway.send(outgoing).await.is_err() {
                    break;
                }
            }
            gateway_frame = gateway.next() => {
                let Some(Ok(frame)) = gateway_frame else {
                    break;
                };
                let outgoing = match frame {
                    GatewayMessage::Text(text) => {
                        if consume_status(&text, &push) {
                            continue;
                        }
                        BrowserMessage::Text(text.to_string().into())
                    }
                    GatewayMessage::Binary(bytes) => BrowserMessage::Binary(bytes.to_vec().into()),
                    GatewayMessage::Ping(bytes) => BrowserMessage::Ping(bytes.to_vec().into()),
                    GatewayMessage::Pong(bytes) => BrowserMessage::Pong(bytes.to_vec().into()),
                    GatewayMessage::Close(_) => break,
                    GatewayMessage::Frame(_) => continue,
                };
                if browser.send(outgoing).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = gateway.close(None).await;
    let _ = browser.close().await;
    push.push_idle();
}

#[cfg(test)]
mod tests {
    use axum::extract::ws::{Message, WebSocketUpgrade};
    use axum::http::{Request, header};
    use axum::routing::get;
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message as ClientMessage;
    use tower::ServiceExt as _;

    use super::*;
    use crate::app::fixtures::{body_bytes, config_for, spawn_gateway, state_for};
    use crate::resolve::ResolvedGateway;

    async fn mock_capability(headers: HeaderMap) -> Response {
        if headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some("Bearer test-key")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        (
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"gpu":true,"engine":true}"#,
        )
            .into_response()
    }

    async fn mock_socket(headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
        if headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some("Bearer test-key")
            || headers
                .get("x-promptforge-workshop-status")
                .and_then(|value| value.to_str().ok())
                != Some("1")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        ws.on_upgrade(|mut socket| async move {
            while let Some(Ok(message)) = socket.recv().await {
                match message {
                    Message::Text(_) | Message::Binary(_) => {
                        if matches!(&message, Message::Text(text) if text.as_str() == "start")
                            && socket
                                .send(Message::Text(
                                    r#"{"type":"workshop_status","label":"Relay listening","description":"private status","severity":"info"}"#
                                        .into(),
                                ))
                                .await
                                .is_err()
                        {
                            return;
                        }
                        if socket.send(message).await.is_err() {
                            return;
                        }
                    }
                    Message::Close(_) => return,
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
        })
    }

    #[tokio::test]
    async fn capability_is_relayed_with_the_gateway_key() {
        let gateway =
            spawn_gateway(Router::new().route("/stt/capability", get(mock_capability))).await;
        let (state, _state_dir) = state_for(&gateway);
        let response = routes(state)
            .oneshot(
                Request::builder()
                    .uri("/stt/capability")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("route responds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&"application/json".parse().expect("header value parses"))
        );
        assert_eq!(body_bytes(response).await, r#"{"gpu":true,"engine":true}"#);
    }

    #[tokio::test]
    async fn websocket_frames_cross_the_authenticated_relay() {
        let gateway = spawn_gateway(Router::new().route("/stt", get(mock_socket))).await;
        let state_dir = tempfile::TempDir::new().expect("tempdir");
        let mut config = config_for(&gateway, state_dir.path());
        config.server.bind = "127.0.0.1:0".to_owned();
        let resolved = ResolvedGateway::from_config(&config.gateway);
        let server = crate::spawn_with_routes(config, resolved, |_| Router::new())
            .expect("Workshop server starts");
        let address = server
            .url()
            .strip_prefix("http")
            .expect("Workshop URL is HTTP");
        let (mut observer, _response) = tokio_tungstenite::connect_async(format!("ws{address}/ws"))
            .await
            .expect("status observer connects");
        let (mut socket, _response) = tokio_tungstenite::connect_async(format!("ws{address}/stt"))
            .await
            .expect("browser-side socket connects");

        socket
            .send(ClientMessage::Text("start".into()))
            .await
            .expect("text frame sends");
        assert_eq!(
            socket
                .next()
                .await
                .expect("reply arrives")
                .expect("reply is valid"),
            ClientMessage::Text("start".into())
        );
        let status = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let message = observer
                    .next()
                    .await
                    .expect("status socket stays open")
                    .expect("status frame is valid");
                let ClientMessage::Text(text) = message else {
                    continue;
                };
                let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if frame["type"] == "status" && frame["label"] == "Relay listening" {
                    break frame;
                }
            }
        })
        .await
        .expect("relayed status reaches the Workshop observer");
        assert_eq!(status["description"], "private status");
        socket
            .send(ClientMessage::Binary(vec![1, 2, 3].into()))
            .await
            .expect("binary frame sends");
        assert_eq!(
            socket
                .next()
                .await
                .expect("reply arrives")
                .expect("reply is valid"),
            ClientMessage::Binary(vec![1, 2, 3].into())
        );

        socket.close(None).await.expect("socket closes");
        observer.close(None).await.expect("observer closes");
        server.shutdown().expect("Workshop server stops");
    }
}
