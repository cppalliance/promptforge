//! Guarded `llama-server` child process for gateway-owned local inference.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::Secret;
use crate::local::error::LocalError;

type Result<T> = std::result::Result<T, LocalError>;

const CAPTURE_LIMIT: usize = 64 * 1024;
const READINESS_DEADLINE: Duration = Duration::from_secs(180);
const READINESS_INTERVAL: Duration = Duration::from_millis(100);
const HTTP_TIMEOUT: Duration = Duration::from_secs(1);
const STARTUP_ATTEMPTS: usize = 4;
const LOOPBACK: &str = "127.0.0.1";
const API_KEY_REDACTION: &str = "<per-attempt-secret>";

#[derive(Clone, Copy, Debug)]
struct StartupPolicy {
    attempts: usize,
    deadline: Duration,
    interval: Duration,
    http_timeout: Duration,
}

const PRODUCTION_POLICY: StartupPolicy = StartupPolicy {
    attempts: STARTUP_ATTEMPTS,
    deadline: READINESS_DEADLINE,
    interval: READINESS_INTERVAL,
    http_timeout: HTTP_TIMEOUT,
};

#[derive(Debug)]
struct AttemptIdentity {
    model_alias: String,
    api_key: String,
}

#[derive(Debug)]
struct SpawnRequest<'a> {
    executable: &'a Path,
    args: &'a [OsString],
    #[cfg(test)]
    port: u16,
    #[cfg(test)]
    model_alias: &'a str,
    #[cfg(test)]
    api_key: &'a str,
}

#[derive(Debug)]
enum WaitOutcome {
    Ready,
    PortCollision(ExitStatus),
}

#[derive(Debug)]
struct BoundedCapture {
    bytes: VecDeque<u8>,
    dropped: usize,
    limit: usize,
}

impl BoundedCapture {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(limit),
            dropped: 0,
            limit,
        }
    }

    fn append(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes);
        while self.bytes.len() > self.limit {
            self.bytes.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    fn render(&self) -> String {
        let bytes = self.bytes.iter().copied().collect::<Vec<_>>();
        let text = String::from_utf8_lossy(&bytes);
        if self.dropped == 0 {
            text.into_owned()
        } else {
            format!("[{} earlier bytes omitted]\n{text}", self.dropped)
        }
    }
}

type SharedCapture = Arc<Mutex<BoundedCapture>>;

/// Launch knobs for one gateway-owned `llama-server` child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchOptions {
    /// Context window passed as `--ctx-size`.
    pub(crate) ctx_size: u32,
    /// Generation ceiling passed as `--n-predict`.
    pub(crate) n_predict: u32,
    /// GPU layers passed as `-ngl`.
    pub(crate) gpu_layers: u32,
    /// When `true`, pass `--flash-attn on`.
    pub(crate) flash_attention: bool,
    /// KV cache type for K (`--cache-type-k`).
    pub(crate) cache_type_k: String,
    /// KV cache type for V (`--cache-type-v`).
    pub(crate) cache_type_v: String,
    /// When `true`, leave thinking enabled; when `false`, pass `--reasoning off`.
    pub(crate) think: bool,
}

/// A running local server that is killed and reaped whenever its owner exits.
#[derive(Debug)]
pub(crate) struct ServerGuard {
    child: Child,
    port: u16,
    model_alias: String,
    api_key: Secret,
    stdout: SharedCapture,
    stderr: SharedCapture,
    readers: Vec<JoinHandle<()>>,
}

