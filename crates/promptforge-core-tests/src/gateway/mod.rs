//! Launch `promptforge-gateway` with a generated profile for real-model runs.
//!
//! Core-tests no longer downloads GGUFs or spawns `llama-server`. It writes a
//! temporary profile TOML (model URL + sha256 pin + launch knobs), starts the
//! gateway binary, waits until `/health` and authenticated `/v1/models` show
//! the local model, and kills the gateway process tree on shutdown or drop.
//!
//! Readiness classification lives in [`readiness`] and process-tree teardown in
//! [`process`]; this module owns the guard and its lifecycle orchestration.

mod process;
mod readiness;

use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::net::TcpListener;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use self::readiness::Readiness;

/// Pins copied from `promptforge-gateway::local` (gateway is the source of truth).
const SCENARIO_MODEL_URL: &str =
    "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q8_0.gguf?download=true";
const SCENARIO_MODEL_SHA256: &str =
    "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031";
const SCENARIO_MODEL_NAME: &str = "qwen3-0.6b";

const CAPTURE_LIMIT: usize = 64 * 1024;
/// Cold starts may download a multi-GB GGUF inside the gateway before bind.
const READINESS_DEADLINE: Duration = Duration::from_secs(1_800);
const READINESS_INTERVAL: Duration = Duration::from_millis(200);
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);
/// Fresh port/token/profile attempts when the child exits before readiness and
/// a listener still owns the port (a genuine stolen bind).
const STARTUP_ATTEMPTS: usize = 4;
pub(crate) const LOOPBACK: &str = "127.0.0.1";
const API_KEY_REDACTION: &str = "<per-attempt-secret>";

/// Profile shape written into the temporary TOML the gateway loads.
#[derive(Clone, Copy, Debug)]
pub(crate) enum GatewayProfile {
    /// Fixed small-model scenario knobs (Qwen3-0.6B, CPU-oriented).
    Scenario,
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

#[derive(Debug)]
enum WaitOutcome {
    Ready,
    /// Child died before readiness and a listener owns the port; retry with a
    /// fresh port/token/profile.
    BindCollision(ExitStatus),
}

/// A running `promptforge-gateway` that is killed (process tree) on shutdown or
/// drop. Prefer [`GatewayGuard::shutdown`] for deterministic async teardown;
/// `Drop` is a best-effort fallback for the cancellation path.
#[derive(Debug)]
pub(crate) struct GatewayGuard {
    child: Option<Child>,
    port: u16,
    model_name: String,
    api_key: String,
    _profile_dir: tempfile::TempDir,
    stdout: SharedCapture,
    stderr: SharedCapture,
    readers: Vec<JoinHandle<()>>,
}

impl GatewayGuard {
    /// Writes a temp profile for `profile`, spawns the gateway, and waits until
    /// the local model appears in `/v1/models`.
    ///
    /// Retries up to [`STARTUP_ATTEMPTS`] times with a fresh port, token, and
    /// profile only when the child exits before readiness and a listener still
    /// owns the chosen port (a stolen bind). Any other early exit or a terminal
    /// readiness failure returns an error immediately.
    pub(crate) fn start(profile: GatewayProfile, interrupted: &AtomicBool) -> Result<Self> {
        let model_name = model_name(profile).to_owned();
        let executable = process::gateway_bin()?;
        let mut collisions = Vec::new();

        for attempt in 1..=STARTUP_ATTEMPTS {
            let port = free_port()?;
            let api_key = random_token();
            let profile_dir =
                tempfile::tempdir().context("create temporary gateway profile dir")?;
            let profile_path = profile_dir.path().join("core-tests.toml");
            let toml = render_profile(profile, port, &api_key, &model_name);
            fs::write(&profile_path, toml)
                .with_context(|| format!("write {}", profile_path.display()))?;

            let mut command = Command::new(&executable);
            command
                .arg("serve")
                .arg(&profile_path)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt as _;
                // Own process group so teardown can kill gateway + llama-server.
                command.process_group(0);
            }
            let child = command.spawn().with_context(|| {
                format!("start promptforge-gateway at {}", executable.display())
            })?;

            let stdout = Arc::new(Mutex::new(BoundedCapture::new(CAPTURE_LIMIT)));
            let stderr = Arc::new(Mutex::new(BoundedCapture::new(CAPTURE_LIMIT)));
            let mut guard = Self {
                child: Some(child),
                port,
                model_name: model_name.clone(),
                api_key,
                _profile_dir: profile_dir,
                stdout,
                stderr,
                readers: Vec::with_capacity(2),
            };
            guard.start_capture()?;

            match guard.wait_until_ready(interrupted) {
                Ok(WaitOutcome::Ready) => return Ok(guard),
                Ok(WaitOutcome::BindCollision(status)) => {
                    collisions.push(format!(
                        "attempt {attempt} on port {port}: child exited with {status}\n{}",
                        guard.diagnostics()
                    ));
                }
                Err(error) => {
                    return Err(error).context("promptforge-gateway startup failed");
                }
            }
        }

