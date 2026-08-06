//! Guarded `llama-server` process management for explicit real-model tests.

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

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

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

/// A running local server that is killed and reaped whenever its owner exits.
#[derive(Debug)]
pub(crate) struct ServerGuard {
    child: Child,
    port: u16,
    model_alias: String,
    api_key: String,
    stdout: SharedCapture,
    stderr: SharedCapture,
    readers: Vec<JoinHandle<()>>,
}

impl ServerGuard {
    /// Starts the pinned server and verifies its authenticated model identity.
    pub(crate) fn start(executable: &Path, model: &Path, interrupted: &AtomicBool) -> Result<Self> {
        let mut select_port = free_port;
        let mut make_identity = random_identity;
        let mut spawn = |request: &SpawnRequest<'_>| {
            Command::new(request.executable)
                .args(request.args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| {
                    format!(
                        "start pinned llama-server at {}",
                        request.executable.display()
                    )
                })
        };
        Self::start_with(
            executable,
            model,
            interrupted,
            PRODUCTION_POLICY,
            &mut select_port,
            &mut make_identity,
            &mut spawn,
        )
    }

    fn start_with(
        executable: &Path,
        model: &Path,
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
            let args = server_args(model, port, &identity.model_alias, &identity.api_key);
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
                api_key: identity.api_key,
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
                    return Err(error.context(format!(
                        "llama-server invocation failed\n{}\n{}",
                        display_invocation(executable, &args),
                        guard.diagnostics()
                    )));
                }
            }
        }

        bail!(
            "llama-server exhausted {} fresh-port attempts after child bind collisions\n{}",
            policy.attempts,
            collisions.join("\n")
        )
    }

    fn start_capture(&mut self) -> Result<()> {
        let child_stdout = self
            .child
            .stdout
            .take()
            .context("capture llama-server stdout")?;
        self.readers.push(capture_reader(
            "llama-server-stdout",
            child_stdout,
            Arc::clone(&self.stdout),
        )?);
        let child_stderr = self
            .child
            .stderr
            .take()
            .context("capture llama-server stderr")?;
        self.readers.push(capture_reader(
            "llama-server-stderr",
            child_stderr,
            Arc::clone(&self.stderr),
        )?);
        Ok(())
    }

    /// Returns the bearer token accepted by this server attempt.
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Returns the unique model alias verified during readiness.
    pub(crate) fn model_alias(&self) -> &str {
        &self.model_alias
    }

    /// Returns the OpenAI-compatible API root used by `GatewayClient`.
    pub(crate) fn base_url(&self) -> String {
        format!("http://{LOOPBACK}:{}/v1", self.port)
    }

    /// Returns bounded tail diagnostics from both captured output streams.
    pub(crate) fn diagnostics(&self) -> String {
        let stdout = self
            .stdout
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .render()
            .replace(&self.api_key, API_KEY_REDACTION);
        let stderr = self
            .stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .render()
            .replace(&self.api_key, API_KEY_REDACTION);
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
            .context("build llama-server readiness client")?;

        loop {
            if interrupted.load(Ordering::Acquire) {
                bail!("llama-server startup interrupted by Ctrl-C");
            }
            if let Some(status) = self.child_status()? {
                return self.classify_early_exit(status, policy.http_timeout);
            }
            if readiness_belongs_to(&client, self.port, &self.api_key, &self.model_alias) {
                if let Some(status) = self.child_status()? {
                    return self.classify_early_exit(status, policy.http_timeout);
                }
                return Ok(WaitOutcome::Ready);
            }
            if Instant::now() >= deadline {
                bail!(
                    "llama-server did not expose its authenticated model at {health} within {} seconds",
                    policy.deadline.as_secs()
                );
            }
            thread::sleep(policy.interval);
        }
    }

    fn child_status(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("inspect llama-server during readiness")
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
            bail!("llama-server exited before readiness with {status}")
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
    let listener = TcpListener::bind((LOOPBACK, 0)).context("select free llama-server port")?;
    listener
        .local_addr()
        .map(|address| address.port())
        .context("read selected llama-server port")
}