impl ServerGuard {
    /// Starts `llama-server` with `options` and verifies authenticated model identity.
    ///
    /// # Errors
    /// Returns [`LocalError::Server`] when spawn, readiness, or identity checks fail.
    pub(crate) fn start(
        executable: &Path,
        model: &Path,
        options: &LaunchOptions,
        interrupted: &AtomicBool,
    ) -> Result<Self> {
        let mut select_port = free_port;
        let mut make_identity = random_identity;
        let mut spawn = |request: &SpawnRequest<'_>| {
            Command::new(request.executable)
                .args(request.args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|source| {
                    LocalError::Server(format!(
                        "start llama-server at {}: {source}",
                        request.executable.display()
                    ))
                })
        };
        Self::start_with(
            executable,
            model,
            options,
            interrupted,
            PRODUCTION_POLICY,
            &mut select_port,
            &mut make_identity,
            &mut spawn,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the test seam threads three injected fakes beside the launch inputs"
    )]
    fn start_with(
        executable: &Path,
        model: &Path,
        options: &LaunchOptions,
        interrupted: &AtomicBool,
        policy: StartupPolicy,
        select_port: &mut dyn FnMut() -> Result<u16>,
        make_identity: &mut dyn FnMut() -> AttemptIdentity,
        spawn: &mut dyn FnMut(&SpawnRequest<'_>) -> Result<Child>,
    ) -> Result<Self> {
        let mut collisions = Vec::new();
        for attempt in 1..=policy.attempts {
            let port = select_port()?;
            let identity = make_identity();
            let args = server_args(
                model,
                port,
                &identity.model_alias,
                &identity.api_key,
                options,
            );
            let request = SpawnRequest {
                executable,
                args: &args,
                #[cfg(test)]
                port,
                #[cfg(test)]
                model_alias: &identity.model_alias,
                #[cfg(test)]
                api_key: &identity.api_key,
            };
            let child = spawn(&request)?;
            let stdout = Arc::new(Mutex::new(BoundedCapture::new(CAPTURE_LIMIT)));
            let stderr = Arc::new(Mutex::new(BoundedCapture::new(CAPTURE_LIMIT)));
            let mut guard = Self {
                child,
                port,
                model_alias: identity.model_alias,
                api_key: Secret::from(identity.api_key),
                stdout,
                stderr,
                readers: Vec::with_capacity(2),
            };
            guard.start_capture()?;

            match guard.wait_until_ready(interrupted, policy) {
                Ok(WaitOutcome::Ready) => return Ok(guard),
                Ok(WaitOutcome::PortCollision(status)) => {
                    collisions.push(format!(
                        "attempt {attempt} on port {port}: child exited with {status}\n{}\n{}",
                        display_invocation(executable, &args),
                        guard.diagnostics()
                    ));
                }
                Err(error) => {
                    return Err(LocalError::Server(format!(
                        "{error}\nllama-server invocation failed\n{}\n{}",
                        display_invocation(executable, &args),
                        guard.diagnostics()
                    )));
                }
            }
        }

        Err(LocalError::Server(format!(
            "llama-server exhausted {} fresh-port attempts after child bind collisions\n{}",
            policy.attempts,
            collisions.join("\n")
        )))
    }

    fn start_capture(&mut self) -> Result<()> {
        let child_stdout = self
            .child
            .stdout
            .take()
            .ok_or_else(|| LocalError::Server("capture llama-server stdout".to_owned()))?;
        self.readers.push(capture_reader(
            "llama-server-stdout",
            child_stdout,
            Arc::clone(&self.stdout),
        )?);
        let child_stderr = self
            .child
            .stderr
            .take()
            .ok_or_else(|| LocalError::Server("capture llama-server stderr".to_owned()))?;
        self.readers.push(capture_reader(
            "llama-server-stderr",
            child_stderr,
            Arc::clone(&self.stderr),
        )?);
        Ok(())
    }

    /// Returns the port this server is listening on.
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// Returns the bearer token accepted by this server attempt.
    pub(crate) fn api_key(&self) -> &str {
        self.api_key.expose()
    }

    /// Returns the per-attempt upstream model id passed as `--alias`.
    pub(crate) fn model_alias(&self) -> &str {
        &self.model_alias
    }

    /// Returns the OpenAI-compatible API root used by the gateway upstream.
    pub(crate) fn base_url(&self) -> String {
        format!("http://{LOOPBACK}:{}/v1", self.port)
    }

    /// Returns bounded tail diagnostics from both captured output streams.
    pub(crate) fn diagnostics(&self) -> String {
        let api_key = self.api_key.expose();
        let stdout = self
            .stdout
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .render()
            .replace(api_key, API_KEY_REDACTION);
        let stderr = self
            .stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .render()
            .replace(api_key, API_KEY_REDACTION);
        format!(
            "llama-server stdout (bounded tail):\n{}\nllama-server stderr (bounded tail):\n{}",
            if stdout.is_empty() {
                "(empty)"
            } else {
                &stdout
            },
            if stderr.is_empty() {
                "(empty)"
            } else {
                &stderr
            },
        )
    }

    fn wait_until_ready(
        &mut self,
        interrupted: &AtomicBool,
        policy: StartupPolicy,
    ) -> Result<WaitOutcome> {
        let health = format!("http://{LOOPBACK}:{}/health", self.port);
        let deadline = Instant::now() + policy.deadline;
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(policy.http_timeout)
            .timeout(policy.http_timeout)
            .build()
            .map_err(|source| {
                LocalError::Server(format!("build llama-server readiness client: {source}"))
            })?;

        loop {
            if interrupted.load(Ordering::Acquire) {
                return Err(LocalError::Server(
                    "llama-server startup interrupted by Ctrl-C".to_owned(),
                ));
            }
            if let Some(status) = self.child_status()? {
                return self.classify_early_exit(status, policy.http_timeout);
            }
            if readiness_belongs_to(&client, self.port, self.api_key.expose(), &self.model_alias) {
                if let Some(status) = self.child_status()? {
                    return self.classify_early_exit(status, policy.http_timeout);
                }
                return Ok(WaitOutcome::Ready);
            }
            if Instant::now() >= deadline {
                return Err(LocalError::Server(format!(
                    "llama-server did not expose its authenticated model at {health} within {} seconds",
                    policy.deadline.as_secs()
                )));
            }
            thread::sleep(policy.interval);
        }
    }

    fn child_status(&mut self) -> Result<Option<ExitStatus>> {
        self.child.try_wait().map_err(|source| {
            LocalError::Server(format!("inspect llama-server during readiness: {source}"))
        })
    }

    fn classify_early_exit(
        &mut self,
        status: ExitStatus,
        connect_timeout: Duration,
    ) -> Result<WaitOutcome> {
        self.join_readers();
        if listener_is_present(self.port, connect_timeout) {
            Ok(WaitOutcome::PortCollision(status))
        } else {
            Err(LocalError::Server(format!(
                "llama-server exited before readiness with {status}"
            )))
        }
    }

    fn join_readers(&mut self) {
        for reader in self.readers.drain(..) {
            let _ignored = reader.join();
        }
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ignored = self.child.kill();
        let _ignored = self.child.wait();
        self.join_readers();
    }
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind((LOOPBACK, 0))
        .map_err(|source| LocalError::Server(format!("select free llama-server port: {source}")))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|source| LocalError::Server(format!("read selected llama-server port: {source}")))
}