        bail!(
            "promptforge-gateway exhausted {STARTUP_ATTEMPTS} fresh-port attempts after child bind collisions\n{}",
            collisions.join("\n")
        )
    }

    fn start_capture(&mut self) -> Result<()> {
        let child = self
            .child
            .as_mut()
            .context("gateway child missing before capture start")?;
        let child_stdout = child.stdout.take().context("capture gateway stdout")?;
        let child_stderr = child.stderr.take().context("capture gateway stderr")?;
        self.readers.push(capture_reader(
            "promptforge-gateway-stdout",
            child_stdout,
            Arc::clone(&self.stdout),
        )?);
        self.readers.push(capture_reader(
            "promptforge-gateway-stderr",
            child_stderr,
            Arc::clone(&self.stderr),
        )?);
        Ok(())
    }

    /// Bearer token configured in the generated profile.
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Caller-facing local model name from the generated profile.
    pub(crate) fn model_alias(&self) -> &str {
        &self.model_name
    }

    /// OpenAI-compatible API root for [`promptforge_core::client::GatewayClient`].
    pub(crate) fn base_url(&self) -> String {
        format!("http://{LOOPBACK}:{}/v1", self.port)
    }

    /// Bounded stdout/stderr tails with the bearer token redacted.
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
            "promptforge-gateway stdout (bounded tail):\n{}\npromptforge-gateway stderr (bounded tail):\n{}",
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

    /// Terminates the gateway process tree and joins the reader threads,
    /// performing the blocking teardown off the async worker via
    /// [`tokio::task::spawn_blocking`]. Returns an error if the child cannot be
    /// reaped or its descendants may survive.
    pub(crate) async fn shutdown(&mut self) -> Result<()> {
        let readers = std::mem::take(&mut self.readers);
        let Some(child) = self.child.take() else {
            join_reader_handles(readers);
            return Ok(());
        };
        tokio::task::spawn_blocking(move || {
            let result = process::terminate(child);
            join_reader_handles(readers);
            result
        })
        .await
        .context("join promptforge-gateway shutdown task")?
    }

    fn wait_until_ready(&mut self, interrupted: &AtomicBool) -> Result<WaitOutcome> {
        let deadline = Instant::now() + READINESS_DEADLINE;
        let client = readiness::build_client(HTTP_TIMEOUT)?;

        loop {
            if interrupted.load(Ordering::Acquire) {
                bail!("promptforge-gateway startup interrupted by Ctrl-C");
            }
            if let Some(status) = self.try_wait_child()? {
                self.join_readers();
                // Only a genuine bind collision (a listener still owns the port)
                // is retryable; any other early exit is a real startup defect.
                if process::port_has_listener(self.port) {
                    return Ok(WaitOutcome::BindCollision(status));
                }
                bail!(
                    "promptforge-gateway exited before readiness with {status} and left port {} unowned\n{}",
                    self.port,
                    self.diagnostics()
                );
            }
            match readiness::probe(&client, self.port, &self.api_key, &self.model_name) {
                Readiness::Ready => {
                    if let Some(status) = self.try_wait_child()? {
                        bail!(
                            "promptforge-gateway exited immediately after readiness with {status}"
                        );
                    }
                    return Ok(WaitOutcome::Ready);
                }
                Readiness::Terminal(message) => {
                    bail!(
                        "promptforge-gateway readiness failed terminally: {message}\n{}",
                        self.diagnostics()
                    );
                }
                Readiness::Pending => {}
            }
            if Instant::now() >= deadline {
                bail!(
                    "promptforge-gateway did not expose model `{}` within {} seconds\n{}",
                    self.model_name,
                    READINESS_DEADLINE.as_secs(),
                    self.diagnostics()
                );
            }
            thread::sleep(READINESS_INTERVAL);
        }
    }

