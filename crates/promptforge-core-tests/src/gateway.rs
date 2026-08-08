//! Launch `promptforge-gateway` with a generated profile for real-model runs.
//!
//! Core-tests no longer downloads GGUFs or spawns `llama-server`. It writes a
//! temporary profile TOML (model URL + sha256 pin + launch knobs), starts the
//! gateway binary, waits until `/health` and authenticated `/v1/models` show
//! the local model, and kills the gateway process tree on drop.

use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

/// Pins copied from `promptforge-gateway::local` (gateway is the source of truth).
const SCENARIO_MODEL_URL: &str =
    "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q8_0.gguf?download=true";
const SCENARIO_MODEL_SHA256: &str =
    "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031";
const SCENARIO_MODEL_NAME: &str = "qwen3-0.6b";

const DEV_MODEL_URL: &str =
    "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf";
const DEV_MODEL_SHA256: &str = "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8";
const DEV_MODEL_NAME: &str = "qwen3.5-9b";

const CAPTURE_LIMIT: usize = 64 * 1024;
/// Cold starts may download a multi-GB GGUF inside the gateway before bind.
const READINESS_DEADLINE: Duration = Duration::from_secs(1_800);
const READINESS_INTERVAL: Duration = Duration::from_millis(200);
const HTTP_TIMEOUT: Duration = Duration::from_secs(2);
/// Fresh port/token/profile attempts when the child exits before readiness
/// (typically a stolen bind after the long LocalRuntime start window).
const STARTUP_ATTEMPTS: usize = 4;
const LOOPBACK: &str = "127.0.0.1";
const API_KEY_REDACTION: &str = "<per-attempt-secret>";

/// Default dev-profile context window, sized for long reasoning chains.
const DEV_DEFAULT_CTX_SIZE: u32 = 131_072;
/// Default dev-profile generation ceiling; reasoning chains are long.
const DEV_DEFAULT_N_PREDICT: u32 = 8192;

/// Selects which pinned local model the generated gateway profile declares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelKind {
    /// Deterministic scenario suite: small Qwen3-0.6B, CPU-oriented knobs.
    Scenario,
    /// Interactive prompt development: large Qwen3.5-9B, GPU-oriented knobs.
    Dev,
}

/// Tunable knobs for the generated dev profile; scenario knobs are fixed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DevServerOptions {
    /// Context window written as `[[local_model]].context`.
    pub(crate) ctx_size: u32,
    /// Generation ceiling written as `[[local_model]].n_predict`.
    pub(crate) n_predict: u32,
    /// When `true`, `thinking = "switchable"`; when `false`, `thinking = "never"`.
    pub(crate) think: bool,
}

impl Default for DevServerOptions {
    fn default() -> Self {
        Self {
            ctx_size: DEV_DEFAULT_CTX_SIZE,
            n_predict: DEV_DEFAULT_N_PREDICT,
            think: true,
        }
    }
}

/// Profile shape written into the temporary TOML the gateway loads.
#[derive(Clone, Copy, Debug)]
pub(crate) enum GatewayProfile {
    /// Fixed small-model scenario knobs.
    Scenario,
    /// Dev knobs from the CLI flags.
    Dev(DevServerOptions),
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
    /// Child died before readiness; retry with a fresh port/token/profile.
    BindCollision(ExitStatus),
}

/// A running `promptforge-gateway` that is killed (process tree) on drop.
#[derive(Debug)]
pub(crate) struct GatewayGuard {
    child: Child,
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
    /// profile when the child exits before readiness (bind stolen after the
    /// long LocalRuntime start window).
    pub(crate) fn start(profile: GatewayProfile, interrupted: &AtomicBool) -> Result<Self> {
        let model_name = model_name(profile).to_owned();
        let executable = gateway_bin()?;
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
                // Own process group so Drop can kill gateway + llama-server children.
                command.process_group(0);
            }
            let child = command.spawn().with_context(|| {
                format!("start promptforge-gateway at {}", executable.display())
            })?;

