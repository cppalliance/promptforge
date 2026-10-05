//! Config writes after the boot speech load: a profile switch, a save of
//! the served document, and an Apply of speech settings, each leaving the
//! boot runtime serving.

use super::*;

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
        "the running document omits the active_profile key: {document}"
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
