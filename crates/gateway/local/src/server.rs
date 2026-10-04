//! Guarded `llama-server` child process for gateway-owned local inference.

mod lifecycle;
mod support;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::LocalError;
use gateway_config::Secret;
use support::{
    ChildSpawner, SharedCapture, capture_reader, display_invocation, free_port,
    listener_is_present, new_capture, random_identity, readiness_belongs_to, server_args,
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

struct AttemptIdentity {
    model_alias: String,
    api_key: String,
}

impl std::fmt::Debug for AttemptIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the per-attempt bearer token (HYGIENE-SECRET-DEBUG-001).
        f.debug_struct("AttemptIdentity")
            .field("model_alias", &self.model_alias)
            .field("api_key", &API_KEY_REDACTION)
            .finish()
    }
}

struct SpawnRequest<'a> {
    executable: &'a Path,
    args: &'a [OsString],
    /// Directories prepended to the child's `PATH`; empty leaves the child on
    /// the inherited `PATH`.
    path_prefix: &'a [PathBuf],
    #[cfg(test)]
    port: u16,
    #[cfg(test)]
    model_alias: &'a str,
    #[cfg(test)]
    api_key: &'a str,
}

/// Renders an argument vector with the value following `--api-key` redacted.
struct RedactedArgs<'a>(&'a [OsString]);

impl std::fmt::Debug for RedactedArgs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut list = f.debug_list();
        let mut redact_next = false;
        for argument in self.0 {
            if redact_next {
                list.entry(&API_KEY_REDACTION);
                redact_next = false;
            } else {
                redact_next = argument.to_string_lossy() == "--api-key";
                list.entry(argument);
            }
        }
        list.finish()
    }
}

impl std::fmt::Debug for SpawnRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact the credential in both `args` (the `--api-key <token>` pair) and
        // the test-only `api_key` field (HYGIENE-SECRET-DEBUG-002).
        let mut dbg = f.debug_struct("SpawnRequest");
        dbg.field("executable", &self.executable);
        dbg.field("args", &RedactedArgs(self.args));
        dbg.field("path_prefix", &self.path_prefix);
        #[cfg(test)]
        {
            dbg.field("port", &self.port);
            dbg.field("model_alias", &self.model_alias);
            dbg.field("api_key", &API_KEY_REDACTION);
        }
        dbg.finish()
    }
}

#[derive(Debug)]
enum WaitOutcome {
    Ready,
    PortCollision(ExitStatus),
}

/// The serving mode a child is launched into, derived from the model kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServeMode {
    /// Chat completions; no extra flag.
    Chat,
    /// Passes `--embeddings` so the child serves embedding requests.
    Embeddings,
    /// Passes `--reranking` so the child serves rerank requests.
    Reranking,
}

/// Launch state for a speculative decoding drafter companion.
///
/// The resolved drafter path is owned here so a respawn re-emits the exact
/// verified artifact without re-resolving external state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SpeculativeLaunch {
    /// Resolved drafter GGUF path, passed as `--spec-draft-model`.
    pub(crate) draft_model: PathBuf,
    /// Maximum tokens drafted per step, passed as `--spec-draft-n-max`.
    pub(crate) draft_max: u32,
}

/// Launch knobs for one gateway-owned `llama-server` child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchOptions {
    /// Context window passed as `--ctx-size`.
    pub(crate) ctx_size: u32,
    /// Generation ceiling passed as `--n-predict`.
    pub(crate) n_predict: u32,
    /// Concurrent slots passed as `--parallel` (the model's admit limit).
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
    /// The child's serving mode (`--embeddings` / `--reranking`; chat is default).
    pub(crate) serve_mode: ServeMode,
    /// The resolved MTP drafter companion, when the model declares one.
    pub(crate) speculative: Option<SpeculativeLaunch>,
    /// The resolved multimodal projector GGUF path (`--mmproj`), when the
    /// model declares one.
    pub(crate) multimodal_projector: Option<PathBuf>,
    /// Directories prepended to the child's `PATH` (the staged CUDA bundle
    /// directory and the CUDA Toolkit runtime directory); empty for
    /// archive-installed servers.
    pub(crate) path_prefix: Vec<PathBuf>,
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
    readers: Vec<(&'static str, JoinHandle<std::io::Result<()>>)>,
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
                path_prefix: &options.path_prefix,
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
        self.readers.push((
            "llama-server-stdout",
            capture_reader(
                "llama-server-stdout",
                child_stdout,
                Arc::clone(&self.stdout),
            )?,
        ));
        let child_stderr = self
            .child
            .stderr
            .take()
            .ok_or(LocalError::Capture { stream: "stderr" })?;
        self.readers.push((
            "llama-server-stderr",
            capture_reader(
                "llama-server-stderr",
                child_stderr,
                Arc::clone(&self.stderr),
            )?,
        ));
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
            .map_err(|source| LocalError::ReadinessClient {
                source: source.into(),
            })?;

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

    fn classify_early_exit(
        &mut self,
        status: ExitStatus,
        connect_timeout: Duration,
    ) -> Result<WaitOutcome> {
        self.join_readers_checked()?;
        if listener_is_present(self.port, connect_timeout) {
            Ok(WaitOutcome::PortCollision(status))
        } else {
            Err(LocalError::EarlyExit {
                status: status.to_string(),
            })
        }
    }
}
