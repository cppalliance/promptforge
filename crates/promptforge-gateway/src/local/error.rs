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

    /// Spawning or waiting for `llama-server` failed.
    #[error("{0}")]
    Server(String),
}
