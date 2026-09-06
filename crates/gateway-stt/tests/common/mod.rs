//! Shared live-server helpers for STT integration tests.

#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Multipart, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::post;
use futures_util::{SinkExt, StreamExt};
use gateway_stt::{SttRuntime, SttState};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tower::ServiceExt as _;

pub(crate) const RECV_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn fixture_runtime(with_final: bool) -> (SttState, SttRuntime) {
    let source = gateway_transcribe::fixtures::require_model();
    fixture_runtime_with_models(&source, with_final.then_some(source.as_path()))
}

pub(crate) fn fixture_runtime_with_models(
    interim_model: &Path,
    final_model: Option<&Path>,
) -> (SttState, SttRuntime) {
    let interim_model = interim_model.to_path_buf();
    let final_model = final_model.map(Path::to_path_buf);
    std::thread::spawn(move || {
        fixture_runtime_with_models_on_dedicated_thread(&interim_model, final_model.as_deref())
    })
    .join()
    .expect("fixture runtime startup thread succeeds")
}

fn fixture_runtime_with_models_on_dedicated_thread(
    interim_model: &Path,
    final_model: Option<&Path>,
) -> (SttState, SttRuntime) {
    let cache = tempfile::tempdir().expect("cache tempdir");
    let interim_source = interim_model.display().to_string().replace('\\', "/");
    let final_source = final_model.map(|path| path.display().to_string().replace('\\', "/"));
    let cache_path = cache.path().display().to_string().replace('\\', "/");
    let final_model = if let Some(source) = final_source {
        format!(
            "[[stt_model]]\nname = \"speech-final\"\nrole = \"final\"\nsource = {source:?}\nvram_gb = 1.0\n"
        )
    } else {
        String::new()
    };
    let profile_models = if final_model.is_empty() {
        "[\"speech\"]"
    } else {
        "[\"speech\", \"speech-final\"]"
    };
    let catalog = gateway_config::Config::from_toml_str(&format!(
        "config-version = 2\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
         [local]\ncache_dir = {cache_path:?}\n\
         [workshop.stt]\nwindow_seconds = 8\ninterval_ms = 400\n\
         [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {interim_source:?}\nvram_gb = 1.0\n\
         {final_model}[[profile]]\nname = \"work\"\nmodels = {profile_models}\n"
    ))
    .expect("fixture catalog parses");
    let config = catalog
        .select_profile(&gateway_config::ProfileName::parse("work").expect("profile name"))
        .expect("fixture profile selects");
    let state = SttState::default();
    let runtime = SttRuntime::start(&config, state.clone(), None).expect("fixture engine loads");
    (state, runtime)
}

pub(crate) fn copy_model_replacing_token(
    source: &Path,
    destination_dir: &Path,
    from: &[u8],
    to: &[u8],
) -> PathBuf {
    assert_eq!(
        from.len(),
        to.len(),
        "model token replacement preserves size"
    );
    let mut model = std::fs::read(source).expect("source model reads");
    let mut replacements = 0usize;
    for offset in 0..=model.len().saturating_sub(from.len()) {
        if model[offset..].starts_with(from) {
            model[offset..offset + from.len()].copy_from_slice(to);
            replacements += 1;
        }
    }
    assert!(
        replacements > 0,
        "source model vocabulary contains {:?}",
        String::from_utf8_lossy(from)
    );
    let destination = destination_dir.join("distinct-final-model.bin");
    std::fs::write(&destination, model).expect("distinct final model writes");
    destination
}

pub(crate) fn fixture_server(with_final: bool) -> TestServer {
    let (state, runtime) = fixture_runtime(with_final);
    TestServer::spawn_with(state, Some(runtime))
}

pub(crate) struct TestServer {
    url: String,
    task: tokio::task::JoinHandle<()>,
    runtime: Option<SttRuntime>,
}

impl TestServer {
    pub(crate) fn spawn() -> Self {
        Self::spawn_with(SttState::default(), None)
    }

    pub(crate) fn spawn_with(state: SttState, runtime: Option<SttRuntime>) -> Self {
        let std_listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("gateway listener binds");
        std_listener
            .set_nonblocking(true)
            .expect("gateway listener becomes nonblocking");
        let address = std_listener
            .local_addr()
            .expect("gateway listener has an address");
        let listener =
            tokio::net::TcpListener::from_std(std_listener).expect("tokio adopts the listener");
        let app = gateway_stt::gateway_routes(state);
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("gateway STT fixture serves");
        });
        Self {
            url: format!("http://{address}"),
            task,
            runtime,
        }
    }

    pub(crate) fn ws_url(&self, path: &str) -> String {
        format!(
            "ws{}{}",
            self.url.strip_prefix("http").expect("server URL is http"),
            path
        )
    }

    pub(crate) async fn shutdown(mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
        if let Some(runtime) = self.runtime.take() {
            tokio::task::spawn_blocking(move || runtime.shutdown())
                .await
                .expect("fixture runtime shutdown task succeeds");
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) async fn send_pcm(socket: &mut JsonSocket, frames: usize) {
    socket.send_binary(vec![0u8; frames * 4]).await;
}

pub(crate) async fn send_samples(socket: &mut JsonSocket, samples: &[f32]) {
    const BLOCK: usize = 4096;
    for chunk in samples.chunks(BLOCK) {
        let mut bytes = Vec::with_capacity(chunk.len() * 4);
        for sample in chunk {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        socket.send_binary(bytes).await;
    }
}

pub(crate) async fn send_samples_once(socket: &mut JsonSocket, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    socket.send_binary(bytes).await;
}

fn wav_f32(samples: &[f32]) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .expect("WAV writer builds");
        for sample in samples {
            writer.write_sample(*sample).expect("WAV sample writes");
        }
        writer.finalize().expect("WAV finalizes");
    }
    bytes.into_inner()
}