fn random_identity() -> AttemptIdentity {
    let model_nonce = format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..));
    let key_nonce = format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..));
    AttemptIdentity {
        model_alias: format!("promptforge-pinned-{model_nonce}"),
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

fn server_args(model: &Path, port: u16, model_alias: &str, api_key: &str) -> Vec<OsString> {
    [
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
        OsString::from("4096"),
        OsString::from("--n-predict"),
        OsString::from("256"),
        OsString::from("--parallel"),
        OsString::from("1"),
        OsString::from("--seed"),
        OsString::from("424242"),
        OsString::from("--temp"),
        OsString::from("0"),
        OsString::from("--jinja"),
        OsString::from("--reasoning"),
        OsString::from("off"),
        OsString::from("--reasoning-format"),
        OsString::from("deepseek"),
    ]
    .into()
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
        .with_context(|| format!("start {name} capture thread"))
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;
    use std::sync::atomic::AtomicBool;

    use super::*;

    const TEST_PORT: &str = "PROMPTFORGE_TEST_LLAMA_PORT";
    const TEST_MODEL_ALIAS: &str = "PROMPTFORGE_TEST_LLAMA_MODEL_ALIAS";
    const TEST_API_KEY: &str = "PROMPTFORGE_TEST_LLAMA_API_KEY";
    const TEST_POLICY: StartupPolicy = StartupPolicy {
        attempts: 2,
        deadline: Duration::from_secs(5),
        interval: Duration::from_millis(10),
        http_timeout: Duration::from_millis(100),
    };

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
        let executable = std::env::current_exe().context("locate test executable")?;
        Command::new(executable)
            .args([
                "--exact",
                "server::tests::fake_llama_server_worker",
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
            .context("spawn fake llama-server child")
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
    fn retries_after_foreign_health_listener_wins_selected_port() {
        let foreign = FakeHttpServer::start("Qwen3-0.6B-Q8_0.gguf");
        let fresh_port = free_port().expect("select retry port");
        let mut ports = VecDeque::from([foreign.port, fresh_port]);
        let mut select_port = || ports.pop_front().context("unexpected port selection");
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
    fn fresh_port_retries_are_bounded_when_children_keep_losing_bind() {
        let first = FakeHttpServer::start("foreign-one");
        let second = FakeHttpServer::start("foreign-two");
        let mut ports = VecDeque::from([first.port, second.port]);
        let mut select_port = || ports.pop_front().context("unexpected port selection");
        let mut identity_index = 0;
        let mut make_identity = || {
            let identity = deterministic_identity(identity_index);
            identity_index += 1;
            identity
        };
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let recorded_attempts = Arc::clone(&attempts);
        let mut spawn = move |request: &SpawnRequest<'_>| {
            recorded_attempts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.port);
            spawn_fake_child(request)
        };
        let interrupted = AtomicBool::new(false);

        let error = ServerGuard::start_with(
            Path::new("fake-llama-server"),
            Path::new("pinned-model.gguf"),
            &interrupted,
            TEST_POLICY,
            &mut select_port,
            &mut make_identity,
            &mut spawn,
        )
        .expect_err("two occupied ports must exhaust the test policy");

        assert!(
            format!("{error:#}").contains("exhausted 2 fresh-port attempts"),
            "unexpected startup error: {error:#}"
        );
        assert_eq!(
            *attempts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [first.port, second.port]
        );
    }

    #[test]
    fn invocation_pins_deterministic_jinja_settings() {
        let args = server_args(
            Path::new("model.gguf"),
            12345,
            "per-attempt-model",
            "private-key",
        );
        let rendered = display_invocation(Path::new("llama-server"), &args);
        for expected in [
            "--alias per-attempt-model",
            "--api-key <per-attempt-secret>",
            "--ctx-size 4096",
            "--n-predict 256",
            "--parallel 1",
            "--seed 424242",
            "--temp 0",
            "--jinja",
            "--reasoning off",
            "--reasoning-format deepseek",
        ] {
            assert!(rendered.contains(expected), "missing setting {expected}");
        }
        assert!(!rendered.contains("private-key"));
    }
}
