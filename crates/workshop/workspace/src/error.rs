//! The workspace operation failure type and its wire mapping.
//!
//! [`WorkspaceError`] is the boundary between the jail's zone-two
//! failures and the HTTP response: each variant maps to exactly one
//! status code and one machine-readable envelope code, rendered through
//! `workshop-protocol`'s [`ErrorEnvelope`] at the route boundary.
//! Internal failure detail (the source chain) reaches the response body
//! in debug builds only; production bodies stay at each variant's own
//! message.

use std::fmt::Write as _;
use std::io;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

// The JSON cause behind `WorkspaceError::UiStateNotJson`. A caller that
// needs the parse error itself names `shared_error_source` directly;
// this crate does not re-export the wrapper, so there is one name for
// the cause across the workspace rather than one per crate.
use shared_error_source::JsonSource;
use workshop_protocol::ErrorEnvelope;

use crate::workspace_file::{UI_STATE_KEYS, WorkspaceFileError};

/// Whether wire bodies carry internal failure detail. Debug builds append
/// the source chain to the envelope message; production bodies stay at
/// the variant's own message.
const LEAK_DETAIL: bool = cfg!(debug_assertions);

/// A workspace operation failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkspaceError {
    /// A granted path could not be canonicalized.
    #[non_exhaustive]
    #[error("grant path cannot be resolved")]
    ResolveGrant {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A requested path could not be canonicalized.
    #[non_exhaustive]
    #[error("requested path cannot be resolved")]
    ResolvePath {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// Filesystem metadata for a path could not be read.
    #[non_exhaustive]
    #[error("path cannot be inspected")]
    InspectPath {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A directory could not be listed.
    #[non_exhaustive]
    #[error("directory cannot be listed")]
    ListDirectory {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A file could not be read.
    #[non_exhaustive]
    #[error("file cannot be read")]
    ReadFile {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A file could not be written.
    #[non_exhaustive]
    #[error("file cannot be written")]
    WriteFile {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// The path is not inside any granted root.
    #[error("path is outside every granted root")]
    OutsideGrants,

    /// The path carries a `..` or an alternate data stream name.
    #[error("path contains a forbidden component")]
    ForbiddenComponent,

    /// The path does not exist.
    #[error("path does not exist")]
    NotFound,

    /// A revoke named a path that is not a granted root.
    #[error("path is not a granted root")]
    NotGranted,

    /// A tree listing was requested for something that is not a directory.
    #[error("path is not a directory")]
    NotADirectory,

    /// A read or write targeted something that is not a regular file.
    #[error("path is not a file")]
    NotAFile,

    /// The file contains NUL bytes and is not editable text.
    #[error("file is binary, not text")]
    BinaryFile,

    /// The file is not valid UTF-8.
    #[non_exhaustive]
    #[error("file is not utf-8 text")]
    NotUtf8 {
        /// Where the bytes stopped being UTF-8.
        #[source]
        source: std::string::FromUtf8Error,
    },

    /// The file or body exceeds the size limit.
    #[non_exhaustive]
    #[error("file exceeds the {limit}-byte size limit")]
    FileTooLarge {
        /// The size limit that was exceeded.
        limit: u64,
    },

    /// The on-disk conflict token does not match the writer's token.
    #[error("file changed on disk since it was read")]
    ModifiedConflict,

    /// A workspace file was refused: not a PromptForge workspace, or one
    /// at a schema version this build does not read. The message is the
    /// refusal's own required-versus-actual text.
    #[non_exhaustive]
    #[error(transparent)]
    WorkspaceFileRefused {
        /// The refusal.
        source: WorkspaceFileError,
    },

    /// A save-as or duplicate named a path that already exists.
    #[error("workspace file path is already taken; a path with no file at it is required")]
    WorkspaceFileTaken,

    /// The workspace file could not be read, written, or copied.
    #[non_exhaustive]
    #[error("workspace file operation failed")]
    WorkspaceFileFailed {
        /// The underlying file failure.
        #[source]
        source: WorkspaceFileError,
    },

    /// A ui-state put named a key outside the allow-list. The message
    /// lists the allow-list itself so it cannot drift from the keys.
    #[non_exhaustive]
    #[error(
        "ui-state key {0:?} is not allowed; one of {allowed} is required",
        allowed = UI_STATE_KEYS.join(", ")
    )]
    UiStateKey(String),

    /// A ui-state value exceeds the size cap.
    #[non_exhaustive]
    #[error("ui-state value is {actual} bytes; at most {cap} bytes are allowed")]
    UiStateTooLarge {
        /// The size of the refused value.
        actual: usize,
        /// The cap it exceeded.
        cap: usize,
    },

    /// A ui-state value does not parse as JSON.
    #[non_exhaustive]
    #[error("ui-state value is not JSON")]
    UiStateNotJson {
        /// The parse refusal.
        #[source]
        source: JsonSource,
    },
}

impl From<WorkspaceFileError> for WorkspaceError {
    /// Sorts a file failure into the wire shape the client can act on: a
    /// missing file is the ordinary not-found, a taken path a conflict,
    /// a refused stamp the client's mistake, and the rest the server's.
    fn from(source: WorkspaceFileError) -> Self {
        match source {
            WorkspaceFileError::NotAWorkspace { .. }
            | WorkspaceFileError::UnsupportedVersion { .. } => {
                Self::WorkspaceFileRefused { source }
            }
            WorkspaceFileError::Io { source: ref cause }
                if cause.kind() == io::ErrorKind::NotFound =>
            {
                Self::NotFound
            }
            WorkspaceFileError::Io { source: ref cause }
                if cause.kind() == io::ErrorKind::AlreadyExists =>
            {
                Self::WorkspaceFileTaken
            }
            WorkspaceFileError::Io { .. }
            | WorkspaceFileError::Database { .. }
            | WorkspaceFileError::Closed => Self::WorkspaceFileFailed { source },
        }
    }
}

impl WorkspaceError {
    /// The one HTTP status this failure answers with.
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::NotADirectory
            | Self::NotAFile
            | Self::WorkspaceFileRefused { .. }
            | Self::UiStateKey(_)
            | Self::UiStateNotJson { .. } => StatusCode::BAD_REQUEST,
            Self::OutsideGrants | Self::ForbiddenComponent => StatusCode::FORBIDDEN,
            Self::NotFound | Self::NotGranted => StatusCode::NOT_FOUND,
            Self::BinaryFile | Self::NotUtf8 { .. } => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::FileTooLarge { .. } | Self::UiStateTooLarge { .. } => {
                StatusCode::PAYLOAD_TOO_LARGE
            }
            Self::ModifiedConflict | Self::WorkspaceFileTaken => StatusCode::CONFLICT,
            Self::ResolveGrant { .. }
            | Self::ResolvePath { .. }
            | Self::InspectPath { .. }
            | Self::ListDirectory { .. }
            | Self::ReadFile { .. }
            | Self::WriteFile { .. }
            | Self::WorkspaceFileFailed { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The machine-readable code of the JSON error envelope.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::ResolveGrant { .. } => "resolve_grant",
            Self::ResolvePath { .. } => "resolve_path",
            Self::InspectPath { .. } => "inspect_path",
            Self::ListDirectory { .. } => "list_directory",
            Self::ReadFile { .. } => "read_file",
            Self::WriteFile { .. } => "write_file",
            Self::OutsideGrants => "outside_grants",
            Self::ForbiddenComponent => "forbidden_component",
            Self::NotFound => "not_found",
            Self::NotGranted => "not_granted",
            Self::NotADirectory => "not_a_directory",
            Self::NotAFile => "not_a_file",
            Self::BinaryFile => "binary_file",
            Self::NotUtf8 { .. } => "not_utf8",
            Self::FileTooLarge { .. } => "file_too_large",
            Self::ModifiedConflict => "modified_conflict",
            Self::WorkspaceFileRefused { .. } => "workspace_file_refused",
            Self::WorkspaceFileTaken => "workspace_file_taken",
            Self::WorkspaceFileFailed { .. } => "workspace_file_failed",
            Self::UiStateKey(_) => "ui_state_key",
            Self::UiStateTooLarge { .. } => "ui_state_too_large",
            Self::UiStateNotJson { .. } => "ui_state_not_json",
        }
    }
}

impl IntoResponse for WorkspaceError {
    fn into_response(self) -> Response {
        let status = self.status();
        let envelope = ErrorEnvelope::new(render_message(&self, LEAK_DETAIL), self.code());
        // Serializing the envelope cannot fail: two strings only.
        // A body that somehow cannot serialize degrades to the
        // status line's own text.
        let body = serde_json::to_string(&envelope)
            .unwrap_or_else(|_| status.canonical_reason().unwrap_or("error").to_string());
        (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
    }
}

/// Renders the envelope message for `error`: its own `Display` text, with
/// the source chain appended as `: cause` segments when `leak_detail` is
/// set.
fn render_message(error: &WorkspaceError, leak_detail: bool) -> String {
    let mut message = error.to_string();
    if leak_detail {
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            // fmt::Write to a String cannot fail; the Result is a trait
            // artifact.
            let _ = write!(message, ": {cause}");
            source = cause.source();
        }
    }
    message
}

#[cfg(test)]
#[path = "error-tests.rs"]
mod tests;