fn random_identity() -> AttemptIdentity {
    let model_nonce = format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..));
    let key_nonce = format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..));
    AttemptIdentity {
        model_alias: format!("promptforge-local-{model_nonce}"),
        api_key: format!("promptforge-local-{key_nonce}"),
    }
}

fn listener_is_present(port: u16, timeout: Duration) -> bool {
    let Ok(address) = format!("{LOOPBACK}:{port}").parse() else {
        return false;
    };
    TcpStream::connect_timeout(&address, timeout).is_ok()
}

fn readiness_belongs_to(
    client: &reqwest::blocking::Client,
    port: u16,
    api_key: &str,
    model_alias: &str,
) -> bool {
    let base = format!("http://{LOOPBACK}:{port}");
    let Ok(health) = client
        .get(format!("{base}/health"))
        .bearer_auth(api_key)
        .send()
    else {
        return false;
    };
    if !health.status().is_success() {
        return false;
    }
    let Ok(models) = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(api_key)
        .send()
    else {
        return false;
    };
    if !models.status().is_success() {
        return false;
    }
    let Ok(body) = models.json::<Value>() else {
        return false;
    };
    body.get("data")
        .and_then(Value::as_array)
        .is_some_and(|models| {
            models
                .iter()
                .any(|model| model.get("id").and_then(Value::as_str) == Some(model_alias))
        })
}

/// Builds the full `llama-server` argument vector for one launch attempt.
fn server_args(
    model: &Path,
    port: u16,
    model_alias: &str,
    api_key: &str,
    options: &LaunchOptions,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--model"),
        model.as_os_str().to_owned(),
        OsString::from("--alias"),
        OsString::from(model_alias),
        OsString::from("--api-key"),
        OsString::from(api_key),
        OsString::from("--host"),
        OsString::from(LOOPBACK),
        OsString::from("--port"),
        OsString::from(port.to_string()),
        OsString::from("--ctx-size"),
        OsString::from(options.ctx_size.to_string()),
        OsString::from("--n-predict"),
        OsString::from(options.n_predict.to_string()),
        OsString::from("--parallel"),
        OsString::from("1"),
        OsString::from("--cache-type-k"),
        OsString::from(&options.cache_type_k),
        OsString::from("--cache-type-v"),
        OsString::from(&options.cache_type_v),
        OsString::from("-ngl"),
        OsString::from(options.gpu_layers.to_string()),
        OsString::from("--jinja"),
    ];
    if options.flash_attention {
        args.extend([OsString::from("--flash-attn"), OsString::from("on")]);
    }
    if !options.think {
        args.extend([OsString::from("--reasoning"), OsString::from("off")]);
    }
    args.extend([OsString::from("--reasoning-format"), OsString::from("auto")]);
    let (temp, top_p) = if options.think {
        ("1.0", "0.95")
    } else {
        ("0.7", "0.8")
    };
    args.extend([
        OsString::from("--temp"),
        OsString::from(temp),
        OsString::from("--top-p"),
        OsString::from(top_p),
        OsString::from("--top-k"),
        OsString::from("20"),
        OsString::from("--presence-penalty"),
        OsString::from("1.5"),
    ]);
    args
}

