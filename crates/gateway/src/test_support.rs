//! Shared admin-route test harness: serves `build_router` over a state
//! assembled from one fixture profile.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use gateway_config::Config;
use shared_progress::ProgressHub;

use crate::routing::Routing;
use crate::{AppState, ProfileSelection, build_router};

/// Filesystem context for shadow-write and env admin-route tests.
pub(crate) struct AdminPaths {
    /// Fixture directory used by filesystem-boundary tests.
    pub(crate) fixture_dir: PathBuf,
    /// The active profile name.
    pub(crate) active: String,
    /// The single config path.
    pub(crate) config_path: PathBuf,
}

/// Serves `build_router` over a state assembled from `config` with no
/// running children: the retained config still carries everything the
/// admin routes read (the cache root, the `[[local_model]]` entries).
pub(crate) async fn serve(config: Config) -> SocketAddr {
    serve_with(config, None, None).await
}

/// Serves like [`serve`], but with the Hugging Face proxy replaced, so a
/// test can aim the `/admin/hf/*` routes at a local stub hub with an
/// explicit token instead of the process env.
pub(crate) async fn serve_with_hf(config: Config, hf: crate::hf::HfProxy) -> SocketAddr {
    serve_with(config, Some(hf), None).await
}

/// Serves like [`serve`], but with active profile and config-file context.
pub(crate) async fn serve_with_paths(config: Config, paths: AdminPaths) -> SocketAddr {
    serve_with(config, None, Some(paths)).await
}

/// The shared harness body behind [`serve`], [`serve_with_hf`], and
/// [`serve_with_paths`].
async fn serve_with(
    config: Config,
    hf: Option<crate::hf::HfProxy>,
    paths: Option<AdminPaths>,
) -> SocketAddr {
    let mut state = app_state(config, paths);
    if let Some(hf) = hf {
        state.hf = Arc::new(hf);
    }
    serve_state(state).await
}

/// Builds the state the harness serves, so a test can override an
/// injected collaborator (the HF proxy, the reveal launcher) or drive a
/// handler directly.
pub(crate) fn app_state(config: Config, paths: Option<AdminPaths>) -> AppState {
    let routing = Routing::from_config(&config).expect("routing builds");
    state_over(config, routing, paths)
}

/// Builds state with deterministic speech workers for Gateway route tests.
#[cfg(feature = "stt")]
pub(crate) async fn app_state_with_scripted_stt(
    config: Config,
    factory: gateway_stt::test_fixtures::ScriptedModelFactory,
) -> Result<AppState, String> {
    let runtime = gateway_stt::test_fixtures::scripted_runtime(factory, 15, 500)
        .map_err(|error| error.to_string())?;
    let stt_state = runtime.state();
    let mut state = app_state(config, None);
    state.live.write().await.stt = Some(runtime);
    state.stt_state = stt_state;
    Ok(state)
}

/// Builds the state the instant-ready boot path serves: an empty routing
/// table over `config`, no active profile, nothing local running - the
/// shell the boot `LoadProfile` command fills.
pub(crate) fn boot_state(config: Config) -> AppState {
    state_over(config, Routing::empty(), None)
}

/// The shared body behind [`app_state`] and [`boot_state`].
fn state_over(config: Config, routing: Routing, paths: Option<AdminPaths>) -> AppState {
    let key = config.server_key();
    let config = Arc::new(config);
    let (config_path, selection) = match paths {
        Some(paths) => (
            Some(paths.config_path),
            ProfileSelection {
                name: Some(paths.active),
                model_allowlist: None,
            },
        ),
        None => (None, ProfileSelection::default()),
    };
    AppState::from_parts(
        Arc::new(routing),
        key,
        Arc::clone(&config),
        #[cfg(feature = "local")]
        crate::local::LocalRuntime::empty(),
        #[cfg(feature = "stt")]
        gateway_stt::SttRuntime::empty(gateway_stt::SttState::default()),
        #[cfg(feature = "web-search")]
        config.web_search_config(),
        config_path,
        selection,
        Arc::new(ProgressHub::new()),
    )
}

/// Binds an ephemeral loopback listener and serves `state` on it,
/// returning the bound address. Connect info and the host-authority wall
/// are wired exactly as in the production serve path, so loopback-only
/// routes see a peer address and the wall sees the bound socket.
pub(crate) async fn serve_state(state: AppState) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test listener binds");
    let addr = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let _ignored = axum::serve(
            listener,
            build_router(state, Some(addr)).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    addr
}

#[cfg(all(test, feature = "stt"))]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};
    use tower::ServiceExt;

    use super::*;

    fn transcription_body() -> (String, Vec<u8>) {
        const BOUNDARY: &str = "scripted-stt-boundary";
        let mut wav = vec![
            b'R', b'I', b'F', b'F', 38, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
            16, 0, 0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0x00, 0x7d, 0, 0, 2, 0, 16, 0, b'd', b'a',
            b't', b'a', 2, 0, 0, 0, 0, 32,
        ];
        let mut body = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n\
             scripted-interim\r\n\
             --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"sample.wav\"\r\n\
             Content-Type: audio/wav\r\n\r\n"
        )
        .into_bytes();
        body.append(&mut wav);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        (BOUNDARY.to_owned(), body)
    }

    #[tokio::test]
    async fn scripted_workers_can_be_injected_without_a_production_constructor() {
        const TRANSCRIPT: &str = "gateway scripted route sentinel";

        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
        )
        .expect("config parses");
        let decoder = ScriptedDecoder::new();
        decoder.push_text(TRANSCRIPT);
        let state = app_state_with_scripted_stt(config, ScriptedModelFactory::new(decoder.clone()))
            .await
            .expect("scripted state builds");
        let (boundary, body) = transcription_body();

        let response = build_router(state, None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header("authorization", "Bearer test-token")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads");
        let response: serde_json::Value =
            serde_json::from_slice(&body).expect("response body is JSON");
        assert_eq!(response["text"], TRANSCRIPT);
        let requests = decoder.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].mode(),
            gateway_stt::test_fixtures::DecodeMode::Interim
        );
        assert_eq!(requests[0].samples(), &[0.25]);
        assert!(requests[0].guidance().is_empty());
        assert!(requests[0].finalized().is_empty());
    }
}
