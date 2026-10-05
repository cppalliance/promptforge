//! Launch argument rendering tests, companions and serve modes included.

use super::*;

#[test]
fn launch_args_match_local_model_defaults() {
    let args = server_args(
        Path::new("model.gguf"),
        12345,
        "qwen-local",
        "private-key",
        &options(false),
    );
    assert_eq!(
        args,
        expected_args(&[
            "--model",
            "model.gguf",
            "--alias",
            "qwen-local",
            "--api-key",
            "private-key",
            "--host",
            "127.0.0.1",
            "--port",
            "12345",
            "--ctx-size",
            "65536",
            "--n-predict",
            "8192",
            "--parallel",
            "1",
            "--cache-type-k",
            "q8_0",
            "--cache-type-v",
            "q4_0",
            "-ngl",
            "99",
            "--jinja",
            "-lv",
            "4",
            "--flash-attn",
            "on",
            "--reasoning",
            "off",
            "--reasoning-format",
            "auto",
            "--temp",
            "0.7",
            "--top-p",
            "0.8",
            "--top-k",
            "20",
            "--presence-penalty",
            "1.5",
        ])
    );
    let rendered = display_invocation(Path::new("llama-server"), &args);
    assert!(rendered.contains("--api-key <per-attempt-secret>"));
    assert!(!rendered.contains("private-key"));
}

#[test]
fn launch_args_emit_companions_in_pinned_order() {
    // The companion flags sit between the base arguments and the serving-mode
    // flag, in the pinned server's spelling: `--spec-draft-model`,
    // `--spec-type draft-mtp`, `--spec-draft-n-max`, then `--mmproj`.
    let mut opts = options(false);
    opts.speculative = Some(SpeculativeLaunch {
        draft_model: PathBuf::from("draft.gguf"),
        draft_max: 2,
    });
    opts.multimodal_projector = Some(PathBuf::from("mmproj.gguf"));
    let args = server_args(
        Path::new("model.gguf"),
        12345,
        "qwen-local",
        "private-key",
        &opts,
    );
    assert_eq!(
        args,
        expected_args(&[
            "--model",
            "model.gguf",
            "--alias",
            "qwen-local",
            "--api-key",
            "private-key",
            "--host",
            "127.0.0.1",
            "--port",
            "12345",
            "--ctx-size",
            "65536",
            "--n-predict",
            "8192",
            "--parallel",
            "1",
            "--cache-type-k",
            "q8_0",
            "--cache-type-v",
            "q4_0",
            "-ngl",
            "99",
            "--jinja",
            "-lv",
            "4",
            "--spec-draft-model",
            "draft.gguf",
            "--spec-type",
            "draft-mtp",
            "--spec-draft-n-max",
            "2",
            "--mmproj",
            "mmproj.gguf",
            "--flash-attn",
            "on",
            "--reasoning",
            "off",
            "--reasoning-format",
            "auto",
            "--temp",
            "0.7",
            "--top-p",
            "0.8",
            "--top-k",
            "20",
            "--presence-penalty",
            "1.5",
        ])
    );
}

#[test]
fn launch_args_omit_companions_when_unconfigured() {
    // A model without companions emits no speculative or projector flag.
    let args = server_args(Path::new("model.gguf"), 1, "alias", "key", &options(false));
    let rendered = display_invocation(Path::new("llama-server"), &args);
    assert!(!rendered.contains("--spec-draft-model"));
    assert!(!rendered.contains("--spec-type"));
    assert!(!rendered.contains("--spec-draft-n-max"));
    assert!(!rendered.contains("--mmproj"));
}

