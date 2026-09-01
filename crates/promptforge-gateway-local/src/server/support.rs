//! Pure helpers for the `llama-server` guard: output capture, port selection,
//! per-attempt identity, readiness probing, and argument rendering.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Deserialize;

use super::{
    API_KEY_REDACTION, AttemptIdentity, CAPTURE_LIMIT, LOOPBACK, LaunchOptions, Result, ServeMode,
    SpawnRequest,
};
use crate::error::LocalError;
use promptforge_gateway_protocol::http_util::MAX_JSON_BODY;

/// A spawn callback: builds a child from a [`SpawnRequest`].
pub(super) type SpawnFn = Box<dyn FnMut(&SpawnRequest<'_>) -> Result<Child> + Send>;

/// Shared spawn callback used for the first start and later same-port respawns.
#[derive(Clone)]
pub(super) struct ChildSpawner {
    inner: Arc<Mutex<SpawnFn>>,
}

impl ChildSpawner {
    pub(super) fn new(
        spawn: impl FnMut(&SpawnRequest<'_>) -> Result<Child> + Send + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(spawn))),
        }
    }

    pub(super) fn production() -> Self {
        Self::new(|request: &SpawnRequest<'_>| {
            production_command(request)?
                .spawn()
                .map_err(|source| LocalError::Spawn {
                    executable: request.executable.to_owned(),
                    source,
                })
        })
    }

    pub(super) fn spawn(&self, request: &SpawnRequest<'_>) -> Result<Child> {
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

/// Win32 `BELOW_NORMAL_PRIORITY_CLASS`. The raw value is a stable ABI
/// constant, so naming it here avoids a `windows-sys` dependency in the main
/// build.
#[cfg(windows)]
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// Builds the production `Command` for one `llama-server` launch attempt.
///
/// On Windows the child is created at `BELOW_NORMAL_PRIORITY_CLASS` so weight
/// loading and inference yield CPU and I/O scheduling to interactive desktop
/// processes. Non-Windows is a documented no-op: a `nice` port would need
/// libc or `pre_exec` unsafe and is deferred.
///
/// When the request carries a `path_prefix` (a staged CUDA bundle), the
/// child's `PATH` is set to the prefix entries followed by the inherited
/// ones. Only the child environment is touched; this process's environment is
/// never mutated.
///
/// # Errors
/// Returns [`LocalError::Spawn`] when the prefixed `PATH` value cannot be
/// joined (a prefix entry contains a platform-forbidden character).
pub(super) fn production_command(request: &SpawnRequest<'_>) -> Result<Command> {
    let mut command = Command::new(request.executable);
    command
        .args(request.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !request.path_prefix.is_empty() {
        let path = child_path_with_prefix(request.path_prefix, std::env::var_os("PATH")).map_err(
            |source| LocalError::Spawn {
                executable: request.executable.to_owned(),
                source: io::Error::other(source),
            },
        )?;
        command.env("PATH", path);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(BELOW_NORMAL_PRIORITY_CLASS | crate::CREATE_NO_WINDOW);
    }
    Ok(command)
}

/// The child's `PATH` value: `prefix` entries first, then the inherited ones.
///
/// Pure join, so the prepend order and the no-process-mutation contract are
/// testable without spawning anything.
fn child_path_with_prefix(
    prefix: &[PathBuf],
    inherited: Option<OsString>,
) -> std::result::Result<OsString, std::env::JoinPathsError> {
    let mut entries = prefix.to_vec();
    if let Some(inherited) = inherited {
        entries.extend(std::env::split_paths(&inherited));
    }
    std::env::join_paths(entries)
}

/// A narrow view of the `llama-server` `/v1/models` readiness response.
///
/// Deserializing into this instead of a free-form `serde_json::Value` keeps the
/// readiness check from allocating an arbitrary document (SERVER-003); it is fed
/// bytes that were already capped by [`read_blocking_capped`].
#[derive(Deserialize)]
struct ReadinessModels {
    #[serde(default)]
    data: Vec<ReadinessModel>,
}

#[derive(Deserialize)]
struct ReadinessModel {
    id: String,
}

/// Reads at most `cap` bytes from a blocking response body.
///
/// `reqwest::blocking::Response` is a [`Read`], so `take(cap)` bounds the read; a
/// stalled or oversized readiness body can never force an unbounded allocation.
fn read_blocking_capped(response: reqwest::blocking::Response, cap: usize) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = response.take(cap as u64).read_to_end(&mut buffer);
    buffer
}

/// Whether the capped readiness body lists `model_alias` under `data[].id`.
///
/// Pure and total: a body that fails to parse (including one truncated at the
/// byte cap) yields `false`, so readiness is simply not yet confirmed.
fn readiness_lists_model(body: &[u8], model_alias: &str) -> bool {
    serde_json::from_slice::<ReadinessModels>(body)
        .is_ok_and(|parsed| parsed.data.iter().any(|model| model.id == model_alias))
}

#[derive(Debug)]
pub(super) struct BoundedCapture {
    bytes: VecDeque<u8>,
    dropped: usize,
    limit: usize,
}

impl BoundedCapture {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::with_capacity(limit),
            dropped: 0,
            limit,
        }
    }

    pub(super) fn append(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes);
        while self.bytes.len() > self.limit {
            self.bytes.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub(super) fn render(&self) -> String {
        let bytes = self.bytes.iter().copied().collect::<Vec<_>>();
        let text = String::from_utf8_lossy(&bytes);
        if self.dropped == 0 {
            text.into_owned()
        } else {
            format!("[{} earlier bytes omitted]\n{text}", self.dropped)
        }
    }
}

pub(super) type SharedCapture = Arc<Mutex<BoundedCapture>>;

pub(super) fn new_capture() -> SharedCapture {
    Arc::new(Mutex::new(BoundedCapture::new(CAPTURE_LIMIT)))
}

pub(super) fn free_port() -> Result<u16> {
    let listener = TcpListener::bind((LOOPBACK, 0)).map_err(|source| LocalError::Port {
        operation: "select free llama-server port",
        source,
    })?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|source| LocalError::Port {
            operation: "read selected llama-server port",
            source,
        })
}

