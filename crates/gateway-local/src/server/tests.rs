use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

#[cfg(windows)]
use super::support::production_command;
use super::support::{BoundedCapture, ChildSpawner};
use super::*;

const TEST_PORT: &str = "PROMPTFORGE_GATEWAY_TEST_LLAMA_PORT";
const TEST_MODEL_ALIAS: &str = "PROMPTFORGE_GATEWAY_TEST_LLAMA_MODEL_ALIAS";
const TEST_API_KEY: &str = "PROMPTFORGE_GATEWAY_TEST_LLAMA_API_KEY";
const TEST_POLICY: StartupPolicy = StartupPolicy {
    attempts: 2,
    deadline: Duration::from_secs(5),
    interval: Duration::from_millis(10),
    http_timeout: Duration::from_millis(100),
};

fn options(think: bool) -> LaunchOptions {
    LaunchOptions {
        ctx_size: 65_536,
        n_predict: 8192,
        parallel: 1,
        gpu_layers: 99,
        flash_attention: true,
        cache_type_k: "q8_0".to_owned(),
        cache_type_v: "q4_0".to_owned(),
        think,
        chat_template_file: None,
        serve_mode: ServeMode::Chat,
        speculative: None,
        multimodal_projector: None,
        path_prefix: Vec::new(),
    }
}

fn expected_args(pieces: &[&str]) -> Vec<OsString> {
    pieces.iter().map(OsString::from).collect()
}

struct FakeHttpServer {
    port: u16,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeHttpServer {
    fn start(model_alias: &str) -> Self {
        // Blocking listener: bound before the accept thread starts, so early
        // client connections are held in the kernel backlog (no startup race,
        // no startup sleep), and blocking `accept` needs no WouldBlock poll
        // loop. `Drop` wakes the final blocking `accept` with a self-connect.
        let listener = TcpListener::bind((LOOPBACK, 0)).expect("bind unrelated fake listener");
        let port = listener
            .local_addr()
            .expect("read unrelated fake listener address")
            .port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let model_alias = model_alias.to_owned();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if thread_shutdown.load(Ordering::Acquire) {
                    break;
                }
                match stream {
                    Ok(stream) => respond(stream, &model_alias, None),
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            shutdown,
            thread: Some(thread),
        }
    }
}

impl Drop for FakeHttpServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ignored = TcpStream::connect((LOOPBACK, self.port));
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                result.expect("join unrelated fake listener");
            }
        }
    }
}

