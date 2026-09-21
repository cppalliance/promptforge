//! Boot-load speech behavior: staged STT loads against scripted workers, through the real router.
//!
//! The boot `LoadProfile` command owns the process's one STT load: the
//! control plane serves complete responses while that load is parked,
//! speech becomes ready when it completes, and a later switch or Apply
//! persists new speech state without touching the running runtime.

#![expect(
    clippy::expect_used,
    reason = "boot-path integration fixtures fail with the invariant named"
)]

use std::time::Duration;

use gateway_config::{Config, ProfileName};
use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_util::sync::CancellationToken;

use crate::commands::Command;
use crate::test_support::{
    AdminPaths, GatedConstructionFactory, arm_boot_speech, boot_state_with_paths,
    fake_chat_backend, serve_state, wait_until,
};

const WAIT: Duration = Duration::from_secs(10);

/// A catalog with one remote model per profile on the fake backend.
fn catalog(backend: std::net::SocketAddr) -> String {
    format!(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [stt]\nwindow_seconds = 8\ninterval_ms = 250\nvocabulary = [\"alpha-words\"]\n\
         [[endpoint]]\nid = \"e\"\nprotocol = \"openai\"\nbase_url = \"http://{backend}\"\napi_key = \"\"\n\
         [[model]]\nname = \"alpha-model\"\ndescription = \"a\"\ncontext = 1024\nupstream = \"backend-model\"\nendpoints = [\"e\"]\n\
         [[model]]\nname = \"beta-model\"\ndescription = \"b\"\ncontext = 1024\nupstream = \"backend-model\"\nendpoints = [\"e\"]\n\
         [[profile]]\nname = \"alpha\"\nmodels = []\n\
         [[profile]]\nname = \"beta\"\nmodels = []\n"
    )
}

/// Writes the catalog and its state file, returning the fixture pieces.
fn persisted_catalog(
    temp: &tempfile::TempDir,
    backend: std::net::SocketAddr,
    active: &str,
) -> (Config, AdminPaths) {
    let config_path = temp.path().join("gateway.toml");
    std::fs::write(&config_path, catalog(backend)).expect("write catalog");
    std::fs::write(
        gateway_config::profile_state_path(&config_path),
        format!("active_profile = \"{active}\"\n"),
    )
    .expect("write state");
    let config = Config::load(
        &config_path,
        &gateway_config::ProfileSelection::new(Some(active), None),
    )
    .expect("load the active profile");
    let paths = AdminPaths {
        fixture_dir: temp.path().to_path_buf(),
        active: active.to_owned(),
        config_path,
    };
    (config, paths)
}

async fn get(addr: std::net::SocketAddr, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("http://{addr}{path}"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("GET sends")
}

async fn get_json(addr: std::net::SocketAddr, path: &str) -> serde_json::Value {
    let response = get(addr, path).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK, "GET {path}");
    response.json().await.expect("JSON body")
}

/// One batch transcription through the served router.
async fn transcribe(addr: std::net::SocketAddr) -> reqwest::Response {
    const BOUNDARY: &str = "boot-speech-boundary";
    let wav = [
        b'R', b'I', b'F', b'F', 38, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ', 16, 0,
        0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0x00, 0x7d, 0, 0, 2, 0, 16, 0, b'd', b'a', b't', b'a',
        2, 0, 0, 0, 0, 32,
    ];
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n\
         scripted-interim\r\n\
         --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"sample.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&wav);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/audio/transcriptions"))
        .bearer_auth("test-token")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(body)
        .send()
        .await
        .expect("batch transcription sends")
}

/// The Realtime upgrade's status code: 101 once speech is ready with a
/// final pass, 503 before. A successful upgrade is closed explicitly so
/// the server-side session releases its admission before shutdown.
async fn realtime_upgrade_status(addr: std::net::SocketAddr) -> u16 {
    let mut request = format!("ws://{addr}/v1/realtime?intent=transcription")
        .into_client_request()
        .expect("Realtime request builds");
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_static("Bearer test-token"),
    );
    match tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(request))
        .await
        .expect("the upgrade answers before the deadline")
    {
        Ok((mut socket, response)) => {
            let status = response.status().as_u16();
            socket.close(None).await.expect("the socket closes");
            status
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(other) => panic!("expected an upgrade or a refusal, got {other:?}"),
    }
}

