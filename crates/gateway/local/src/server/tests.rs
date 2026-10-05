//! Tests for llama-server launch arguments, readiness polling, and child process lifecycle.

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

mod launch_args;
mod readiness;
mod upstream_lifecycle;
mod upstream_send;

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

#[cfg(windows)]
#[test]
fn production_command_child_runs_at_below_normal_priority() {
    // The workspace forbids unsafe_code, so a windows-sys GetPriorityClass
    // probe cannot compile in this crate; instead the child reports its own
    // priority class on stdout, which breaks if creation_flags is dropped or
    // holds the wrong value.
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