fn respond(mut stream: TcpStream, model_alias: &str, required_api_key: Option<&str>) {
    let _ignored = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let mut request = [0_u8; 4096];
    let Ok(count) = stream.read(&mut request) else {
        return;
    };
    let request = String::from_utf8_lossy(&request[..count]);
    let authorized = required_api_key.is_none_or(|api_key| {
        request
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {api_key}")))
    });
    let (status, body) = if !authorized {
        ("401 Unauthorized", r#"{"error":"unauthorized"}"#.to_owned())
    } else if request.starts_with("GET /health ") {
        ("200 OK", r#"{"status":"ok"}"#.to_owned())
    } else if request.starts_with("GET /v1/models ") {
        (
            "200 OK",
            format!(r#"{{"data":[{{"id":"{model_alias}"}}]}}"#),
        )
    } else if request.starts_with("POST /v1/chat/completions ")
        || request.starts_with("POST /chat/completions ")
    {
        (
            "200 OK",
            format!(
                r#"{{"model":"{model_alias}","choices":[{{"index":0,"message":{{"role":"assistant","content":"ok"}}}}]}}"#
            ),
        )
    } else if request.starts_with("POST /v1/embeddings ")
        || request.starts_with("POST /embeddings ")
    {
        (
            "200 OK",
            format!(
                r#"{{"object":"list","model":"{model_alias}","data":[{{"object":"embedding","index":0,"embedding":[0.1,0.2,0.3]}}],"usage":{{"prompt_tokens":2,"total_tokens":2}}}}"#
            ),
        )
    } else if request.starts_with("POST /v1/rerank ") || request.starts_with("POST /rerank ") {
        (
            "200 OK",
            format!(
                r#"{{"model":"{model_alias}","results":[{{"index":1,"relevance_score":0.9}},{{"index":0,"relevance_score":0.1}}],"usage":{{"total_tokens":12}}}}"#
            ),
        )
    } else {
        ("404 Not Found", r#"{"error":"not found"}"#.to_owned())
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ignored = stream.write_all(response.as_bytes());
}

fn deterministic_identity(index: usize) -> AttemptIdentity {
    AttemptIdentity {
        model_alias: format!("promptforge-test-model-{index}"),
        api_key: format!("promptforge-test-key-{index}"),
    }
}

fn spawn_fake_child(request: &SpawnRequest<'_>) -> Result<Child> {
    spawn_child_serving(request, request.model_alias)
}

/// Spawns the fake worker serving `model_alias`, which may differ from the
/// attempt's alias so authenticated readiness never belongs to the child.
fn spawn_child_serving(request: &SpawnRequest<'_>, model_alias: &str) -> Result<Child> {
    let executable = std::env::current_exe().map_err(|source| LocalError::Spawn {
        executable: PathBuf::from("<test-executable>"),
        source,
    })?;
    Command::new(&executable)
        .args([
            "--exact",
            "server::tests::fake_llama_server_worker",
            "--ignored",
            "--nocapture",
        ])
        .env(TEST_PORT, request.port.to_string())
        .env(TEST_MODEL_ALIAS, model_alias)
        .env(TEST_API_KEY, request.api_key)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| LocalError::Spawn { executable, source })
}

#[test]
#[ignore = "subprocess worker invoked by startup regression tests"]
fn fake_llama_server_worker() {
    let (Ok(port), Ok(model_alias), Ok(api_key)) = (
        std::env::var(TEST_PORT),
        std::env::var(TEST_MODEL_ALIAS),
        std::env::var(TEST_API_KEY),
    ) else {
        return;
    };
    let Ok(port) = port.parse::<u16>() else {
        return;
    };
    let Ok(listener) = TcpListener::bind((LOOPBACK, port)) else {
        return;
    };
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            break;
        };
        respond(stream, &model_alias, Some(&api_key));
    }
}

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
    // A model without companions emits exactly the pre-companion command line:
    // no speculative or projector flag may appear.
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
    // shutdown still terminates the companion-carrying child.
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
        None,
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
        "shutdown must terminate the companion-carrying child"
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

#[test]
fn captured_diagnostics_keep_only_the_bounded_tail() {
    let mut capture = BoundedCapture::new(8);
    capture.append(b"abcdef");
    capture.append(b"ghijkl");
    assert_eq!(capture.render(), "[4 earlier bytes omitted]\nefghijkl");
}

#[test]
fn attempt_identity_and_spawn_request_debug_redact_the_token() {
    // HYGIENE-SECRET-DEBUG-001/002: neither struct's Debug may render the token.
    const TOKEN: &str = "super-secret-per-attempt-token";

    let identity = AttemptIdentity {
        model_alias: "promptforge-local-alias".to_owned(),
        api_key: TOKEN.to_owned(),
    };
    let rendered = format!("{identity:?}");
    assert!(
        !rendered.contains(TOKEN),
        "identity leaked token: {rendered}"
    );
    assert!(rendered.contains(API_KEY_REDACTION));

    let args = server_args(
        Path::new("model.gguf"),
        4242,
        "promptforge-local-alias",
        TOKEN,
        &options(false),
    );
    let request = SpawnRequest {
        executable: Path::new("llama-server"),
        args: &args,
        path_prefix: &[],
        port: 4242,
        model_alias: "promptforge-local-alias",
        api_key: TOKEN,
    };
    let rendered = format!("{request:?}");
    assert!(
        !rendered.contains(TOKEN),
        "spawn request leaked token: {rendered}"
    );
    assert!(rendered.contains(API_KEY_REDACTION));
    // The non-secret alias still renders, so redaction is targeted.
    assert!(rendered.contains("promptforge-local-alias"));
}

#[test]
fn debug_redacts_api_key() {
    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let interrupted = AtomicBool::new(false);
    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(spawn_fake_child),
    )
    .expect("fake child should become ready");
    let key = guard.api_key().to_owned();
    let rendered = format!("{guard:?}");
    assert!(!rendered.contains(&key));
    assert!(rendered.contains("Secret(redacted)"));
}

#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn ready_leaf_completes_when_the_readiness_poll_succeeds() {
    // The readiness leaf is indeterminate: it jumps from 0.0 to 1.0 exactly
    // once, when the bounded poll confirms authenticated readiness.
    let hub = Arc::new(shared_progress::ProgressHub::new());
    let tree = hub.operation();
    let ready = tree.register("ready", 1.0);
    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let interrupted = AtomicBool::new(false);
    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        Some(&ready),
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(spawn_fake_child),
    )
    .expect("fake child should become ready");
    assert_eq!(ready.fraction(), 1.0);
    drop(guard);
}

