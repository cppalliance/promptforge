//! Guarded `llama-server` child process for gateway-owned local inference.

mod support;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::Secret;
use crate::local::error::LocalError;
use support::{
    SharedCapture, capture_reader, display_invocation, free_port, listener_is_present, new_capture,
    random_identity, readiness_belongs_to, server_args,
};

type Result<T> = std::result::Result<T, LocalError>;

const CAPTURE_LIMIT: usize = 64 * 1024;
const READINESS_DEADLINE: Duration = Duration::from_secs(180);
const READINESS_INTERVAL: Duration = Duration::from_millis(100);
const HTTP_TIMEOUT: Duration = Duration::from_secs(1);
const STARTUP_ATTEMPTS: usize = 4;
const LOOPBACK: &str = "127.0.0.1";
const API_KEY_REDACTION: &str = "<per-attempt-secret>";
/// Upper bound on how long an explicit or drop-time teardown waits for a killed
/// child to be reaped before giving up. Keeps teardown bounded, never unbounded.
const TEARDOWN_DEADLINE: Duration = Duration::from_secs(5);
/// Poll interval while reaping a killed child during bounded teardown.
const TEARDOWN_POLL: Duration = Duration::from_millis(10);

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

type SpawnFn = Box<dyn FnMut(&SpawnRequest<'_>) -> Result<Child> + Send>;

/// Shared spawn callback used for the first start and later same-port respawns.
#[derive(Clone)]
struct ChildSpawner {
    inner: Arc<Mutex<SpawnFn>>,
}

impl ChildSpawner {
    fn new(spawn: impl FnMut(&SpawnRequest<'_>) -> Result<Child> + Send + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(spawn))),
        }
    }

    fn production() -> Self {
        Self::new(|request: &SpawnRequest<'_>| {
            Command::new(request.executable)
                .args(request.args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|source| LocalError::Spawn {
                    executable: request.executable.to_owned(),
                    source,
                })
        })
    }

    fn spawn(&self, request: &SpawnRequest<'_>) -> Result<Child> {
        (self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))(request)
    }
}

impl std::fmt::Debug for ChildSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChildSpawner")
    }
}

/// Launch knobs for one gateway-owned `llama-server` child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchOptions {
    /// Context window passed as `--ctx-size`.
    pub(crate) ctx_size: u32,
    /// Generation ceiling passed as `--n-predict`.
    pub(crate) n_predict: u32,
    /// Concurrent slots passed as `--parallel` (lane admit limit).
    pub(crate) parallel: u32,
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
    /// Optional Jinja override passed as `--chat-template-file`.
    pub(crate) chat_template_file: Option<PathBuf>,
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
    spawner: ChildSpawner,
    policy: StartupPolicy,
}

impl ServerGuard {
    /// Starts `llama-server` with `options` and verifies authenticated model identity.
    ///
    /// # Errors
    /// Returns a [`LocalError`] when spawn, readiness, or identity checks fail.
    pub(crate) fn start(
        executable: &Path,
        model: &Path,
        options: &LaunchOptions,
        interrupted: &AtomicBool,
    ) -> Result<Self> {
        let mut select_port = free_port;
        let mut make_identity = random_identity;
        Self::start_with(
            executable,
            model,
            options,
            interrupted,
            PRODUCTION_POLICY,
            &mut select_port,
            &mut make_identity,
            &ChildSpawner::production(),
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
        spawner: &ChildSpawner,
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
            let child = spawner.spawn(&request)?;
            let stdout = new_capture();
            let stderr = new_capture();
            let mut guard = Self {
                child,
                port,
                model_alias: identity.model_alias,
                api_key: Secret::new(identity.api_key),
                stdout,
                stderr,
                readers: Vec::with_capacity(2),
                spawner: spawner.clone(),
                policy,
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
                    return Err(LocalError::Startup {
                        detail: format!(
                            "{}\n{}",
                            display_invocation(executable, &args),
                            guard.diagnostics()
                        ),
                        source: Box::new(error),
                    });
                }
            }
        }

