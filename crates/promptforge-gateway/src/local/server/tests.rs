use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

use super::support::BoundedCapture;
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
        let listener = TcpListener::bind((LOOPBACK, 0)).expect("bind unrelated fake listener");
        listener
            .set_nonblocking(true)
            .expect("make unrelated fake listener nonblocking");
        let port = listener
            .local_addr()
            .expect("read unrelated fake listener address")
            .port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let model_alias = model_alias.to_owned();
        let thread = thread::spawn(move || {
            while !thread_shutdown.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => respond(stream, &model_alias, None),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
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
    let executable = std::env::current_exe().map_err(|source| LocalError::Spawn {
        executable: PathBuf::from("<test-executable>"),
        source,
    })?;
    Command::new(&executable)
        .args([
            "--exact",
            "local::server::tests::fake_llama_server_worker",
            "--ignored",
            "--nocapture",
        ])
        .env(TEST_PORT, request.port.to_string())
        .env(TEST_MODEL_ALIAS, request.model_alias)
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
fn launch_args_emit_chat_template_file() {
    let mut opts = options(false);
    opts.chat_template_file = Some(PathBuf::from("mistral-tools.jinja"));
    let args = server_args(Path::new("model.gguf"), 1, "alias", "key", &opts);
    let rendered = display_invocation(Path::new("llama-server"), &args);
    assert!(rendered.contains("--chat-template-file"));
    assert!(rendered.contains("mistral-tools.jinja"));
}

#[test]
fn launch_args_emit_lane_parallel() {
    let mut opts = options(false);
    opts.parallel = 3;
    let args = server_args(Path::new("model.gguf"), 1, "alias", "key", &opts);
    let rendered = display_invocation(Path::new("llama-server"), &args);
    assert!(rendered.contains("--parallel 3"));
    assert!(!rendered.contains("--parallel 1"));
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
    use crate::local::upstream::LocalUpstream;
    use crate::upstream::Upstream;
    use crate::wire::ChatRequest;
    use serde_json::Map;

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