#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn ready_leaf_stays_unfinished_when_readiness_never_arrives() {
    // A child serving another attempt's alias never passes the identity
    // check, so the poll times out and the leaf is never completed.
    const FAST_POLICY: StartupPolicy = StartupPolicy {
        attempts: 1,
        deadline: Duration::from_millis(300),
        interval: Duration::from_millis(10),
        http_timeout: Duration::from_millis(50),
    };
    let hub = Arc::new(shared_progress::ProgressHub::new());
    let tree = hub.operation();
    let ready = tree.register("ready", 1.0);
    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let interrupted = AtomicBool::new(false);
    let error = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        Some(&ready),
        FAST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(|request: &SpawnRequest<'_>| {
            spawn_child_serving(request, "someone-else")
        }),
    )
    .expect_err("a foreign alias must never become ready");
    assert!(matches!(error, LocalError::Startup { .. }));
    assert_eq!(ready.fraction(), 0.0);
}

#[test]
fn retries_after_foreign_health_listener_wins_selected_port() {
    let foreign = FakeHttpServer::start("Qwen3-0.6B-Q8_0.gguf");
    let fresh_port = free_port().expect("select retry port");
    let mut ports = VecDeque::from([foreign.port, fresh_port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut identity_index = 0;
    let mut make_identity = || {
        let identity = deterministic_identity(identity_index);
        identity_index += 1;
        identity
    };
    let attempted_ports = Arc::new(Mutex::new(Vec::new()));
    let recorded_ports = Arc::clone(&attempted_ports);
    let interrupted = AtomicBool::new(false);

    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            recorded_ports
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.port);
            spawn_fake_child(request)
        }),
    )
    .expect("retry should reach the spawned fake server");

    assert_eq!(
        *attempted_ports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        [foreign.port, fresh_port]
    );
    assert_eq!(guard.port, fresh_port);
    assert_eq!(guard.model_alias(), "promptforge-test-model-1");
    assert_eq!(guard.api_key(), "promptforge-test-key-1");
}

#[test]
fn drop_kills_the_child_process() {
    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let child_id = Arc::new(Mutex::new(None));
    let recorded_id = Arc::clone(&child_id);
    let interrupted = AtomicBool::new(false);

    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            let child = spawn_fake_child(request)?;
            *recorded_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child.id());
            Ok(child)
        }),
    )
    .expect("fake child should become ready");
    assert!(listener_is_present(port, Duration::from_millis(100)));
    let id = child_id
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .expect("child id recorded");
    drop(guard);

    assert!(
        !process_is_alive(id),
        "ServerGuard Drop must kill the llama-server child"
    );
    assert!(!listener_is_present(port, Duration::from_millis(100)));
}

#[test]
fn respawn_reuses_port_and_identity_after_child_death() {
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
    let interrupted = AtomicBool::new(false);

    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    request.port,
                    request.model_alias.to_owned(),
                    request.api_key.to_owned(),
                ));
            spawn_fake_child(request)
        }),
    )
    .expect("fake child should become ready");

    let alias = guard.model_alias().to_owned();
    let key = guard.api_key().to_owned();
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    guard
        .respawn(
            Path::new("fake-llama-server"),
            Path::new("pinned-model.gguf"),
            &options(false),
            &AtomicBool::new(false),
        )
        .expect("respawn should become ready on the same port");

    assert_eq!(guard.port(), port);
    assert_eq!(guard.model_alias(), alias);
    assert_eq!(guard.api_key(), key);
    assert!(guard.is_running().expect("inspect respawned child"));
    let log = spawn_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(log.len(), 2);
    assert_eq!(log[0], (port, alias.clone(), key.clone()));
    assert_eq!(log[1], (port, alias, key));
}

#[test]
fn local_upstream_send_respawns_dead_child_once() {
    use crate::upstream::LocalUpstream;
    use serde_json::Map;
    use shared_protocol::upstream::Upstream;
    use shared_protocol::wire::ChatRequest;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let spawn_log = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&spawn_log);
    let interrupted = AtomicBool::new(false);

    // Blocking readiness must run outside a Tokio async context.
    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            *counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    request.port,
                    request.model_alias.to_owned(),
                    request.api_key.to_owned(),
                ));
            spawn_fake_child(request)
        }),
    )
    .expect("fake child should become ready");

    let alias = guard.model_alias().to_owned();
    let key = guard.api_key().to_owned();
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let response = runtime
        .block_on(upstream.send(
            ChatRequest {
                model: "qwen-local".to_owned(),
                messages: Vec::new(),
                stream: false,
                rest: Map::new(),
            },
            &alias,
        ))
        .expect("send should respawn and succeed");

    assert_eq!(response.model, "qwen-local");
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        2
    );
    let log = spawn_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(log[0], (port, alias.clone(), key.clone()));
    assert_eq!(log[1], (port, alias, key));
}

