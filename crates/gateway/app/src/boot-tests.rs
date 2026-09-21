//! Provisioning-state route behavior: the resolver's miss ladder and the 503s that name the active command.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gateway_config::{Config, ProfileName};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

use crate::commands::Command;
use crate::test_support::{app_state, fake_chat_backend, parking_executor, wait_until};
use crate::{AppState, build_router};

/// `app_state` routes only the remote catalog, so the catalog's local
/// `slow-model` stays configured-but-unloaded for the test's whole run.
fn state() -> AppState {
    let config = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[local_model]]\nname = \"slow-model\"\ndescription = \"d\"\n\
         source = \"/models/slow.gguf\"\ncontext = 4096\n\
         [[profile]]\nname = \"main\"\nmodels = [\"slow-model\"]\n",
    )
    .expect("config parses");
    app_state(config, None)
}

async fn chat(state: AppState, model: &str) -> axum::response::Response {
    build_router(state, None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("authorization", "Bearer test-token")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"model":"{model}","messages":[{{"role":"user","content":"ping"}}]}}"#
                )))
                .expect("request builds"),
        )
        .await
        .expect("router answers")
}

#[tokio::test]
async fn an_unloaded_but_configured_model_earns_a_503_naming_the_active_command() {
    let state = state();
    let worker = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("worker spawns");
    let _boot = state.commands.enqueue(Command::load_profile(
        ProfileName::parse("main").expect("profile name"),
        CancellationToken::new(),
    ));
    wait_until("the command to go active", || {
        state.commands.active_command().is_some()
    })
    .await;

    let response = chat(state.clone(), "slow-model").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let text = std::str::from_utf8(&body).expect("the envelope is UTF-8");
    assert!(
        text.contains("model provisioning in progress"),
        "the 503 names the condition: {text}"
    );
    assert!(
        text.contains("load-profile: main"),
        "the 503 names the active command: {text}"
    );

    // A model the catalog does not name keeps its plain 404.
    let response = chat(state.clone(), "ghost").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    state.commands.cancel_active();
    state.commands.shutdown();
    worker.await.expect("the worker exits on shutdown");
}

/// The resolver's miss ladder: a name in `loading` is `ModelLoading`,
/// a name the catalog does not know stays `UnknownModel`.
#[tokio::test]
async fn a_routing_miss_on_a_loading_model_is_model_loading_not_not_found() {
    let state = state();
    state
        .live
        .write()
        .await
        .loading
        .insert("pending-model".to_owned());
    let loading = crate::relay::resolve_routed_model(&state, "pending-model").await;
    assert!(
        matches!(&loading, Err(crate::error::GatewayError::ModelLoading(name)) if name == "pending-model"),
        "a loading model resolves to ModelLoading: {loading:?}"
    );
    let unknown = crate::relay::resolve_routed_model(&state, "ghost").await;
    assert!(
        matches!(unknown, Err(crate::error::GatewayError::UnknownModel(_))),
        "a name outside loading keeps its 404: {unknown:?}"
    );
}

/// A profile over one remote model on `backend` and one local model
/// whose source is a real file but whose `llama-server` is a plain text
/// file, so the artifact step succeeds and the spawn fails per model.
#[cfg(feature = "local")]
fn local_profile_config(temp: &tempfile::TempDir, backend: &str) -> Config {
    let fake_server = temp.path().join("fake-llama-server");
    std::fs::write(&fake_server, b"not a server").expect("write fake server");
    let model_file = temp.path().join("local.gguf");
    std::fs::write(&model_file, b"not a gguf").expect("write model");
    let slash = |path: &std::path::Path| path.display().to_string().replace('\\', "/");
    Config::from_toml_str(&format!(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [local]\ncache_dir = '{}'\nllama_server_path = '{}'\n\
         [[endpoint]]\nid = \"e\"\nprotocol = \"openai\"\n\
         base_url = \"{backend}\"\napi_key = \"\"\n\
         [[model]]\nname = \"remote-model\"\ndescription = \"d\"\n\
         context = 8192\nupstream = \"backend-model\"\nendpoints = [\"e\"]\n\
         [[local_model]]\nname = \"local-model\"\ndescription = \"l\"\n\
         source = '{}'\ncontext = 4096\n\
         [[profile]]\nname = \"main\"\nmodels = [\"local-model\"]\n",
        slash(&temp.path().join("cache")),
        slash(&fake_server),
        slash(&model_file),
    ))
    .expect("config parses")
}

#[cfg(feature = "local")]
async fn status(state: AppState) -> serde_json::Value {
    let response = build_router(state, None)
        .oneshot(
            Request::builder()
                .uri("/admin/status")
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads"),
    )
    .expect("the status body is JSON")
}

/// The boot load over the runner's shell, parked at `phase` on the
/// production worker: the state, the park, the worker, and the enqueued
/// command's outcome.
#[cfg(feature = "local")]
async fn parked_boot_load(
    phase: crate::park::Phase,
) -> (
    AppState,
    Arc<crate::park::PhasePark>,
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Receiver<crate::commands::SharedOutcome>,
    tempfile::TempDir,
) {
    let backend = fake_chat_backend().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let mut state =
        crate::test_support::boot_state(local_profile_config(&temp, &format!("http://{backend}")));
    let park = Arc::new(crate::park::PhasePark::at(phase));
    state.park = Some(Arc::clone(&park));
    let worker = state
        .commands
        .spawn_worker(&state)
        .expect("the production worker spawns");
    let boot = state.commands.enqueue(Command::load_profile(
        ProfileName::parse("main").expect("profile name"),
        CancellationToken::new(),
    ));
    tokio::time::timeout(Duration::from_secs(10), park.entered())
        .await
        .unwrap_or_else(|_| panic!("the boot load parks at {phase:?}"));
    (state, park, worker, boot.outcome, temp)
}