fn multipart_body(file: &[u8], model: &str) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "gateway-stt-integration-boundary";
    let mut body = format!(
        "--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"model\"\r\n\r\n\
         {model}\r\n\
         --{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"response_format\"\r\n\r\n\
         json\r\n\
         --{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (BOUNDARY.to_owned(), body)
}

async fn batch_endpoint(State(state): State<SttState>, multipart: Multipart) -> Response {
    match gateway_stt::transcribe(&state, multipart).await {
        Ok(response) => response,
        Err(error) if error.model_not_found().is_some() => {
            (StatusCode::NOT_FOUND, error.to_string()).into_response()
        }
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

pub(crate) async fn transcribe_batch(
    state: SttState,
    model: &str,
    samples: &[f32],
) -> (StatusCode, serde_json::Value) {
    let (boundary, body) = multipart_body(&wav_f32(samples), model);
    let response = axum::Router::new()
        .route("/v1/audio/transcriptions", post(batch_endpoint))
        .with_state(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("batch request builds"),
        )
        .await
        .expect("batch route answers");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("batch response body reads");
    let json = serde_json::from_slice(&body).expect("batch response is JSON");
    (status, json)
}

pub(crate) struct JsonSocket {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl JsonSocket {
    pub(crate) async fn connect(url: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("WebSocket connects");
        Self { socket }
    }

    pub(crate) async fn send_text(&mut self, text: &str) {
        self.socket
            .send(Message::Text(text.to_owned().into()))
            .await
            .expect("text frame sends");
    }

    pub(crate) async fn send_binary(&mut self, bytes: Vec<u8>) {
        self.socket
            .send(Message::Binary(bytes.into()))
            .await
            .expect("binary frame sends");
    }

    pub(crate) async fn recv_json(&mut self) -> serde_json::Value {
        let message = tokio::time::timeout(RECV_TIMEOUT, self.socket.next())
            .await
            .expect("frame arrives before timeout")
            .expect("socket open")
            .expect("frame has no socket error");
        let text = message.into_text().expect("frame is text");
        serde_json::from_str(&text).expect("frame is JSON")
    }

    pub(crate) async fn recv_until(
        &mut self,
        deadline: Duration,
        keep: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        tokio::time::timeout(deadline, async {
            loop {
                let frame = self.recv_json().await;
                if keep(&frame) {
                    break frame;
                }
            }
        })
        .await
        .expect("matching frame arrives before deadline")
    }

    pub(crate) async fn close(mut self) {
        self.socket.close(None).await.expect("socket closes");
    }
}
