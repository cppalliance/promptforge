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

use workshop_protocol::ErrorEnvelope;

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
    #[error("file is not utf-8 text")]
    NotUtf8,

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
}

impl WorkspaceError {
    /// The one HTTP status this failure answers with.
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::NotADirectory | Self::NotAFile => StatusCode::BAD_REQUEST,
            Self::OutsideGrants | Self::ForbiddenComponent => StatusCode::FORBIDDEN,
            Self::NotFound | Self::NotGranted => StatusCode::NOT_FOUND,
            Self::BinaryFile | Self::NotUtf8 => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::FileTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::ModifiedConflict => StatusCode::CONFLICT,
            Self::ResolveGrant { .. }
            | Self::ResolvePath { .. }
            | Self::InspectPath { .. }
            | Self::ListDirectory { .. }
            | Self::ReadFile { .. }
            | Self::WriteFile { .. } => StatusCode::INTERNAL_SERVER_ERROR,
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
            Self::NotUtf8 => "not_utf8",
            Self::FileTooLarge { .. } => "file_too_large",
            Self::ModifiedConflict => "modified_conflict",
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
