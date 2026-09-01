//! The opaque wire error every HTTP failure answers with.
//!
//! [`AppError`] is the boundary between zone-two failures and the HTTP
//! response: one variant per wire failure that exists today, each mapped to
//! exactly one status code by the central [`IntoResponse`] impl, so the
//! same failure is built in one place no matter which handler hits it.
//! Conversions in are explicit - handler seams name a variant constructor,
//! and workspace failures go through the deliberate `From<WorkspaceError>`
//! mapping below; no `#[from]` derive exists on this side of the boundary.
//! Internal failure detail (the source chain) reaches the response body in
//! debug builds only; production bodies stay at each variant's own message,
//! close to the status text. Rich construction-time errors live elsewhere
//! ([`crate::config::ConfigError`], [`crate::serve::SpawnError`]) and never
//! cross the wire.

use std::fmt::Write as _;
use std::io;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::gateway::GatewayError;
use crate::workspace::WorkspaceError;

/// Whether wire bodies carry internal failure detail. Debug builds append
/// the source chain to the envelope message; production bodies stay at the
/// variant's own message.
const LEAK_DETAIL: bool = cfg!(debug_assertions);

/// A failure answered over the HTTP wire.
///
/// Every variant renders as exactly one status code. Variants carrying a
/// source keep it out of `Display`; [`render_message`] appends the chain to
/// the response body in debug builds only.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum AppError {
    /// The heartbeat knows the gateway is down, so the call was never
    /// attempted. The message is user-visible in the UI and pinned by the
    /// characterization tests, hence the capitalized wire text.
    #[error("Gateway unreachable")]
    GatewayUnreachable,

    /// An attempted gateway call failed in transport. Transparent so the
    /// wire message stays the [`GatewayError`]'s own summary line, as it
    /// was before the error split.
    #[error(transparent)]
    Gateway(GatewayError),

    /// The request arrived from a cross-site browser context: a
    /// `Sec-Fetch-Site: cross-site` marking, a non-loopback `Host`
    /// (DNS rebinding), or a foreign WebSocket `Origin` (see
    /// [`crate::cross_site`]).
    #[error("cross-site request refused")]
    CrossSite,

    /// A body-bearing request did not declare an `application/json` body.
    #[error("request body is not application/json")]
    NotJson,

    /// The config-panel proxy refused to forward a path outside its
    /// allowlist (see [`crate::routes::gateway_config`]).
    #[error("path is not forwardable to the gateway")]
    ForwardDenied,

    /// A granted workspace path could not be canonicalized.
    #[error("grant path cannot be resolved")]
    ResolveGrant {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A requested workspace path could not be canonicalized.
    #[error("requested path cannot be resolved")]
    ResolvePath {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// Filesystem metadata for a workspace path could not be read.
    #[error("path cannot be inspected")]
    InspectPath {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A workspace directory could not be listed.
    #[error("directory cannot be listed")]
    ListDirectory {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A workspace file could not be read.
    #[error("file cannot be read")]
    ReadFile {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A workspace file could not be written.
    #[error("file cannot be written")]
    WriteFile {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// The path is not inside any granted workspace root.
    #[error("path is outside every granted root")]
    OutsideGrants,

    /// The path carries a `..` or an alternate data stream name.
    #[error("path contains a forbidden component")]
    ForbiddenComponent,

    /// The workspace path does not exist.
    #[error("path does not exist")]
    NotFound,

    /// A revoke named a path that is not a granted workspace root.
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

    /// The file or body exceeds the workspace size limit.
    #[error("file exceeds the {limit}-byte size limit")]
    FileTooLarge {
        /// The size limit that was exceeded.
        limit: u64,
    },

    /// The on-disk modified time does not match the writer's token.
    #[error("file changed on disk since it was read")]
    ModifiedConflict,

    /// An embedded UI asset is missing from the bundle.
    #[error("ui asset not found: {0}")]
    AssetMissing(String),
}

impl AppError {
    /// The one HTTP status this failure answers with.
    fn status(&self) -> StatusCode {
        match self {
            Self::GatewayUnreachable | Self::Gateway(_) => StatusCode::BAD_GATEWAY,
            Self::NotADirectory | Self::NotAFile => StatusCode::BAD_REQUEST,
            Self::OutsideGrants
            | Self::ForbiddenComponent
            | Self::CrossSite
            | Self::ForwardDenied => StatusCode::FORBIDDEN,
            Self::NotFound | Self::NotGranted | Self::AssetMissing(_) => StatusCode::NOT_FOUND,
            Self::BinaryFile | Self::NotUtf8 | Self::NotJson => StatusCode::UNSUPPORTED_MEDIA_TYPE,
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

    /// The machine-readable code of the JSON error envelope, or `None` for
    /// the failures rendered as plain text instead of the envelope.
    fn code(&self) -> Option<&'static str> {
        match self {
            Self::GatewayUnreachable | Self::Gateway(_) => Some("gateway_unreachable"),
            Self::CrossSite => Some("cross_site"),
            Self::NotJson => Some("not_json"),
            Self::ForwardDenied => Some("forward_denied"),
            Self::ResolveGrant { .. } => Some("resolve_grant"),
            Self::ResolvePath { .. } => Some("resolve_path"),
            Self::InspectPath { .. } => Some("inspect_path"),
            Self::ListDirectory { .. } => Some("list_directory"),
            Self::ReadFile { .. } => Some("read_file"),
            Self::WriteFile { .. } => Some("write_file"),
            Self::OutsideGrants => Some("outside_grants"),
            Self::ForbiddenComponent => Some("forbidden_component"),
            Self::NotFound => Some("not_found"),
            Self::NotGranted => Some("not_granted"),
            Self::NotADirectory => Some("not_a_directory"),
            Self::NotAFile => Some("not_a_file"),
            Self::BinaryFile => Some("binary_file"),
            Self::NotUtf8 => Some("not_utf8"),
            Self::FileTooLarge { .. } => Some("file_too_large"),
            Self::ModifiedConflict => Some("modified_conflict"),
            Self::AssetMissing(_) => None,
        }
    }
}

/// The deliberate workspace seam: each domain failure keeps the status,
/// code, and message it answered with before the error split.
impl From<WorkspaceError> for AppError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::ResolveGrant { source } => Self::ResolveGrant { source },
            WorkspaceError::ResolvePath { source } => Self::ResolvePath { source },
            WorkspaceError::InspectPath { source } => Self::InspectPath { source },
            WorkspaceError::ListDirectory { source } => Self::ListDirectory { source },
            WorkspaceError::ReadFile { source } => Self::ReadFile { source },
            WorkspaceError::WriteFile { source } => Self::WriteFile { source },
            WorkspaceError::OutsideGrants => Self::OutsideGrants,
            WorkspaceError::ForbiddenComponent => Self::ForbiddenComponent,
            WorkspaceError::NotFound => Self::NotFound,
            WorkspaceError::NotGranted => Self::NotGranted,
            WorkspaceError::NotADirectory => Self::NotADirectory,
            WorkspaceError::NotAFile => Self::NotAFile,
            WorkspaceError::BinaryFile => Self::BinaryFile,
            WorkspaceError::NotUtf8 => Self::NotUtf8,
            WorkspaceError::FileTooLarge { limit } => Self::FileTooLarge { limit },
            WorkspaceError::ModifiedConflict => Self::ModifiedConflict,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        match self.code() {
            Some(code) => {
                let body = serde_json::json!({
                    "error": {
                        "message": render_message(&self, LEAK_DETAIL),
                        "code": code,
                    }
                });
                (
                    status,
                    [(header::CONTENT_TYPE, "application/json")],
                    body.to_string(),
                )
                    .into_response()
            }
            None => (
                status,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                self.to_string(),
            )
                .into_response(),
        }
    }
}

/// Renders the envelope message for `error`: its own `Display` text, with
/// the source chain appended as `: cause` segments when `leak_detail` is
/// set.
fn render_message(error: &AppError, leak_detail: bool) -> String {
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
mod tests {
    use super::*;

    use crate::app::fixtures::body_bytes;

    /// A distinctive injected cause for leak-boundary assertions.
    fn injected_io() -> io::Error {
        io::Error::other("injected disk failure")
    }

    #[test]
    fn gateway_failures_map_to_bad_gateway() {
        let unreachable = AppError::GatewayUnreachable;
        assert_eq!(unreachable.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(unreachable.code(), Some("gateway_unreachable"));
        let transport = AppError::Gateway(GatewayError::Transport(Box::new(injected_io())));
        assert_eq!(transport.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(transport.code(), Some("gateway_unreachable"));
    }

    #[test]
    fn the_asset_miss_maps_to_not_found_with_no_envelope_code() {
        let miss = AppError::AssetMissing("app.js".to_string());
        assert_eq!(miss.status(), StatusCode::NOT_FOUND);
        assert_eq!(miss.code(), None, "the asset 404 is plain text, not JSON");
    }

    /// Every workspace failure keeps the status, code, and message it
    /// answered with before the error split, through the `From` seam.
    #[test]
    fn workspace_failures_keep_their_wire_mapping_through_the_seam() {
        let cases: Vec<(WorkspaceError, StatusCode, &str)> = vec![
            (
                WorkspaceError::ResolveGrant {
                    source: injected_io(),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "resolve_grant",
            ),
            (
                WorkspaceError::ResolvePath {
                    source: injected_io(),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "resolve_path",
            ),
            (
                WorkspaceError::InspectPath {
                    source: injected_io(),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "inspect_path",
            ),
            (
                WorkspaceError::ListDirectory {
                    source: injected_io(),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "list_directory",
            ),
            (
                WorkspaceError::ReadFile {
                    source: injected_io(),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "read_file",
            ),
            (
                WorkspaceError::WriteFile {
                    source: injected_io(),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "write_file",
            ),
            (
                WorkspaceError::OutsideGrants,
                StatusCode::FORBIDDEN,
                "outside_grants",
            ),
            (
                WorkspaceError::ForbiddenComponent,
                StatusCode::FORBIDDEN,
                "forbidden_component",
            ),
            (WorkspaceError::NotFound, StatusCode::NOT_FOUND, "not_found"),
            (
                WorkspaceError::NotGranted,
                StatusCode::NOT_FOUND,
                "not_granted",
            ),
            (
                WorkspaceError::NotADirectory,
                StatusCode::BAD_REQUEST,
                "not_a_directory",
            ),
            (
                WorkspaceError::NotAFile,
                StatusCode::BAD_REQUEST,
                "not_a_file",
            ),
            (
                WorkspaceError::BinaryFile,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "binary_file",
            ),
            (
                WorkspaceError::NotUtf8,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "not_utf8",
            ),
            (
                WorkspaceError::FileTooLarge { limit: 7 },
                StatusCode::PAYLOAD_TOO_LARGE,
                "file_too_large",
            ),
            (
                WorkspaceError::ModifiedConflict,
                StatusCode::CONFLICT,
                "modified_conflict",
            ),
        ];
        for (error, status, code) in cases {
            let message = error.to_string();
            let wire = AppError::from(error);
            assert_eq!(wire.status(), status, "status for {code}");
            assert_eq!(wire.code(), Some(code), "code for {code}");
            assert_eq!(wire.to_string(), message, "message for {code}");
        }
    }

    #[tokio::test]
    async fn the_json_envelope_carries_message_code_and_content_type() {
        let response = AppError::GatewayUnreachable.into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("the envelope sets content-type");
        assert_eq!(content_type, "application/json");
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("the envelope is JSON");
        assert_eq!(
            json["error"]["message"], "Gateway unreachable",
            "the pinned user-visible message is identical in every build"
        );
        assert_eq!(json["error"]["code"], "gateway_unreachable");
    }

    #[tokio::test]
    async fn the_asset_miss_renders_the_plain_text_diagnostic() {
        let response = AppError::AssetMissing("app.js".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("the diagnostic sets content-type");
        assert_eq!(content_type, "text/plain; charset=utf-8");
        assert_eq!(
            &body_bytes(response).await[..],
            b"ui asset not found: app.js"
        );
    }

    #[test]
    fn production_messages_stay_at_the_variant_text() {
        let read = AppError::ReadFile {
            source: injected_io(),
        };
        assert_eq!(
            render_message(&read, false),
            "file cannot be read",
            "production bodies carry no source detail"
        );
        let gateway = AppError::Gateway(GatewayError::Transport(Box::new(injected_io())));
        assert_eq!(
            render_message(&gateway, false),
            "gateway transport error",
            "the transport failure keeps its pre-split production message"
        );
    }

    #[test]
    fn debug_messages_append_the_source_chain() {
        let read = AppError::ReadFile {
            source: injected_io(),
        };
        assert_eq!(
            render_message(&read, true),
            "file cannot be read: injected disk failure"
        );
    }

    /// Tests run under debug assertions, so the live envelope must carry
    /// the detail the debug side of the boundary promises - in the exact
    /// pre-split message format.
    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn debug_builds_leak_detail_into_the_live_envelope() {
        let response = AppError::ReadFile {
            source: injected_io(),
        }
        .into_response();
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("the envelope is JSON");
        assert_eq!(
            json["error"]["message"],
            "file cannot be read: injected disk failure"
        );
    }
}
