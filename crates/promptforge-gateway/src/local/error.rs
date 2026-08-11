//! Errors from local model provisioning and `llama-server` lifecycle.

use std::io;
use std::path::PathBuf;

/// A failure while downloading, verifying, or launching a local model.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum LocalError {
    /// The host OS/arch has no pinned `llama-server` archive.
    #[error("unsupported llama-server platform `{os}/{arch}`")]
    UnsupportedPlatform {
        /// Operating system triple fragment (`windows`, `linux`, `macos`).
        os: String,
        /// CPU architecture (`x86_64`, `aarch64`).
        arch: String,
    },

    /// Building the HTTP client failed.
    #[error("build HTTP client")]
    HttpClient(#[source] reqwest::Error),

    /// Downloading a URL failed.
    #[error("download `{url}`")]
    Download {
        /// The URL that failed.
        url: String,
        /// The underlying transport error.
        #[source]
        source: reqwest::Error,
    },

    /// Reading the download body failed.
    #[error("read download from `{url}`")]
    DownloadRead {
        /// The URL being read.
        url: String,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A filesystem operation under the cache failed.
    #[error("{operation} `{path}`")]
    Io {
        /// Short name of the operation.
        operation: &'static str,
        /// Path involved in the failure.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A configured `sha256` pin was not a 64-character lowercase-normalizable
    /// hex string.
    #[error("invalid sha-256 pin `{value}`: {reason}")]
    InvalidDigest {
        /// The offending configured digest string.
        value: String,
        /// Why it was rejected.
        reason: &'static str,
    },

    /// A downloaded or cached blob did not match its pin.
    #[error("sha-256 mismatch for `{name}`: expected {expected}, got {actual}")]
    DigestMismatch {
        /// Artifact display name.
        name: String,
        /// Expected lowercase hex digest.
        expected: String,
        /// Actual lowercase hex digest.
        actual: String,
    },

    /// An archive entry was rejected as unsafe.
    #[error("unsafe or unsupported entry `{entry}` in archive `{archive}`")]
    UnsafeArchiveEntry {
        /// Archive path (display form).
        archive: String,
        /// Rejected entry name.
        entry: String,
    },

    /// Reading or unpacking an archive failed.
    #[error("read archive `{archive}`")]
    Archive {
        /// Archive path (display form).
        archive: String,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The archive did not contain the expected executable.
    #[error("archive `{archive}` does not contain `{executable}`")]
    MissingExecutable {
        /// Archive path (display form).
        archive: String,
        /// Expected executable basename.
        executable: String,
    },

    /// The archive contained more than one matching executable.
    #[error("archive `{archive}` contains more than one `{executable}`")]
    DuplicateExecutable {
        /// Archive path (display form).
        archive: String,
        /// Executable basename that collided.
        executable: String,
    },

    /// A path was not valid UTF-8.
    #[error("invalid UTF-8 path inside `{path}`")]
    InvalidPath {
        /// The offending path.
        path: PathBuf,
    },

    /// A cache path escaped the cache root or crossed a link.
    #[error("cache path `{path}` escapes the cache or contains a link/reparse point")]
    UnsafeCachePath {
        /// The offending path.
        path: PathBuf,
    },

    /// A `[[local_model]].source` value could not be interpreted.
    #[error("invalid local model source `{value}`: {reason}")]
    InvalidSource {
        /// The configured source string.
        value: String,
        /// Why it was rejected.
        reason: String,
    },

    /// Spawning the `llama-server` child process failed.
    #[error("start llama-server at `{executable}`")]
    Spawn {
        /// The executable that could not be spawned.
        executable: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// Selecting or reading a free loopback port failed.
    #[error("{operation}")]
    Port {
        /// Short description of the port operation.
        operation: &'static str,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A child output stream handle was unavailable for capture.
    #[error("capture llama-server {stream}")]
    Capture {
        /// The stream name (`stdout`/`stderr`).
        stream: &'static str,
    },

    /// Spawning a capture reader thread failed.
    #[error("start {stream} capture thread")]
    CaptureThread {
        /// The capture thread name.
        stream: &'static str,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// Building the readiness HTTP client failed.
    #[error("build llama-server readiness client")]
    ReadinessClient {
        /// The underlying transport error.
        #[source]
        source: reqwest::Error,
    },

    /// Inspecting the child process (`try_wait`) failed.
    #[error("inspect llama-server process")]
    Inspect {
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// Killing the child process failed.
    #[error("kill llama-server child")]
    Kill {
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// `llama-server` startup was interrupted by Ctrl-C.
    #[error("llama-server startup interrupted by Ctrl-C")]
    StartupInterrupted,

    /// `llama-server` did not expose its authenticated model within the deadline.
    #[error("llama-server did not expose its authenticated model within {seconds} seconds")]
    ReadinessTimeout {
        /// The readiness deadline, in seconds.
        seconds: u64,
    },

    /// The child exited before it became ready.
    #[error("llama-server exited before readiness with {status}")]
    EarlyExit {
        /// The child's exit status (rendered).
        status: String,
    },

    /// A killed child was not reaped within the bounded teardown deadline.
    #[error("llama-server child did not exit within the teardown deadline")]
    TeardownTimeout,

    /// Every fresh-port startup attempt hit a child bind collision.
    #[error(
        "llama-server exhausted {attempts} fresh-port attempts after child bind collisions\n{detail}"
    )]
    PortCollisions {
        /// Number of attempts made.
        attempts: usize,
        /// Per-attempt diagnostics.
        detail: String,
    },

    /// A `llama-server` startup attempt failed for a non-collision reason.
    #[error("llama-server invocation failed\n{detail}")]
    Startup {
        /// Invocation and captured-output diagnostics.
        detail: String,
        /// The underlying readiness failure.
        #[source]
        source: Box<LocalError>,
    },

    /// A respawn on the fixed port hit a bind collision.
    #[error("llama-server respawn hit a port collision on {port}\n{detail}")]
    RespawnPortCollision {
        /// The fixed port that collided.
        port: u16,
        /// Invocation and captured-output diagnostics.
        detail: String,
    },

    /// A dead local child is still within its respawn cooldown window.
    #[error("llama-server for {model} exited; respawn cooldown active")]
    RespawnCooldown {
        /// The affected model name.
        model: String,
    },

    /// Resolving lane concurrency for a local model failed.
    #[error("resolve lane concurrency for local model {model}")]
    LaneConcurrency {
        /// The affected model name.
        model: String,
        /// The underlying configuration error.
        #[source]
        source: crate::error::ConfigError,
    },

    /// A lane concurrency value did not fit the child's `--parallel` argument.
    #[error("lane concurrency {concurrency} does not fit in u32")]
    LaneTooLarge {
        /// The offending concurrency value.
        concurrency: usize,
    },

    /// A downloaded artifact exceeded the size ceiling.
    #[error("artifact at `{url}` exceeds the {limit}-byte limit")]
    ArtifactTooLarge {
        /// The artifact URL.
        url: String,
        /// The byte ceiling.
        limit: u64,
    },

    /// Probing the child's tool-dialect endpoints failed at the transport layer.
    #[error("{operation}")]
    DialectProbe {
        /// Short description of the probe operation.
        operation: &'static str,
        /// The underlying transport error.
        #[source]
        source: reqwest::Error,
    },

    /// A dialect-probe endpoint returned a non-success status.
    #[error("{operation} returned {status}")]
    DialectProbeStatus {
        /// Short description of the probe operation.
        operation: &'static str,
        /// The status returned (rendered).
        status: String,
    },

    /// Resolving the tool dialect from `/props` evidence failed.
    #[error("dialect resolution failed for local model {model}")]
    DialectResolution {
        /// The affected model name.
        model: String,
        /// The underlying resolution error (boxed: the core error is large).
        #[source]
        source: Box<promptforge_core::dialects::DialectError>,
    },
}

impl LocalError {
    /// Whether the failure is plausibly transient - a transport fault, process
    /// liveness issue, or port contention - rather than a permanent integrity,
    /// configuration, or validation fault. Used to annotate respawn diagnostics.
    #[must_use]
    pub(crate) fn is_retryable(&self) -> bool {
        matches!(
            self,
            LocalError::HttpClient(_)
                | LocalError::Download { .. }
                | LocalError::DownloadRead { .. }
                | LocalError::Spawn { .. }
                | LocalError::Port { .. }
                | LocalError::CaptureThread { .. }
                | LocalError::ReadinessClient { .. }
                | LocalError::Inspect { .. }
                | LocalError::Kill { .. }
                | LocalError::ReadinessTimeout { .. }
                | LocalError::EarlyExit { .. }
                | LocalError::TeardownTimeout
                | LocalError::PortCollisions { .. }
                | LocalError::RespawnPortCollision { .. }
                | LocalError::Startup { .. }
                | LocalError::DialectProbe { .. }
                | LocalError::DialectProbeStatus { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    fn io() -> io::Error {
        io::Error::other("boom")
    }

    #[test]
    fn is_retryable_classifies_transient_versus_permanent() {
        // Transient: transport, process liveness, port contention.
        assert!(LocalError::TeardownTimeout.is_retryable());
        assert!(
            LocalError::EarlyExit {
                status: "signal: 9".to_owned()
            }
            .is_retryable()
        );
        assert!(LocalError::ReadinessTimeout { seconds: 180 }.is_retryable());
        assert!(
            LocalError::Spawn {
                executable: PathBuf::from("llama-server"),
                source: io(),
            }
            .is_retryable()
        );
        assert!(LocalError::Kill { source: io() }.is_retryable());
        // Permanent: integrity, config, validation, and misuse.
        assert!(!LocalError::StartupInterrupted.is_retryable());
        assert!(
            !LocalError::DigestMismatch {
                name: "m".to_owned(),
                expected: "a".to_owned(),
                actual: "b".to_owned(),
            }
            .is_retryable()
        );
        assert!(
            !LocalError::LaneTooLarge {
                concurrency: 1 << 40
            }
            .is_retryable()
        );
        assert!(
            !LocalError::RespawnCooldown {
                model: "m".to_owned()
            }
            .is_retryable()
        );
        assert!(!LocalError::Capture { stream: "stdout" }.is_retryable());
    }

    #[test]
    fn source_bearing_variants_preserve_their_cause_without_doubling_display() {
        let spawn = LocalError::Spawn {
            executable: PathBuf::from("llama-server"),
            source: io(),
        };
        assert!(spawn.source().is_some());

        // A wrapped readiness failure is preserved as `source()`, and the outer
        // Display renders only the wrapper message (no doubled chain).
        let startup = LocalError::Startup {
            detail: "invocation + diagnostics".to_owned(),
            source: Box::new(LocalError::ReadinessTimeout { seconds: 5 }),
        };
        assert!(startup.source().is_some());
        assert!(startup.to_string().contains("invocation + diagnostics"));
        assert!(!startup.to_string().contains("did not expose"));
    }
}