    fn try_wait_child(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .as_mut()
            .context("gateway child missing during readiness")?
            .try_wait()
            .context("inspect promptforge-gateway during readiness")
    }

    fn join_readers(&mut self) {
        join_reader_handles(std::mem::take(&mut self.readers));
    }
}

impl Drop for GatewayGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            process::best_effort_terminate(child);
        }
        // Detach the capture readers rather than joining. Once the process tree
        // is killed the pipes reach EOF and the reader threads exit on their
        // own; joining here could block the dropping thread (possibly a runtime
        // worker) unboundedly if a surviving descendant still holds a pipe open.
        // The async `shutdown` path joins them, but inside `spawn_blocking`.
        self.readers.clear();
    }
}

fn join_reader_handles(readers: Vec<JoinHandle<()>>) {
    for reader in readers {
        let _ignored = reader.join();
    }
}

fn model_name(profile: GatewayProfile) -> &'static str {
    match profile {
        GatewayProfile::Scenario => SCENARIO_MODEL_NAME,
    }
}

/// Renders the temporary gateway profile TOML for one harness launch.
pub(crate) fn render_profile(
    profile: GatewayProfile,
    port: u16,
    token: &str,
    model_name: &str,
) -> String {
    let (source, sha256, description, context, n_predict, thinking, gpu_layers, flash, cache_v) =
        match profile {
            GatewayProfile::Scenario => (
                SCENARIO_MODEL_URL,
                SCENARIO_MODEL_SHA256,
                "Tiny Qwen3-0.6B for deterministic core-tests scenarios",
                4096_u32,
                256_u32,
                "never",
                0_u32,
                false,
                "q8_0",
            ),
        };

    format!(
        r#"[server]
bind = "{LOOPBACK}:{port}"
key = "{token}"

[[local_model]]
name = "{model_name}"
description = "{description}"
source = "{source}"
sha256 = "{sha256}"
context = {context}
n_predict = {n_predict}
thinking = "{thinking}"
gpu_layers = {gpu_layers}
flash_attention = {flash}
cache_type_k = "q8_0"
cache_type_v = "{cache_v}"
"#
    )
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind((LOOPBACK, 0)).context("select free gateway port")?;
    listener
        .local_addr()
        .map(|address| address.port())
        .context("read selected gateway port")
}

/// A per-attempt bearer token drawn from the OS-seeded cryptographic `ThreadRng`
/// so a local process cannot predict the credential guarding the loopback
/// gateway during a real-model run.
fn random_token() -> String {
    use rand::RngCore as _;
    let mut rng = rand::rng();
    format!(
        "promptforge-local-{:016x}{:016x}",
        rng.next_u64(),
        rng.next_u64()
    )
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
    use super::*;

    #[test]
    fn scenario_profile_pins_small_cpu_oriented_model() {
        let rendered = render_profile(
            GatewayProfile::Scenario,
            12_345,
            "secret-token",
            SCENARIO_MODEL_NAME,
        );
        assert!(rendered.contains("bind = \"127.0.0.1:12345\""));
        assert!(rendered.contains("key = \"secret-token\""));
        assert!(rendered.contains(&format!("name = \"{SCENARIO_MODEL_NAME}\"")));
        assert!(rendered.contains(SCENARIO_MODEL_URL));
        assert!(rendered.contains(SCENARIO_MODEL_SHA256));
        assert!(rendered.contains("context = 4096"));
        assert!(rendered.contains("n_predict = 256"));
        assert!(rendered.contains("thinking = \"never\""));
        assert!(rendered.contains("gpu_layers = 0"));
        assert!(rendered.contains("flash_attention = false"));
    }

    #[test]
    fn captured_diagnostics_keep_only_the_bounded_tail() {
        let mut capture = BoundedCapture::new(8);
        capture.append(b"abcdef");
        capture.append(b"ghijkl");
        assert_eq!(capture.render(), "[4 earlier bytes omitted]\nefghijkl");
    }

    #[test]
    fn a_random_token_is_long_and_unpredictable() {
        let first = random_token();
        let second = random_token();
        assert!(first.starts_with("promptforge-local-"));
        assert_eq!(first.len(), "promptforge-local-".len() + 32);
        assert_ne!(first, second, "two draws must not collide");
    }
}