pub(super) fn random_identity() -> AttemptIdentity {
    // The loopback bearer token guards the local llama-server; use the OS-seeded
    // cryptographic RNG (`rand::rng`, a ChaCha-based CSPRNG) rather than a fast
    // non-cryptographic generator.
    use rand::Rng;
    let mut rng = rand::rng();
    let model_nonce = format!("{:016x}{:016x}", rng.random::<u64>(), rng.random::<u64>());
    let key_nonce = format!("{:016x}{:016x}", rng.random::<u64>(), rng.random::<u64>());
    AttemptIdentity {
        model_alias: format!("promptforge-local-{model_nonce}"),
        api_key: format!("promptforge-local-{key_nonce}"),
    }
}

pub(super) fn listener_is_present(port: u16, timeout: Duration) -> bool {
    let Ok(address) = format!("{LOOPBACK}:{port}").parse() else {
        return false;
    };
    TcpStream::connect_timeout(&address, timeout).is_ok()
}

pub(super) fn readiness_belongs_to(
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
    let body = read_blocking_capped(models, MAX_JSON_BODY);
    readiness_lists_model(&body, model_alias)
}

/// Builds the full `llama-server` argument vector for one launch attempt.
pub(super) fn server_args(
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
        OsString::from(options.parallel.to_string()),
        OsString::from("--cache-type-k"),
        OsString::from(&options.cache_type_k),
        OsString::from("--cache-type-v"),
        OsString::from(&options.cache_type_v),
        OsString::from("-ngl"),
        OsString::from(options.gpu_layers.to_string()),
        OsString::from("--jinja"),
        // The pinned server (third_party/llama.cpp @ fb0e6b6, common/log.cpp)
        // maps llama/ggml INFO messages - device reports and `load_tensors`
        // offload lines - to its trace verbosity, so the default threshold
        // hides exactly the evidence the captured diagnostics exist for.
        OsString::from("-lv"),
        OsString::from("4"),
    ];
    // Companion spellings are pinned to the bundled server
    // (third_party/llama.cpp @ fb0e6b6, common/arg.cpp): `--spec-draft-model`
    // (alias of `-md`), `--spec-type draft-mtp` (the only speculation type the
    // configuration can express), `--spec-draft-n-max`, and `--mmproj`. The
    // legacy `--draft`/`--draft-max` flags were removed at that pin.
    if let Some(speculative) = &options.speculative {
        args.extend([
            OsString::from("--spec-draft-model"),
            speculative.draft_model.as_os_str().to_owned(),
            OsString::from("--spec-type"),
            OsString::from("draft-mtp"),
            OsString::from("--spec-draft-n-max"),
            OsString::from(speculative.draft_max.to_string()),
        ]);
    }
    if let Some(projector) = &options.multimodal_projector {
        args.extend([OsString::from("--mmproj"), projector.as_os_str().to_owned()]);
    }
    match options.serve_mode {
        ServeMode::Chat => {}
        ServeMode::Embeddings => args.push(OsString::from("--embeddings")),
        ServeMode::Reranking => args.push(OsString::from("--reranking")),
    }
    if let Some(template) = &options.chat_template_file {
        args.extend([
            OsString::from("--chat-template-file"),
            template.as_os_str().to_owned(),
        ]);
    }
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