fn speech_models(state: &crate::AppState) -> Vec<String> {
    state
        .speech
        .models()
        .iter()
        .map(|model| model.name().to_owned())
        .collect()
}

/// While the boot command is parked inside its STT load, every
/// control-plane and ready non-STT route answers completely; releasing
/// the load brings batch and Realtime to readiness.
///
/// Multi-threaded: the closing `speech.shutdown()` blocks its calling
/// thread while the served realtime session's cleanup runs on another.
#[cfg(feature = "config-ui")]
#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "the single linear scenario probes every control-plane surface across the same parked-load boundary"
)]
async fn the_parked_boot_speech_load_never_blocks_the_control_plane() {
    let backend = fake_chat_backend().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let (config, paths) = persisted_catalog(&temp, backend, "alpha");
    let interim = ScriptedDecoder::new();
    interim.push_text("boot transcript");
    let gate = GatedConstructionFactory::new(
        ScriptedModelFactory::new(interim).with_final(ScriptedDecoder::new()),
    );
    let mut state = boot_state_with_paths(config, paths);
    arm_boot_speech(&mut state, gate.clone());
    let addr = serve_state(state.clone()).await;
    let worker = state.commands.spawn_worker(&state).expect("worker spawns");
    let boot = state.commands.enqueue(Command::load_profile(
        ProfileName::parse("alpha").expect("profile name"),
        CancellationToken::new(),
    ));
    wait_until("the boot command to park inside the speech load", || {
        gate.entered()
    })
    .await;

    // The runner published the remote table before the boot load ran:
    // the remote model routes and answers end to end.
    let chat = reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({
            "model": "alpha-model",
            "messages": [{"role": "user", "content": "ping"}]
        }))
        .send()
        .await
        .expect("chat sends");
    assert_eq!(chat.status(), reqwest::StatusCode::OK, "chat completes");
    let catalog = get_json(addr, "/v1/models").await;
    assert_eq!(
        catalog["data"]
            .as_array()
            .expect("catalog data")
            .iter()
            .map(|model| model["id"].as_str().expect("model id"))
            .collect::<Vec<_>>(),
        ["alpha-model", "beta-model"],
        "only the ready non-STT models are advertised"
    );

    let health = get(addr, "/health").await;
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    let status = get_json(addr, "/admin/status").await;
    assert_eq!(status["profile"], "alpha");
    assert_eq!(
        status["speech"],
        serde_json::json!({"configured": false, "ready": false, "gpu": false}),
        "speech is not ready while its load is parked"
    );
    assert_eq!(
        status["queue"]["active"]["name"], "load-profile: alpha",
        "the queue status names the boot command doing the load"
    );

    let progress = get(addr, "/admin/progress").await;
    assert_eq!(progress.status(), reqwest::StatusCode::OK);
    assert_eq!(
        progress
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
        "the progress stream serves while the load is parked"
    );

    let config_body = get_json(addr, "/admin/config").await;
    assert!(
        config_body.get("active_profile").is_none(),
        "the running document never names the profile; status does"
    );
    let profiles = get_json(addr, "/admin/profiles").await;
    assert_eq!(profiles["profiles"], serde_json::json!(["alpha", "beta"]));

    for (path, content_type) in [
        ("/config/", "text/html"),
        ("/config/app.js", "text/javascript"),
        ("/config/app.css", "text/css"),
        ("/config/icons/promptforge-icon.png", "image/png"),
    ] {
        let response = get(addr, path).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "GET {path}");
        let served = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .expect("content type")
            .to_owned();
        assert!(
            served.starts_with(content_type),
            "GET {path} serves {content_type}, got {served}"
        );
        assert!(
            !response.bytes().await.expect("asset body").is_empty(),
            "GET {path} serves a complete body"
        );
    }

    // Speech routes keep their unavailable behavior while parked.
    let batch = transcribe(addr).await;
    assert_eq!(
        batch.status(),
        reqwest::StatusCode::NOT_FOUND,
        "batch transcription has no model before the load completes"
    );
    assert_eq!(realtime_upgrade_status(addr).await, 503);

    // Release the load: the boot command settles and speech is ready.
    gate.release();
    let outcome = tokio::time::timeout(WAIT, boot.outcome)
        .await
        .expect("the boot command settles")
        .expect("the worker settles the command");
    assert!(outcome.is_ok(), "the boot command succeeds: {outcome:?}");

    let status = get_json(addr, "/admin/status").await;
    assert_eq!(
        status["speech"],
        serde_json::json!({"configured": true, "ready": true, "gpu": false})
    );
    let catalog = get_json(addr, "/v1/models").await;
    assert_eq!(
        catalog["data"]
            .as_array()
            .expect("catalog data")
            .iter()
            .map(|model| model["id"].as_str().expect("model id"))
            .collect::<Vec<_>>(),
        [
            "alpha-model",
            "beta-model",
            "scripted-interim",
            "scripted-final",
            "realtime-transcribe"
        ]
    );
    let batch = transcribe(addr).await;
    assert_eq!(batch.status(), reqwest::StatusCode::OK, "batch is ready");
    let body: serde_json::Value = batch.json().await.expect("batch body");
    assert_eq!(body["text"], "boot transcript");
    assert_eq!(
        realtime_upgrade_status(addr).await,
        101,
        "Realtime is ready"
    );

    state.commands.shutdown();
    worker.await.expect("the worker exits on shutdown");
    state.speech.shutdown();
}