#[test]
fn companion_args_are_byte_identical_across_respawn_and_shutdown() {
    // The owned paths in `LaunchOptions` are the whole respawn state: the
    // respawn argv must equal the initial argv byte for byte, and an explicit
    // shutdown still terminates the child launched with companions.
    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_log = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&spawn_log);
    let child_id = Arc::new(Mutex::new(None));
    let recorded_id = Arc::clone(&child_id);
    let interrupted = AtomicBool::new(false);

    let mut opts = options(false);
    opts.speculative = Some(SpeculativeLaunch {
        draft_model: PathBuf::from("draft.gguf"),
        draft_max: 2,
    });
    opts.multimodal_projector = Some(PathBuf::from("mmproj.gguf"));

    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &opts,
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.args.to_vec());
            let child = spawn_fake_child(request)?;
            *recorded_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child.id());
            Ok(child)
        }),
    )
    .expect("fake child should become ready");

    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    guard
        .respawn(
            Path::new("fake-llama-server"),
            Path::new("pinned-model.gguf"),
            &opts,
            &AtomicBool::new(false),
        )
        .expect("respawn should become ready on the same port");

    let log = spawn_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(log.len(), 2);
    assert_eq!(log[0], log[1], "respawn argv must equal the initial argv");
    let initial = log[0]
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(initial.contains("--spec-draft-model draft.gguf"));
    assert!(initial.contains("--spec-type draft-mtp"));
    assert!(initial.contains("--spec-draft-n-max 2"));
    assert!(initial.contains("--mmproj mmproj.gguf"));
    drop(log);

    let pid = child_id
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .expect("child id recorded");
    guard.shutdown().expect("shutdown should succeed");
    assert!(
        !process_is_alive(pid),
        "shutdown must terminate the child launched with companions"
    );
}

#[test]
fn launch_args_emit_chat_template_file() {
    let mut opts = options(false);
    opts.chat_template_file = Some(PathBuf::from("mistral-tools.jinja"));
    let args = server_args(Path::new("model.gguf"), 1, "alias", "key", &opts);
    let rendered = display_invocation(Path::new("llama-server"), &args);
    assert!(rendered.contains("--chat-template-file"));
    assert!(rendered.contains("mistral-tools.jinja"));
}

#[test]
fn launch_args_emit_parallel() {
    let mut opts = options(false);
    opts.parallel = 3;
    let args = server_args(Path::new("model.gguf"), 1, "alias", "key", &opts);
    let rendered = display_invocation(Path::new("llama-server"), &args);
    assert!(rendered.contains("--parallel 3"));
    assert!(!rendered.contains("--parallel 1"));
}

#[test]
fn launch_args_emit_embeddings_for_embedding_kind() {
    let mut opts = options(false);
    opts.serve_mode = ServeMode::Embeddings;
    let args = server_args(Path::new("embed.gguf"), 1, "alias", "key", &opts);
    let rendered = display_invocation(Path::new("llama-server"), &args);
    assert!(rendered.contains("--embeddings"));

    let chat_args = server_args(Path::new("model.gguf"), 1, "alias", "key", &options(false));
    let chat_rendered = display_invocation(Path::new("llama-server"), &chat_args);
    assert!(!chat_rendered.contains("--embeddings"));
}

#[test]
fn launch_args_emit_reranking_for_classifier_kind() {
    let mut opts = options(false);
    opts.serve_mode = ServeMode::Reranking;
    let args = server_args(Path::new("rerank.gguf"), 1, "alias", "key", &opts);
    let rendered = display_invocation(Path::new("llama-server"), &args);
    assert!(rendered.contains("--reranking"));

    let chat_args = server_args(Path::new("model.gguf"), 1, "alias", "key", &options(false));
    let chat_rendered = display_invocation(Path::new("llama-server"), &chat_args);
    assert!(!chat_rendered.contains("--reranking"));
}

#[test]
fn thinking_preset_omits_reasoning_off() {
    let args = server_args(Path::new("model.gguf"), 1, "alias", "key", &options(true));
    let rendered = display_invocation(Path::new("llama-server"), &args);
    assert!(!rendered.contains("--reasoning off"));
    assert!(rendered.contains("--temp 1.0"));
    assert!(rendered.contains("--top-p 0.95"));
}