        Err(LocalError::PortCollisions {
            attempts: policy.attempts,
            detail: collisions.join("\n"),
        })
    }

    fn start_capture(&mut self) -> Result<()> {
        let child_stdout = self
            .child
            .stdout
            .take()
            .ok_or(LocalError::Capture { stream: "stdout" })?;
        self.readers.push(capture_reader(
            "llama-server-stdout",
            child_stdout,
            Arc::clone(&self.stdout),
        )?);
        let child_stderr = self
            .child
            .stderr
            .take()
            .ok_or(LocalError::Capture { stream: "stderr" })?;
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
        let deadline = Instant::now() + policy.deadline;
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(policy.http_timeout)
            .timeout(policy.http_timeout)
            .build()
            .map_err(|source| LocalError::ReadinessClient { source })?;

        loop {
            if interrupted.load(Ordering::Acquire) {
                return Err(LocalError::StartupInterrupted);
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
                return Err(LocalError::ReadinessTimeout {
                    seconds: policy.deadline.as_secs(),
                });
            }
            thread::sleep(policy.interval);
        }
    }

    fn child_status(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .map_err(|source| LocalError::Inspect { source })
    }

    /// Returns whether the child process is still running.
    ///
    /// When the child has already exited, joins capture threads so a later
    /// [`Self::respawn`] can attach fresh readers.
    pub(crate) fn is_running(&mut self) -> Result<bool> {
        if self.child_status()?.is_none() {
            Ok(true)
        } else {
            self.join_readers();
            Ok(false)
        }
    }

    /// Kills the current child (if any) and starts a new one on the same port,
    /// alias, and API key, then waits until authenticated readiness succeeds.
    ///
    /// # Errors
    /// Returns a [`LocalError`] when kill, spawn, or readiness fails.
    pub(crate) fn respawn(
        &mut self,
        executable: &Path,
        model: &Path,
        options: &LaunchOptions,
    ) -> Result<()> {
        self.terminate_child()?;
        self.join_readers();

        let args = server_args(
            model,
            self.port,
            &self.model_alias,
            self.api_key.expose(),
            options,
        );
        let request = SpawnRequest {
            executable,
            args: &args,
            #[cfg(test)]
            port: self.port,
            #[cfg(test)]
            model_alias: &self.model_alias,
            #[cfg(test)]
            api_key: self.api_key.expose(),
        };
        let child = self.spawner.spawn(&request)?;
        self.child = child;
        self.stdout = new_capture();
        self.stderr = new_capture();
        self.readers = Vec::with_capacity(2);
        self.start_capture()?;

        let interrupted = AtomicBool::new(false);
        let policy = self.policy;
        match self.wait_until_ready(&interrupted, policy)? {
            WaitOutcome::Ready => Ok(()),
            WaitOutcome::PortCollision(status) => Err(LocalError::RespawnPortCollision {
                port: self.port,
                detail: format!(
                    "child exited with {status}\n{}\n{}",
                    display_invocation(executable, &args),
                    self.diagnostics()
                ),
            }),
        }
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
            Err(LocalError::EarlyExit {
                status: status.to_string(),
            })
        }
    }

    fn join_readers(&mut self) {
        for reader in self.readers.drain(..) {
            let _ignored = reader.join();
        }
    }

    /// Best-effort bounded termination of the current child.
    ///
    /// Checks `try_wait` first so an already-exited child is never re-signalled,
    /// then kills and reaps within [`TEARDOWN_DEADLINE`] so teardown can never
    /// block unbounded. Kill and reap-timeout failures are surfaced to callers.
    fn terminate_child(&mut self) -> Result<()> {
        if self.child_status()?.is_some() {
            return Ok(());
        }
        self.child
            .kill()
            .map_err(|source| LocalError::Kill { source })?;
        let deadline = Instant::now() + TEARDOWN_DEADLINE;
        loop {
            if self.child_status()?.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(LocalError::TeardownTimeout);
            }
            thread::sleep(TEARDOWN_POLL);
        }
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        // Best-effort, bounded teardown. `terminate_child` caps its reap at
        // `TEARDOWN_DEADLINE` (polling `try_wait`), so drop never waits unbounded
        // even if the child ignores the kill signal. The error-surfacing path is
        // `respawn`, which calls `terminate_child` with `?`; here in `Drop` the
        // result is intentionally discarded because there is no caller to report
        // to (SERVER-001: bounded Drop; async offload is rejected, see below).
        let _ignored = self.terminate_child();
        self.join_readers();
    }
}