/// A failed boot STT load fails the boot command but never the gateway:
/// the boot profile keeps serving, speech stays unavailable, and a
/// later switch persists its selection without retrying the load or
/// emitting a speech stage.
#[tokio::test]
async fn a_failed_boot_speech_load_leaves_the_gateway_serving_without_speech() {
    let backend = fake_chat_backend().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let (config, paths) = persisted_catalog(&temp, backend, "alpha");
    let mut state = boot_state_with_paths(config, paths);
    arm_boot_speech(
        &mut state,
        ScriptedModelFactory::new(ScriptedDecoder::new())
            .with_interim_failure("boot speech sentinel"),
    );
    let addr = serve_state(state.clone()).await;
    let worker = state.commands.spawn_worker(&state).expect("worker spawns");
    let boot = state.commands.enqueue(Command::load_profile(
        ProfileName::parse("alpha").expect("profile name"),
        CancellationToken::new(),
    ));

    let outcome = tokio::time::timeout(WAIT, boot.outcome)
        .await
        .expect("the boot command settles")
        .expect("the worker settles the command");
    let chain = match &*outcome {
        Ok(profile) => panic!("the STT failure fails the boot command, got {profile}"),
        Err(error) => crate::error::error_chain(error),
    };
    assert!(chain.contains("load-speech"), "the stage is named: {chain}");
    assert!(
        chain.contains("boot speech sentinel"),
        "the cause is preserved: {chain}"
    );
    assert!(
        !state.shutdown.is_fired(),
        "an STT failure never requests gateway shutdown"
    );

    let health = get(addr, "/health").await;
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    let status = get_json(addr, "/admin/status").await;
    assert_eq!(status["profile"], "alpha", "the boot profile serves");
    assert_eq!(
        status["speech"],
        serde_json::json!({"configured": false, "ready": false, "gpu": false})
    );
    let catalog = get_json(addr, "/v1/models").await;
    assert_eq!(
        catalog["data"]
            .as_array()
            .expect("catalog data")
            .iter()
            .map(|model| model["id"].as_str().expect("model id"))
            .collect::<Vec<_>>(),
        ["alpha-model", "beta-model"],
        "speech discovery stays empty until restart"
    );

    // A later switch persists its selection without a speech stage and
    // without retrying the spent initial load.
    let switching = reqwest::Client::new()
        .post(format!("http://{addr}/admin/switch-profile"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "name": "beta" }))
        .send()
        .await
        .expect("the switch request sends");
    assert_eq!(switching.status(), reqwest::StatusCode::OK);
    assert!(
        !state.hub.current().busy,
        "a switch runs no command, so no speech stage begins: {:?}",
        state.hub.current()
    );
    assert!(!state.speech.status().ready());

    state.commands.shutdown();
    worker.await.expect("the worker exits on shutdown");
}