/// After the spawn failed: nothing is promised as loading, the local
/// model is a plain 404, and the remote model keeps routing.
#[cfg(feature = "local")]
async fn assert_settled_after_failed_spawn(state: &AppState) {
    {
        let live = state.live.read().await;
        assert!(
            live.loading.is_empty(),
            "a failed spawn clears the loading set"
        );
        assert!(
            live.routing.model("remote-model").is_ok(),
            "the remote routing stays live after the failed spawn"
        );
        assert!(
            live.local.models().is_empty(),
            "no child is installed after the failed spawn"
        );
    }
    let response = chat(state.clone(), "local-model").await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a model whose spawn failed is a 404, never a lingering 503"
    );
    assert_eq!(
        status(state.clone()).await["loading_models"],
        serde_json::json!([]),
        "the status lists nothing as loading after the spawn"
    );
    assert_eq!(
        chat(state.clone(), "remote-model").await.status(),
        StatusCode::OK,
        "the remote model keeps serving"
    );
}

/// The remote table the runner published serves end to end while the
/// boot load downloads: nothing is promised as loading yet, so the
/// local model answers 503 naming the boot command, and the status
/// names the command as active. Released, the fake llama-server fails
/// the spawn and the local model is a plain 404.
#[cfg(feature = "local")]
#[tokio::test]
async fn a_remote_model_serves_while_the_boot_load_downloads() {
    let (state, park, worker, outcome, _temp) =
        parked_boot_load(crate::park::Phase::Download).await;

    assert_eq!(
        chat(state.clone(), "remote-model").await.status(),
        StatusCode::OK,
        "the remote model serves while the boot load downloads the local one"
    );
    assert!(
        state.live.read().await.loading.is_empty(),
        "nothing is promised as loading before the artifacts are staged"
    );
    let response = chat(state.clone(), "local-model").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
    assert_eq!(json["error"]["code"], "model_provisioning");
    assert_eq!(
        status(state.clone()).await["queue"]["active"]["name"],
        "load-profile: main",
        "the boot load is the active command"
    );

    park.release();
    let outcome = tokio::time::timeout(Duration::from_secs(30), outcome)
        .await
        .expect("the boot load settles")
        .expect("the worker settles the command");
    assert!(
        matches!(
            &*outcome,
            Err(crate::error::GatewayError::PartialStart { failed, .. })
                if failed.iter().any(|entry| entry.starts_with("local-model"))
        ),
        "the fake llama-server cannot start the local model: {outcome:?}"
    );
    assert_settled_after_failed_spawn(&state).await;
    state.commands.shutdown();
    worker.await.expect("the worker exits on shutdown");
}

/// Once the artifacts are staged the local model is promised as
/// loading: it answers 503 `model_loading` with `Retry-After`, the
/// status lists it, the catalog lists only what routes, and the remote
/// model serves throughout. Once the spawn fails the promise is
/// withdrawn to a 404.
#[cfg(feature = "local")]
#[tokio::test]
async fn a_loading_model_answers_503_until_its_spawn_settles() {
    let (state, park, worker, outcome, _temp) = parked_boot_load(crate::park::Phase::Spawn).await;

    assert_eq!(
        state.live.read().await.loading.iter().collect::<Vec<_>>(),
        ["local-model"],
        "the local model is promised as loading during the spawn"
    );
    let response = chat(state.clone(), "local-model").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("5"),
        "the 503 names the wait"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
    assert_eq!(json["error"]["code"], "model_loading");
    let status = status(state.clone()).await;
    assert_eq!(status["loading_models"], serde_json::json!(["local-model"]));
    assert_eq!(status["models"], serde_json::json!(["remote-model"]));
    assert_eq!(
        chat(state.clone(), "remote-model").await.status(),
        StatusCode::OK
    );

    park.release();
    let outcome = tokio::time::timeout(Duration::from_secs(30), outcome)
        .await
        .expect("the boot load settles")
        .expect("the worker settles the command");
    assert!(
        matches!(
            &*outcome,
            Err(crate::error::GatewayError::PartialStart { .. })
        ),
        "the fake llama-server cannot start the local model: {outcome:?}"
    );
    assert_settled_after_failed_spawn(&state).await;
    state.commands.shutdown();
    worker.await.expect("the worker exits on shutdown");
}

/// A cancellation while the download is parked settles the boot load as
/// cancelled and leaves the remote table serving: the local model is a
/// 404, not a lingering 503.
#[cfg(feature = "local")]
#[tokio::test]
async fn a_cancellation_during_the_download_keeps_the_remote_routing() {
    let (state, park, worker, outcome, _temp) =
        parked_boot_load(crate::park::Phase::Download).await;

    assert!(state.commands.cancel_active(), "the boot load is active");
    park.release();
    let outcome = tokio::time::timeout(Duration::from_secs(10), outcome)
        .await
        .expect("the cancelled command settles")
        .expect("the worker settles the command");
    assert!(
        matches!(
            &*outcome,
            Err(crate::error::GatewayError::CommandCancelled(_))
        ),
        "the download honors the cancellation: {outcome:?}"
    );
    assert_settled_after_failed_spawn(&state).await;
    state.commands.shutdown();
    worker.await.expect("the worker exits on shutdown");
}
