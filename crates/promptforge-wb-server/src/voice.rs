//! The `/voice` WebSocket endpoint: push-to-talk PCM capture sessions.
//!
//! A client upgrades `GET /voice`, sends the text control message `start`,
//! streams binary messages of little-endian f32 PCM (16 kHz mono), and sends
//! `stop` to end a take. On `stop` the server answers with one text message,
//! `{"frames":N}`, holding the total PCM frames received since the most
//! recent `start`. Transcription is a later step; for now the count is the
//! whole reply.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;

/// Session ids for log correlation, handed out in connection order.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// Upgrades a `GET /voice` request to a WebSocket voice-capture session.
pub(crate) async fn upgrade(ws: WebSocketUpgrade) -> Response {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    ws.on_upgrade(move |socket| run_session(session, socket))
}

/// Runs one capture session until the socket closes or fails.
async fn run_session(session: u64, mut socket: WebSocket) {
    tracing::info!(session, "voice session opened");
    let mut frames = 0u64;
    while let Some(received) = socket.recv().await {
        match received {
            Ok(Message::Binary(payload)) => {
                // One PCM frame is a single 16 kHz mono f32 sample: four
                // bytes. A trailing partial sample is ignored.
                frames += payload.len() as u64 / 4;
            }
            Ok(Message::Text(text)) => match text.as_str() {
                "start" => {
                    frames = 0;
                    tracing::info!(session, "voice capture started");
                }
                "stop" => {
                    tracing::info!(session, frames, "voice capture stopped");
                    let reply = format!(r#"{{"frames":{frames}}}"#);
                    if let Err(error) = socket.send(Message::Text(reply.into())).await {
                        tracing::warn!(session, %error, "voice stop reply failed to send");
                        break;
                    }
                }
                _ => {}
            },
            // Pings and pongs are answered by axum itself.
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(Message::Close(_)) => break,
            Err(error) => {
                tracing::warn!(session, %error, "voice session socket failed");
                break;
            }
        }
    }
    tracing::info!(session, frames, "voice session closed");
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite;

    /// Binds the voice route on a free loopback port and returns its
    /// WebSocket URL.
    async fn spawn_voice_server() -> String {
        let app = axum::Router::new().route("/voice", axum::routing::get(upgrade));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the voice test server");
        let addr = listener.local_addr().expect("voice test server address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("voice test server serves");
        });
        format!("ws://{addr}/voice")
    }

    /// Sends one binary message holding `frames` f32 PCM samples.
    async fn send_pcm<S>(socket: &mut S, frames: usize)
    where
        S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
    {
        socket
            .send(tungstenite::Message::Binary(vec![0u8; frames * 4].into()))
            .await
            .expect("the PCM block is sent");
    }

    #[tokio::test]
    async fn pcm_frames_are_counted_until_stop() {
        let url = spawn_voice_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        send_pcm(&mut socket, 128).await;
        send_pcm(&mut socket, 64).await;
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");

        let reply = socket
            .next()
            .await
            .expect("a reply follows stop")
            .expect("the reply is not a socket error");
        let text = reply.into_text().expect("the reply is a text message");
        assert_eq!(text.as_str(), r#"{"frames":192}"#);
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn start_resets_the_frame_count_for_a_new_take() {
        let url = spawn_voice_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        send_pcm(&mut socket, 100).await;
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send a second start");
        send_pcm(&mut socket, 10).await;
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");

        let reply = socket
            .next()
            .await
            .expect("a reply follows stop")
            .expect("the reply is not a socket error");
        let text = reply.into_text().expect("the reply is a text message");
        assert_eq!(
            text.as_str(),
            r#"{"frames":10}"#,
            "the second take counts only its own frames"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn unknown_text_is_ignored_and_partial_samples_are_dropped() {
        let url = spawn_voice_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /voice");
        socket
            .send(tungstenite::Message::Text("start".into()))
            .await
            .expect("send start");
        socket
            .send(tungstenite::Message::Text("bogus".into()))
            .await
            .expect("send an unknown control message");
        send_pcm(&mut socket, 10).await;
        // Three bytes are a trailing partial sample: not a whole f32 frame.
        socket
            .send(tungstenite::Message::Binary(vec![0u8; 3].into()))
            .await
            .expect("send a partial sample");
        socket
            .send(tungstenite::Message::Text("stop".into()))
            .await
            .expect("send stop");

        let reply = socket
            .next()
            .await
            .expect("a reply follows stop")
            .expect("the reply is not a socket error");
        let text = reply.into_text().expect("the reply is a text message");
        assert_eq!(
            text.as_str(),
            r#"{"frames":10}"#,
            "unknown text is ignored and the partial sample is not counted"
        );
        socket.close(None).await.expect("close the socket");
    }
}