/// Cancelling the boot command while its STT load is parked settles the
/// command as cancelled, spends the one attempt, and lets the worker
/// exit on queue shutdown.
#[tokio::test]
async fn cancelling_the_parked_boot_speech_load_settles_cancelled() {
    let backend = fake_chat_backend().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let (config, paths) = persisted_catalog(&temp, backend, "alpha");
    let gate = GatedConstructionFactory::new(ScriptedModelFactory::new(ScriptedDecoder::new()));
    let mut state = boot_state_with_paths(config, paths);
    arm_boot_speech(&mut state, gate.clone());
    let addr = serve_state(state.clone()).await;
    let worker = state.commands.spawn_worker(&state).expect("worker spawns");
    let boot = state.commands.enqueue(Command::load_profile(
        ProfileName::parse("alpha").expect("profile name"),
        CancellationToken::new(),
    ));
    wait_until("the boot command to park inside the speech load", || {
        gate.entered()
    })
    .await;

    assert!(state.commands.cancel_active(), "the boot command is active");
    gate.release();
    let outcome = tokio::time::timeout(WAIT, boot.outcome)
        .await
        .expect("the cancelled command settles")
        .expect("the worker settles the command");
    assert!(
        matches!(
            &*outcome,
            Err(crate::error::GatewayError::CommandCancelled(_))
        ),
        "the cancelled speech load settles the command as cancelled: {outcome:?}"
    );
    assert!(
        !state.speech.status().ready(),
        "the cancelled load published nothing"
    );
    let health = get(addr, "/health").await;
    assert_eq!(health.status(), reqwest::StatusCode::OK);

    state.commands.shutdown();
    tokio::time::timeout(WAIT, worker)
        .await
        .expect("the worker exits on shutdown")
        .expect("the worker task joins");
}

/// Boot speech A keeps serving after a later switch persists B and
/// reports a restart; a fresh process state over the persisted
/// selection loads B.
#[tokio::test]
async fn a_later_switch_persists_b_while_boot_speech_a_keeps_serving() {
    let backend = fake_chat_backend().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let (config, paths) = persisted_catalog(&temp, backend, "alpha");
    let config_path = paths.config_path.clone();
    let interim = ScriptedDecoder::new();
    interim.push_text("boot alpha transcript");
    let mut state = boot_state_with_paths(config, paths);
    arm_boot_speech(&mut state, ScriptedModelFactory::new(interim));
    let addr = serve_state(state.clone()).await;
    let worker = state.commands.spawn_worker(&state).expect("worker spawns");
    let boot = state.commands.enqueue(Command::load_profile(
        ProfileName::parse("alpha").expect("profile name"),
        CancellationToken::new(),
    ));
    let outcome = tokio::time::timeout(WAIT, boot.outcome)
        .await
        .expect("the boot command settles")
        .expect("the worker settles the command");
    assert!(outcome.is_ok(), "boot A loads: {outcome:?}");
    assert_eq!(speech_models(&state), ["scripted-interim"]);

    // Select beta through the admin route: the selection persists and
    // the reply asks for a restart; nothing running changes.
    let switching = reqwest::Client::new()
        .post(format!("http://{addr}/admin/switch-profile"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "name": "beta" }))
        .send()
        .await
        .expect("the switch request sends");
    assert_eq!(switching.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = switching.json().await.expect("the reply is JSON");
    assert_eq!(
        body,
        serde_json::json!({ "profile": "beta", "restart_required": true }),
        "the switch to beta persists and reports a restart"
    );

    assert_eq!(
        std::fs::read_to_string(gateway_config::profile_state_path(&config_path))
            .expect("read state"),
        "active_profile = \"beta\"\n",
        "the switch persisted B"
    );
    assert_eq!(
        speech_models(&state),
        ["scripted-interim"],
        "the running process stays on boot speech A"
    );
    let batch = transcribe(addr).await;
    assert_eq!(batch.status(), reqwest::StatusCode::OK);
    let batch_body: serde_json::Value = batch.json().await.expect("batch body");
    assert_eq!(
        batch_body["text"], "boot alpha transcript",
        "boot speech A still serves"
    );

    state.commands.shutdown();
    worker.await.expect("the worker exits on shutdown");
    state.speech.shutdown();

    // The restart: a fresh process state over the persisted selection
    // loads B through its own boot command.
    let config = Config::load(&config_path, &gateway_config::ProfileSelection::default())
        .expect("the persisted selection loads");
    let paths = AdminPaths {
        fixture_dir: temp.path().to_path_buf(),
        active: "beta".to_owned(),
        config_path: config_path.clone(),
    };
    let mut restarted = boot_state_with_paths(config, paths);
    arm_boot_speech(
        &mut restarted,
        ScriptedModelFactory::new(ScriptedDecoder::new()).with_final(ScriptedDecoder::new()),
    );
    let worker = restarted
        .commands
        .spawn_worker(&restarted)
        .expect("worker spawns");
    let boot = restarted.commands.enqueue(Command::load_profile(
        ProfileName::parse("beta").expect("profile name"),
        CancellationToken::new(),
    ));
    let outcome = tokio::time::timeout(WAIT, boot.outcome)
        .await
        .expect("the restart boot command settles")
        .expect("the worker settles the command");
    assert!(outcome.is_ok(), "the restart boot loads: {outcome:?}");
    assert_eq!(
        speech_models(&restarted),
        ["scripted-interim", "scripted-final", "realtime-transcribe"],
        "the new process loads B"
    );
    restarted.commands.shutdown();
    worker.await.expect("the worker exits on shutdown");
    restarted.speech.shutdown();
}