            let stdout = Arc::new(Mutex::new(BoundedCapture::new(CAPTURE_LIMIT)));
            let stderr = Arc::new(Mutex::new(BoundedCapture::new(CAPTURE_LIMIT)));
            let mut guard = Self {
                child,
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
                    return Err(error).context(format!(
                        "promptforge-gateway startup failed\n{}",
                        guard.diagnostics()
                    ));
                }
            }
        }

        bail!(
            "promptforge-gateway exhausted {STARTUP_ATTEMPTS} fresh-port attempts after child bind collisions\n{}",
            collisions.join("\n")
        )
    }

    fn start_capture(&mut self) -> Result<()> {
        let child_stdout = self
            .child
            .stdout
            .take()
            .context("capture promptforge-gateway stdout")?;
        self.readers.push(capture_reader(
            "promptforge-gateway-stdout",
            child_stdout,
            Arc::clone(&self.stdout),
        )?);
        let child_stderr = self
            .child
            .stderr
            .take()
            .context("capture promptforge-gateway stderr")?;
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

    fn wait_until_ready(&mut self, interrupted: &AtomicBool) -> Result<WaitOutcome> {
        let health = format!("http://{LOOPBACK}:{}/health", self.port);
        let deadline = Instant::now() + READINESS_DEADLINE;
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(HTTP_TIMEOUT)
            .timeout(HTTP_TIMEOUT)
            .build()
            .context("build promptforge-gateway readiness client")?;

        loop {
            if interrupted.load(Ordering::Acquire) {
                bail!("promptforge-gateway startup interrupted by Ctrl-C");
            }
            if let Some(status) = self
                .child
                .try_wait()
                .context("inspect promptforge-gateway during readiness")?
            {
                self.join_readers();
                return Ok(WaitOutcome::BindCollision(status));
            }
            if readiness_belongs_to(&client, self.port, &self.api_key, &self.model_name) {
                if let Some(status) = self
                    .child
                    .try_wait()
                    .context("inspect promptforge-gateway after readiness")?
                {
                    bail!("promptforge-gateway exited immediately after readiness with {status}");
                }
                return Ok(WaitOutcome::Ready);
            }
            if Instant::now() >= deadline {
                bail!(
                    "promptforge-gateway did not expose model `{}` at {health} within {} seconds",
                    self.model_name,
                    READINESS_DEADLINE.as_secs()
                );
            }
            thread::sleep(READINESS_INTERVAL);
        }
    }

    fn join_readers(&mut self) {
        for reader in self.readers.drain(..) {
            let _ignored = reader.join();
        }
    }

    fn kill_tree(&mut self) {
        let pid = self.child.id();
        #[cfg(windows)]
        {
            let _ignored = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(unix)]
        {
            // Negative PID kills the process group started with process_group(0).
            let _ignored = Command::new("kill")
                .args(["-KILL", &format!("-{pid}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ignored = self.child.kill();
        let _ignored = self.child.wait();
    }
}

impl Drop for GatewayGuard {
    fn drop(&mut self) {
        self.kill_tree();
        self.join_readers();
    }
}

/// Resolves the `promptforge-gateway` binary path.
///
/// Order: `PROMPTFORGE_GATEWAY_BIN`, then `target/debug`, then `target/release`
/// relative to the workspace (two levels above this crate's manifest).
pub(crate) fn gateway_bin() -> Result<PathBuf> {
    let override_path = std::env::var_os("PROMPTFORGE_GATEWAY_BIN").map(PathBuf::from);
    let workspace_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    resolve_gateway_bin(override_path.as_deref(), &workspace_target)
}

/// Testable binary resolution used by [`gateway_bin`].
fn resolve_gateway_bin(env_override: Option<&Path>, workspace_target: &Path) -> Result<PathBuf> {
    if let Some(path) = env_override {
        if path.is_file() {
            return Ok(path.to_owned());
        }
        bail!(
            "PROMPTFORGE_GATEWAY_BIN points at missing file {}",
            path.display()
        );
    }

    let name = gateway_executable_name();
    for profile in ["debug", "release"] {
        let candidate = workspace_target.join(profile).join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    bail!(
        "promptforge-gateway binary not found under {}/{{debug,release}}/{name}; \
         build it with `cargo build -p promptforge-gateway` or set PROMPTFORGE_GATEWAY_BIN",
        workspace_target.display()
    )
}

fn gateway_executable_name() -> &'static str {
    if cfg!(windows) {
        "promptforge-gateway.exe"
    } else {
        "promptforge-gateway"
    }
}

fn model_name(profile: GatewayProfile) -> &'static str {
    match profile {
        GatewayProfile::Scenario => SCENARIO_MODEL_NAME,
        GatewayProfile::Dev(_) => DEV_MODEL_NAME,
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
            GatewayProfile::Dev(options) => (
                DEV_MODEL_URL,
                DEV_MODEL_SHA256,
                "Qwen3.5-9B for interactive promptforge prompt development",
                options.ctx_size,
                options.n_predict,
                if options.think { "switchable" } else { "never" },
                99_u32,
                true,
                "q8_0",
            ),
        };

    format!(
        r#"[server]
bind = "{LOOPBACK}:{port}"
token = "{token}"

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

fn random_token() -> String {
    format!(
        "promptforge-local-{:016x}{:016x}",
        fastrand::u64(..),
        fastrand::u64(..)
    )
}

fn readiness_belongs_to(
    client: &reqwest::blocking::Client,
    port: u16,
    api_key: &str,
    model_name: &str,
) -> bool {
    let base = format!("http://{LOOPBACK}:{port}");
    let Ok(health) = client.get(format!("{base}/health")).send() else {
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
                .any(|model| model.get("id").and_then(Value::as_str) == Some(model_name))
        })
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
        assert!(rendered.contains("token = \"secret-token\""));
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
    fn dev_profile_plumbs_cli_knobs_and_gpu_defaults() {
        let rendered = render_profile(
            GatewayProfile::Dev(DevServerOptions {
                ctx_size: 32_768,
                n_predict: 512,
                think: false,
            }),
            9_001,
            "dev-token",
            DEV_MODEL_NAME,
        );
        assert!(rendered.contains("bind = \"127.0.0.1:9001\""));
        assert!(rendered.contains(DEV_MODEL_URL));
        assert!(rendered.contains(DEV_MODEL_SHA256));
        assert!(rendered.contains("context = 32768"));
        assert!(rendered.contains("n_predict = 512"));
        assert!(rendered.contains("thinking = \"never\""));
        assert!(rendered.contains("gpu_layers = 99"));
        assert!(rendered.contains("flash_attention = true"));
        assert!(rendered.contains("cache_type_v = \"q8_0\""));
    }

    #[test]
    fn dev_think_default_is_switchable() {
        let rendered = render_profile(
            GatewayProfile::Dev(DevServerOptions::default()),
            1,
            "t",
            DEV_MODEL_NAME,
        );
        assert!(rendered.contains("thinking = \"switchable\""));
        assert!(rendered.contains("context = 131072"));
        assert!(rendered.contains("n_predict = 8192"));
    }

    #[test]
    fn resolve_gateway_bin_reports_missing_override_and_missing_target() {
        let missing = Path::new("/nonexistent/promptforge-gateway");
        let error = resolve_gateway_bin(Some(missing), Path::new("/no-such-target"))
            .expect_err("missing override must fail");
        assert!(
            format!("{error:#}").contains("PROMPTFORGE_GATEWAY_BIN"),
            "unexpected error: {error:#}"
        );

        let error = resolve_gateway_bin(None, Path::new("/no-such-target"))
            .expect_err("empty target tree must fail");
        assert!(
            format!("{error:#}").contains("promptforge-gateway binary not found"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn captured_diagnostics_keep_only_the_bounded_tail() {
        let mut capture = BoundedCapture::new(8);
        capture.append(b"abcdef");
        capture.append(b"ghijkl");
        assert_eq!(capture.render(), "[4 earlier bytes omitted]\nefghijkl");
    }
}
