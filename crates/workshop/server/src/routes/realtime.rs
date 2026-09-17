//! Same-origin, payload-opaque relay for Gateway Realtime transcription.

use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{CloseFrame as BrowserCloseFrame, Message as BrowserMessage};
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message as GatewayMessage;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as GatewayCloseFrame;

use crate::app::AppState;
use crate::gateway::GatewayRealtimeSocket;

const RELAY_IO_DEADLINE: Duration = Duration::from_millis(500);

/// The Workshop endpoint mirroring Gateway Realtime transcription.
pub(crate) fn routes(state: AppState) -> Router {
    Router::new()
        .route("/v1/realtime", get(upgrade))
        .with_state(state)
}

async fn upgrade(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !same_origin_allowed(&uri, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if ws.requested_protocols().next().is_some() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let gateway = state.gateway_snapshot();
    match gateway.client().connect_realtime().await {
        Ok(gateway) => ws.on_upgrade(move |browser| relay(browser, gateway)),
        Err(error) => {
            tracing::warn!(%error, "could not connect the Workshop Realtime relay to the gateway");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

fn same_origin_allowed(uri: &Uri, headers: &HeaderMap) -> bool {
    let Ok(origin) = single_header(headers, header::ORIGIN) else {
        return false;
    };
    let authority = match uri.authority() {
        Some(authority) => Some(authority.as_str()),
        None => match single_header(headers, header::HOST) {
            Ok(authority) => authority,
            Err(()) => return false,
        },
    };
    shared_loopback::workshop_same_origin_authority_allowed(origin, authority)
}

fn single_header(headers: &HeaderMap, name: header::HeaderName) -> Result<Option<&str>, ()> {
    let mut values = headers.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(());
    }
    first.to_str().map(Some).map_err(|_| ())
}

async fn relay(mut browser: WebSocket, mut gateway: GatewayRealtimeSocket) {
    loop {
        tokio::select! {
            gateway_frame = gateway.next() => {
                let Some(Ok(frame)) = gateway_frame else {
                    close_browser(&mut browser).await;
                    return;
                };
                match frame {
                    GatewayMessage::Text(text) => {
                        if !send_browser(&mut browser, BrowserMessage::Text(text.to_string().into())).await {
                            return;
                        }
                    }
                    GatewayMessage::Binary(bytes) => {
                        if !send_browser(&mut browser, BrowserMessage::Binary(bytes.to_vec().into())).await {
                            return;
                        }
                    }
                    GatewayMessage::Ping(_) => {
                        if !flush_gateway(&mut gateway).await {
                            return;
                        }
                    }
                    GatewayMessage::Pong(_) | GatewayMessage::Frame(_) => {}
                    GatewayMessage::Close(frame) => {
                        let outgoing = BrowserMessage::Close(frame.map(|frame| BrowserCloseFrame {
                            code: frame.code.into(),
                            reason: frame.reason.to_string().into(),
                        }));
                        let _sent = send_browser(&mut browser, outgoing).await;
                        let _flushed = flush_gateway(&mut gateway).await;
                        return;
                    }
                }
            }
            browser_frame = browser.recv() => {
                let Some(Ok(frame)) = browser_frame else {
                    close_gateway(&mut gateway).await;
                    return;
                };
                match frame {
                    BrowserMessage::Text(text) => {
                        if !send_gateway(&mut gateway, GatewayMessage::Text(text.to_string().into())).await {
                            return;
                        }
                    }
                    BrowserMessage::Binary(bytes) => {
                        if !send_gateway(&mut gateway, GatewayMessage::Binary(bytes.to_vec().into())).await {
                            return;
                        }
                    }
                    BrowserMessage::Ping(_) => {
                        if !flush_browser(&mut browser).await {
                            return;
                        }
                    }
                    BrowserMessage::Pong(_) => {}
                    BrowserMessage::Close(frame) => {
                        let outgoing = GatewayMessage::Close(frame.map(|frame| GatewayCloseFrame {
                            code: frame.code.into(),
                            reason: frame.reason.to_string().into(),
                        }));
                        let _sent = send_gateway(&mut gateway, outgoing).await;
                        let _flushed = flush_browser(&mut browser).await;
                        return;
                    }
                }
            }
        }
    }
}

async fn send_browser(browser: &mut WebSocket, message: BrowserMessage) -> bool {
    matches!(
        tokio::time::timeout(RELAY_IO_DEADLINE, browser.send(message)).await,
        Ok(Ok(()))
    )
}

async fn send_gateway(gateway: &mut GatewayRealtimeSocket, message: GatewayMessage) -> bool {
    matches!(
        tokio::time::timeout(RELAY_IO_DEADLINE, gateway.send(message)).await,
        Ok(Ok(()))
    )
}

async fn flush_browser(browser: &mut WebSocket) -> bool {
    matches!(
        tokio::time::timeout(RELAY_IO_DEADLINE, browser.flush()).await,
        Ok(Ok(()))
    )
}

async fn flush_gateway(gateway: &mut GatewayRealtimeSocket) -> bool {
    matches!(
        tokio::time::timeout(RELAY_IO_DEADLINE, gateway.flush()).await,
        Ok(Ok(()))
    )
}

async fn close_browser(browser: &mut WebSocket) {
    let _bounded = tokio::time::timeout(RELAY_IO_DEADLINE, browser.close()).await;
}

async fn close_gateway(gateway: &mut GatewayRealtimeSocket) {
    let _bounded = tokio::time::timeout(RELAY_IO_DEADLINE, gateway.close(None)).await;
}