#[test]
fn local_upstream_send_embeddings_routes_through_child() {
    // An embeddings request forwards to the child's `/v1/embeddings` and the
    // response restores the caller's model name, same contract as chat.
    use crate::upstream::LocalUpstream;
    use serde_json::Map;
    use shared_protocol::upstream::Upstream;
    use shared_protocol::wire::{EmbeddingInput, EmbeddingRequest};

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let interrupted = AtomicBool::new(false);

    let mut opts = options(false);
    opts.serve_mode = ServeMode::Embeddings;
    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-embed.gguf"),
        &opts,
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(spawn_fake_child),
    )
    .expect("fake child should become ready");
    let alias = guard.model_alias().to_owned();

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-embed.gguf"),
        opts,
        "bge-local".to_owned(),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let response = runtime
        .block_on(upstream.send_embeddings(
            EmbeddingRequest {
                model: "bge-local".to_owned(),
                input: EmbeddingInput::One("embed me".to_owned()),
                encoding_format: None,
                rest: Map::new(),
            },
            &alias,
        ))
        .expect("embeddings send should succeed through the child");

    assert_eq!(response.model, "bge-local");
    assert_eq!(response.data.len(), 1);
    assert_eq!(
        response.data[0].pointer("/embedding"),
        Some(&serde_json::json!([0.1, 0.2, 0.3]))
    );
}

#[test]
fn local_upstream_send_rerank_routes_through_child() {
    // A rerank request forwards to the child's `/v1/rerank` and the response
    // restores the caller's model name, same contract as chat.
    use crate::upstream::LocalUpstream;
    use serde_json::Map;
    use shared_protocol::upstream::Upstream;
    use shared_protocol::wire::RerankRequest;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let interrupted = AtomicBool::new(false);

    let mut opts = options(false);
    opts.serve_mode = ServeMode::Reranking;
    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-rerank.gguf"),
        &opts,
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(spawn_fake_child),
    )
    .expect("fake child should become ready");
    let alias = guard.model_alias().to_owned();

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-rerank.gguf"),
        opts,
        "jina-local".to_owned(),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let response = runtime
        .block_on(upstream.send_rerank(
            RerankRequest {
                model: "jina-local".to_owned(),
                query: "what is rust".to_owned(),
                documents: vec!["a card game".to_owned(), "a systems language".to_owned()],
                top_n: None,
                rest: Map::new(),
            },
            &alias,
        ))
        .expect("rerank send should succeed through the child");

    assert_eq!(response.model, "jina-local");
    assert_eq!(response.results.len(), 2);
    assert_eq!(
        response.results[0].pointer("/relevance_score"),
        Some(&serde_json::json!(0.9))
    );
}

#[test]
fn local_upstream_send_honors_cooldown_after_failed_respawn() {
    // UPSTREAM-005: a failed respawn records the attempt time; an immediate
    // second failure is short-circuited by the cooldown (no respawn storm).
    use crate::upstream::LocalUpstream;
    use serde_json::Map;
    use shared_protocol::ProtocolError;
    use shared_protocol::upstream::Upstream;
    use shared_protocol::wire::ChatRequest;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let interrupted = AtomicBool::new(false);

    // First spawn (initial start) succeeds; every later (respawn) spawn fails.
    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            let mut count = counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *count += 1;
            if *count == 1 {
                spawn_fake_child(request)
            } else {
                Err(LocalError::Spawn {
                    executable: PathBuf::from("fake-llama-server"),
                    source: std::io::Error::other("respawn refused"),
                })
            }
        }),
    )
    .expect("initial start should become ready");

    let alias = guard.model_alias().to_owned();
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let make_req = || ChatRequest {
        model: "qwen-local".to_owned(),
        messages: Vec::new(),
        stream: false,
        rest: Map::new(),
    };
    let err1 = runtime
        .block_on(upstream.send(make_req(), &alias))
        .expect_err("failed respawn should surface an error");
    let err2 = runtime
        .block_on(upstream.send(make_req(), &alias))
        .expect_err("cooldown should surface an error");

    // Initial spawn + exactly one failed respawn; the second send is short-
    // circuited by the cooldown and never spawns again.
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        2
    );
    assert!(matches!(err1, ProtocolError::UpstreamTransport(..)));
    // The cooldown error is preserved through the transport wrapper.
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(&err2);
    let mut saw_cooldown = false;
    while let Some(error) = current {
        if error.to_string().contains("cooldown") {
            saw_cooldown = true;
            break;
        }
        current = error.source();
    }
    assert!(saw_cooldown, "expected cooldown in error chain: {err2:?}");
}