fn display_invocation(executable: &Path, args: &[OsString]) -> String {
    let mut pieces = Vec::with_capacity(args.len() + 1);
    pieces.push(executable.display().to_string());
    let mut redact_next = false;
    for argument in args {
        if redact_next {
            pieces.push(API_KEY_REDACTION.to_owned());
            redact_next = false;
        } else {
            let rendered = argument.to_string_lossy().into_owned();
            redact_next = rendered == "--api-key";
            pieces.push(rendered);
        }
    }
    pieces.join(" ")
}

fn capture_reader<R>(
    name: &'static str,
    mut reader: R,
    capture: SharedCapture,
) -> Result<JoinHandle<()>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => capture
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .append(&buffer[..count]),
                }
            }
        })
        .map_err(|source| LocalError::Server(format!("start {name} capture thread: {source}")))
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;

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
            gpu_layers: 99,
            flash_attention: true,
            cache_type_k: "q8_0".to_owned(),
            cache_type_v: "q4_0".to_owned(),
            think,
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
        let executable = std::env::current_exe()
            .map_err(|source| LocalError::Server(format!("locate test executable: {source}")))?;
        Command::new(executable)
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
            .map_err(|source| {
                LocalError::Server(format!("spawn fake llama-server child: {source}"))
            })
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
            ports
                .pop_front()
                .ok_or_else(|| LocalError::Server("unexpected port selection".to_owned()))
        };
        let mut make_identity = || deterministic_identity(0);
        let mut spawn = |request: &SpawnRequest<'_>| spawn_fake_child(request);
        let interrupted = AtomicBool::new(false);
        let guard = ServerGuard::start_with(
            Path::new("fake-llama-server"),
            Path::new("pinned-model.gguf"),
            &options(false),
            &interrupted,
            TEST_POLICY,
            &mut select_port,
            &mut make_identity,
            &mut spawn,
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
            ports
                .pop_front()
                .ok_or_else(|| LocalError::Server("unexpected port selection".to_owned()))
        };
        let mut identity_index = 0;
        let mut make_identity = || {
            let identity = deterministic_identity(identity_index);
            identity_index += 1;
            identity
        };
        let attempted_ports = Arc::new(Mutex::new(Vec::new()));
        let recorded_ports = Arc::clone(&attempted_ports);
        let mut spawn = move |request: &SpawnRequest<'_>| {
            recorded_ports
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.port);
            spawn_fake_child(request)
        };
        let interrupted = AtomicBool::new(false);

        let guard = ServerGuard::start_with(
            Path::new("fake-llama-server"),
            Path::new("pinned-model.gguf"),
            &options(false),
            &interrupted,
            TEST_POLICY,
            &mut select_port,
            &mut make_identity,
            &mut spawn,
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
            ports
                .pop_front()
                .ok_or_else(|| LocalError::Server("unexpected port selection".to_owned()))
        };
        let mut make_identity = || deterministic_identity(0);
        let child_id = Arc::new(Mutex::new(None));
        let recorded_id = Arc::clone(&child_id);
        let mut spawn = move |request: &SpawnRequest<'_>| {
            let child = spawn_fake_child(request)?;
            *recorded_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child.id());
            Ok(child)
        };
        let interrupted = AtomicBool::new(false);

        let guard = ServerGuard::start_with(
            Path::new("fake-llama-server"),
            Path::new("pinned-model.gguf"),
            &options(false),
            &interrupted,
            TEST_POLICY,
            &mut select_port,
            &mut make_identity,
            &mut spawn,
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
}