pub(super) fn display_invocation(executable: &Path, args: &[OsString]) -> String {
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

pub(super) fn capture_reader<R>(
    name: &'static str,
    mut reader: R,
    capture: SharedCapture,
) -> Result<JoinHandle<io::Result<()>>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || -> io::Result<()> {
            // Surface a genuine capture read failure (SERVER-005) instead of
            // silently ending the loop; EOF (`Ok(0)`) is normal completion.
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => return Ok(()),
                    Ok(count) => capture
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .append(&buffer[..count]),
                    Err(source) => return Err(source),
                }
            }
        })
        .map_err(|source| LocalError::CaptureThread {
            stream: name,
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::{
        capture_reader, child_path_with_prefix, new_capture, production_command,
        readiness_lists_model,
    };
    use crate::server::SpawnRequest;
    use std::ffi::{OsStr, OsString};
    use std::io::{self, Read};
    use std::path::{Path, PathBuf};

    struct ErroringReader;

    impl Read for ErroringReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("capture stream boom"))
        }
    }

    struct EofReader;

    impl Read for EofReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    #[test]
    fn capture_reader_surfaces_read_errors_on_join() {
        // SERVER-005: a genuine capture read failure is returned on join, not
        // silently swallowed by ending the loop.
        let handle = capture_reader("test-stream", ErroringReader, new_capture()).expect("spawn");
        assert!(handle.join().expect("thread joined").is_err());
    }

    #[test]
    fn capture_reader_reports_clean_eof_as_ok() {
        // Normal completion (child pipe closed) is EOF, an `Ok(())` result.
        let handle = capture_reader("test-stream", EofReader, new_capture()).expect("spawn");
        assert!(handle.join().expect("thread joined").is_ok());
    }

    #[test]
    fn readiness_lists_model_matches_alias_and_tolerates_junk() {
        let body = br#"{"object":"list","data":[{"id":"promptforge-local-abc"}]}"#;
        assert!(readiness_lists_model(body, "promptforge-local-abc"));
        assert!(!readiness_lists_model(body, "some-other-alias"));
        // Missing `data`, empty body, and truncated JSON all read as not-ready.
        assert!(!readiness_lists_model(br#"{"object":"list"}"#, "x"));
        assert!(!readiness_lists_model(b"", "x"));
        assert!(!readiness_lists_model(
            br#"{"data":[{"id":"promptforge"#,
            "x"
        ));
    }

    #[test]
    fn child_path_with_prefix_orders_prefix_before_inherited() {
        let prefix = vec![PathBuf::from("staged"), PathBuf::from("toolkit-bin")];
        let inherited =
            std::env::join_paths([PathBuf::from("c"), PathBuf::from("d")]).expect("join inherited");
        let joined = child_path_with_prefix(&prefix, Some(inherited)).expect("join child path");
        let entries: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(
            entries,
            vec![
                PathBuf::from("staged"),
                PathBuf::from("toolkit-bin"),
                PathBuf::from("c"),
                PathBuf::from("d"),
            ]
        );
    }

    #[test]
    fn child_path_with_prefix_without_inherited_is_just_the_prefix() {
        let prefix = vec![PathBuf::from("staged")];
        let joined = child_path_with_prefix(&prefix, None).expect("join child path");
        let entries: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(entries, vec![PathBuf::from("staged")]);
    }

    #[test]
    fn production_command_prepends_path_to_child_env_only() {
        let before = std::env::var_os("PATH");
        let args = [OsString::from("--version")];
        let prefix = [PathBuf::from("staged-dir"), PathBuf::from("toolkit-bin")];
        let request = SpawnRequest {
            executable: Path::new("llama-server"),
            args: &args,
            path_prefix: &prefix,
            port: 0,
            model_alias: "env-test",
            api_key: "env-test",
        };
        let command = production_command(&request).expect("build child command");

        // The process-global environment is never mutated.
        assert_eq!(std::env::var_os("PATH"), before);

        let child_path = command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("PATH"))
            .and_then(|(_, value)| value)
            .expect("child PATH is set")
            .to_owned();
        let entries: Vec<PathBuf> = std::env::split_paths(&child_path).collect();
        assert_eq!(entries[..2], prefix[..]);
        if let Some(inherited) = before {
            let inherited_entries: Vec<PathBuf> = std::env::split_paths(&inherited).collect();
            assert!(entries.ends_with(&inherited_entries));
        }
    }

    #[test]
    fn production_command_with_empty_prefix_leaves_child_path_inherited() {
        let args = [OsString::from("--version")];
        let request = SpawnRequest {
            executable: Path::new("llama-server"),
            args: &args,
            path_prefix: &[],
            port: 0,
            model_alias: "env-test",
            api_key: "env-test",
        };
        let command = production_command(&request).expect("build child command");
        assert!(
            command.get_envs().all(|(key, _)| key != OsStr::new("PATH")),
            "an empty prefix must not override the child's inherited PATH"
        );
    }
}