#[test]
fn local_upstream_concurrent_sends_respawn_child_at_most_once() {
    // UPSTREAM-005: two concurrent transport failures on a dead child serialize
    // through the guard mutex, so recovery respawns the child exactly once.
    use crate::upstream::LocalUpstream;
    use serde_json::Map;
    use shared_protocol::upstream::Upstream;
    use shared_protocol::wire::ChatRequest;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let interrupted = AtomicBool::new(false);

    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            *counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            spawn_fake_child(request)
        }),
    )
    .expect("initial start should become ready");

    let alias = guard.model_alias().to_owned();
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );
    let make_req = || ChatRequest {
        model: "qwen-local".to_owned(),
        messages: Vec::new(),
        stream: false,
        rest: Map::new(),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let (first, second) = runtime.block_on(async {
        tokio::join!(
            upstream.send(make_req(), &alias),
            upstream.send(make_req(), &alias)
        )
    });

    // Exactly one respawn (initial + one), even under two concurrent failures.
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        2
    );
    assert!(
        first.is_ok() || second.is_ok(),
        "at least one concurrent send should succeed after the single respawn"
    );
}

#[test]
fn recover_if_dead_is_a_noop_for_a_live_but_unreachable_child() {
    // UPSTREAM-005: when a transport failure occurs but the child is still
    // running (live-but-unreachable), recovery is a no-op returning Ok(false) -
    // it never respawns a child that has not actually died.
    use crate::upstream::LocalUpstream;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let interrupted = AtomicBool::new(false);

    // The child is started and stays alive (never killed).
    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            *counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            spawn_fake_child(request)
        }),
    )
    .expect("initial start should become ready");

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );

    // The child is alive, so recovery must not respawn.
    assert!(
        !upstream.test_recover().expect("recover ok"),
        "a live child must not be respawned"
    );
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        1,
        "recovery of a live child must not spawn a replacement"
    );
}

#[test]
fn local_upstream_shutdown_kills_child_and_disables_respawn() {
    // PFGL-MOD-001/PF-GW-SERVER-004: an explicit shutdown terminates the child
    // and prevents any later transport failure from respawning it.
    use crate::upstream::LocalUpstream;
    use serde_json::Map;
    use shared_protocol::upstream::Upstream;
    use shared_protocol::wire::ChatRequest;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let child_id = Arc::new(Mutex::new(None));
    let recorded_id = Arc::clone(&child_id);
    let interrupted = AtomicBool::new(false);

    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            *counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            let child = spawn_fake_child(request)?;
            *recorded_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child.id());
            Ok(child)
        }),
    )
    .expect("initial start should become ready");

    let alias = guard.model_alias().to_owned();
    let pid = child_id
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .expect("child id recorded");

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );

    // Explicit teardown kills the child even though the upstream (an
    // Arc<dyn Upstream> stand-in) is still referenced below.
    upstream.shutdown().expect("teardown should succeed");
    assert!(
        !process_is_alive(pid),
        "shutdown must terminate the llama-server child"
    );

    // A send now fails (child dead) and must NOT respawn: spawn count stays at 1.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let result = runtime.block_on(upstream.send(
        ChatRequest {
            model: "qwen-local".to_owned(),
            messages: Vec::new(),
            stream: false,
            rest: Map::new(),
        },
        &alias,
    ));
    assert!(result.is_err(), "send to a shut-down upstream must fail");
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        1,
        "a shut-down upstream must never respawn its child"
    );
}

const BLOCKED_CHILD_MARKER: &str = "PROMPTFORGE_GATEWAY_TEST_BLOCKED_CHILD";

#[test]
#[ignore = "subprocess worker: stays alive without ever serving readiness"]
fn blocked_child_worker() {
    if std::env::var_os(BLOCKED_CHILD_MARKER).is_none() {
        return;
    }
    // Alive but unreachable: bind an unrelated loopback port and block on a
    // connection that never arrives (no sleep, no busy loop). The guard's
    // readiness probes its own port, never this one, so `accept` blocks until
    // the parent kills us.
    let listener = TcpListener::bind((LOOPBACK, 0)).expect("bind blocked-child socket");
    let _ignored = listener.accept();
}