/// The document `GET /admin/config` serves is accepted verbatim by
/// `PUT /admin/config`: the running document has no
/// `active_profile` key for the save route to refuse.
#[tokio::test]
async fn the_served_config_round_trips_through_the_save_route() {
    let backend = fake_chat_backend().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let (config, paths) = persisted_catalog(&temp, backend, "alpha");
    let addr = serve_state(boot_state_with_paths(config, paths)).await;

    let document = get_json(addr, "/admin/config").await;
    assert!(
        document.get("active_profile").is_none(),
        "the running document carries no active_profile key: {document}"
    );
    let save = reqwest::Client::new()
        .put(format!("http://{addr}/admin/config"))
        .bearer_auth("test-token")
        .json(&document)
        .send()
        .await
        .expect("the save sends");
    assert_eq!(
        save.status(),
        reqwest::StatusCode::OK,
        "the served document saves unchanged"
    );
    let reply: serde_json::Value = save.json().await.expect("save body");
    assert!(
        reply.get("shadow").is_some(),
        "the reply names the config shadow: {reply}"
    );
}

/// An Apply that persists an STT settings change leaves the running
/// boot runtime untouched and emits no speech stage.
#[tokio::test]
async fn an_apply_persisting_speech_changes_leaves_boot_speech_untouched() {
    let backend = fake_chat_backend().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let (config, paths) = persisted_catalog(&temp, backend, "alpha");
    let config_path = paths.config_path.clone();
    let interim = ScriptedDecoder::new();
    interim.push_text("boot alpha transcript");
    let mut state = boot_state_with_paths(config, paths);
    arm_boot_speech(&mut state, ScriptedModelFactory::new(interim));
    let addr = serve_state(state.clone()).await;
    let worker = state.commands.spawn_worker(&state).expect("worker spawns");
    let boot = state.commands.enqueue(Command::load_profile(
        ProfileName::parse("alpha").expect("profile name"),
        CancellationToken::new(),
    ));
    let outcome = tokio::time::timeout(WAIT, boot.outcome)
        .await
        .expect("the boot command settles")
        .expect("the worker settles the command");
    assert!(outcome.is_ok(), "boot A loads: {outcome:?}");

    // Stage an STT vocabulary change through the real save route.
    let mut document = get_json(addr, "/admin/config").await;
    document["stt"]["vocabulary"] = serde_json::json!(["beta-words"]);
    let save = reqwest::Client::new()
        .put(format!("http://{addr}/admin/config"))
        .bearer_auth("test-token")
        .json(&document)
        .send()
        .await
        .expect("the save sends");
    assert_eq!(save.status(), reqwest::StatusCode::OK);

    let apply = reqwest::Client::new()
        .post(format!("http://{addr}/admin/config-apply"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("the apply sends");
    assert_eq!(apply.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = apply.json().await.expect("apply body");
    assert_eq!(reply["reloaded"], true);
    assert_eq!(
        reply["restart_required"], true,
        "the speech pipeline is read once at boot"
    );
    assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));

    assert!(
        std::fs::read_to_string(&config_path)
            .expect("read applied config")
            .contains("beta-words"),
        "the apply persisted the new speech setting"
    );
    assert_eq!(
        speech_models(&state),
        ["scripted-interim"],
        "the running process stays on boot speech A"
    );
    let batch = transcribe(addr).await;
    let batch_body: serde_json::Value = batch.json().await.expect("batch body");
    assert_eq!(batch_body["text"], "boot alpha transcript");
    let statuses = state.commands.active_command();
    assert!(statuses.is_none(), "the apply command settled");
    assert!(
        !state.hub.current().busy,
        "the settled apply left no activity behind: {:?}",
        state.hub.current()
    );

    state.commands.shutdown();
    worker.await.expect("the worker exits on shutdown");
    state.speech.shutdown();
}