fn spawn_blocked_child(_request: &SpawnRequest<'_>) -> Result<Child> {
    let executable = std::env::current_exe().map_err(|source| LocalError::Spawn {
        executable: PathBuf::from("<test-executable>"),
        source,
    })?;
    Command::new(&executable)
        .args([
            "--exact",
            "server::tests::blocked_child_worker",
            "--ignored",
            "--nocapture",
        ])
        .env(BLOCKED_CHILD_MARKER, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| LocalError::Spawn { executable, source })
}

#[test]
fn switch_shutdown_terminates_an_in_flight_respawned_child() {
    // PFGL-MOD-001/PF-GW-SERVER-004: a shutdown concurrent with an in-flight
    // recovery/respawn must cancel the respawn and terminate the freshly spawned
    // child, so no old child can outlive a profile switch.
    use crate::upstream::LocalUpstream;
    use serde_json::Map;
    use shared_protocol::upstream::Upstream;
    use shared_protocol::wire::ChatRequest;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let (started_tx, started_rx) = std::sync::mpsc::channel::<u32>();
    let interrupted = AtomicBool::new(false);

    // Spawn #1 becomes ready; the respawn (spawn #2) is a live-but-unreachable
    // child that never serves readiness, so wait_until_ready blocks on cancel.
    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        None,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            let n = {
                let mut count = counted
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *count += 1;
                *count
            };
            if n == 1 {
                spawn_fake_child(request)
            } else {
                let child = spawn_blocked_child(request)?;
                let _ = started_tx.send(child.id());
                Ok(child)
            }
        }),
    )
    .expect("initial start should become ready");

    // Kill the ready child so the first request triggers a recovery/respawn.
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    let upstream = Arc::new(LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    ));

    // Background: a send() whose forward fails (dead child) drives recovery into
    // an in-flight respawn of the blocked child.
    let send_upstream = Arc::clone(&upstream);
    let send_handle = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        runtime.block_on(send_upstream.send(
            ChatRequest {
                model: "qwen-local".to_owned(),
                messages: Vec::new(),
                stream: false,
                rest: Map::new(),
            },
            "qwen-local",
        ))
    });

    // Wait until the respawn has spawned the blocked child.
    let blocked_pid = started_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("respawn should spawn the blocked child");
    assert!(
        process_is_alive(blocked_pid),
        "blocked child should be alive mid-respawn"
    );

    // The switch teardown cancels the in-flight respawn and terminates the child.
    upstream.shutdown().expect("teardown should succeed");

    assert!(
        !process_is_alive(blocked_pid),
        "an in-flight respawned child must not outlive the switch"
    );

    let send_result = send_handle.join().expect("send thread");
    assert!(
        send_result.is_err(),
        "a send whose respawn was cancelled must return an error"
    );
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        2,
        "shutdown must not permit a further respawn"
    );
}

#[cfg(windows)]
#[test]
fn production_command_child_runs_at_below_normal_priority() {
    // The workspace forbids unsafe_code, so a windows-sys GetPriorityClass
    // probe cannot compile in this crate; instead the child reports its own
    // priority class on stdout, which breaks if creation_flags is dropped or
    // carries the wrong value.
    let args = [
        OsString::from("-NoProfile"),
        OsString::from("-Command"),
        OsString::from("(Get-Process -Id $PID).PriorityClass"),
    ];
    let request = SpawnRequest {
        executable: Path::new("powershell.exe"),
        args: &args,
        path_prefix: &[],
        port: 0,
        model_alias: "priority-test",
        api_key: "priority-test",
    };
    let mut child = production_command(&request)
        .expect("build priority probe command")
        .spawn()
        .expect("spawn powershell priority probe");
    let mut stdout = child.stdout.take().expect("child stdout is piped");
    let mut reported = String::new();
    stdout
        .read_to_string(&mut reported)
        .expect("read reported priority class");
    let status = child.wait().expect("wait for priority probe");
    assert!(status.success());
    assert_eq!(reported.trim(), "BelowNormal");
}

fn process_is_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map_or(true, |output| {
                let text = String::from_utf8_lossy(&output.stdout);
                text.contains(&pid.to_string())
            })
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map_or(true, |status| status.success())
    }
}
