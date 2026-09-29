//! Building the internal error: its constructors, the `From` bridges that
//! map the gateway client, parser, and Lua crates' errors onto it variant
//! for variant, and the rebuild of a raised Lua error table into the
//! variant it stands in for.

use std::borrow::Cow;

use promptforge_lua::Error as LuaError;
use promptforge_model_client::Error as GatewayClientError;
use promptforge_parser::Error as ParserError;
use promptforge_types::ids::TaskId;

use super::Error;

impl Error {
    /// Builds a parse failure with a stable classification and no source span.
    pub(crate) fn parse(kind: crate::parser::ParseErrorKind, message: impl Into<String>) -> Error {
        Error::ParseStructured {
            kind,
            span: None,
            message: message.into(),
            name: None,
            line: None,
            column: None,
        }
    }

    /// Stamps the prompt's frontmatter name onto a parse failure raised by
    /// `run` itself: the prompt is already parsed at that point, so the
    /// name is known and the location can name it. Any other variant passes
    /// through unchanged.
    pub(crate) fn with_prompt_name(mut self, name: &str) -> Error {
        if let Error::ParseStructured { name: slot, .. } = &mut self {
            *slot = Some(name.to_owned());
        }
        self
    }

    /// Builds an internal-invariant failure, capturing the Rust source
    /// position of the call site so [`crate::RunError::location`] can point
    /// at the broken invariant.
    #[track_caller]
    pub(crate) fn internal(message: &'static str) -> Error {
        let location = std::panic::Location::caller();
        Error::Internal {
            message,
            file: location.file(),
            line: location.line(),
        }
    }

    /// Wraps a store failure that has no operation to render - a handle
    /// acquisition, the declared-store probe - as [`Error::Store`] with
    /// the fixed probe message.
    pub(crate) fn store(source: promptforge_vfs::VfsError) -> Error {
        Error::Store {
            message: "store operation failed".to_owned(),
            source,
        }
    }

    /// Wraps one store operation's failure as [`Error::Store`], rendering
    /// the model-facing message with the operation for its wording.
    pub(crate) fn store_op(
        op: &promptforge_lua::StoreOp,
        source: promptforge_vfs::VfsError,
    ) -> Error {
        Error::Store {
            message: crate::lua::store_error_message(op, &source),
            source,
        }
    }

    /// Wraps an `mlua` failure as [`Error::LuaRuntime`], preserving it as the
    /// `#[source]` cause rather than flattening it to a string.
    #[cfg(test)]
    pub(crate) fn lua(source: mlua::Error) -> Error {
        Error::LuaRuntime {
            message: source.to_string(),
            source: Box::new(source),
        }
    }
}

impl From<crate::subst::SubstitutionError> for Error {
    fn from(error: crate::subst::SubstitutionError) -> Error {
        Error::Substitution(Box::new(error))
    }
}

/// Maps the gateway-client error type back onto this one variant for
/// variant, so `Display`, `source()` chains, and `RunError`/`CompletionError`
/// classification are unchanged by the extraction. The client crate's
/// error type is `#[non_exhaustive]`; a variant this match does not name
/// becomes [`Error::Config`] with its display text and itself as the source,
/// so it classifies as a completion failure that is not retryable.
impl From<GatewayClientError> for Error {
    fn from(error: GatewayClientError) -> Error {
        match error {
            GatewayClientError::MissingEnv(name) => Error::MissingEnv(name),
            GatewayClientError::InvalidEnv(name) => Error::InvalidEnv(name),
            GatewayClientError::InvalidConfig(detail) => Error::InvalidConfig(detail),
            GatewayClientError::Config { message, source } => Error::Config { message, source },
            GatewayClientError::GatewayDisabled => Error::GatewayDisabled,
            GatewayClientError::Http(source) => Error::Http(source),
            GatewayClientError::Backend { status, body } => Error::Backend { status, body },
            GatewayClientError::MalformedResponse(message) => Error::MalformedResponse(message),
            GatewayClientError::MalformedResponseSource { message, source } => {
                Error::MalformedResponseSource { message, source }
            }
            GatewayClientError::BackendBodyRead { status, source } => {
                Error::BackendBodyRead { status, source }
            }
            GatewayClientError::EmptyModelReply {
                detail,
                finish_reason,
            } => Error::EmptyModelReply {
                detail: Cow::Borrowed(detail),
                finish_reason,
            },
            GatewayClientError::ModelSetLock(message) => Error::Lua(message),
            other => Error::Config {
                message: other.to_string(),
                source: Box::new(other),
            },
        }
    }
}

impl From<crate::model::CompletionError> for Error {
    fn from(error: crate::model::CompletionError) -> Error {
        Error::from(GatewayClientError::from(error))
    }
}

/// Maps a parse failure back onto this internal type variant for variant, so
/// `Display`, `source()` chains, and `RunError` classification are unchanged
/// by the parser extraction. The parser crate's error type is not
/// `#[non_exhaustive]` (the two crates version together), so this match is
/// total.
impl From<crate::parser::ParseError> for Error {
    fn from(error: crate::parser::ParseError) -> Self {
        match promptforge_parser::detail::parse_error_into_inner(error) {
            ParserError::ParseFrontmatter {
                message,
                source,
                line,
                column,
            } => Error::ParseFrontmatter {
                message,
                source,
                line,
                column,
            },
            ParserError::ParseStructured {
                kind,
                span,
                message,
                name,
                line,
                column,
            } => Error::ParseStructured {
                kind,
                span,
                message,
                name,
                line,
                column,
            },
            ParserError::Lua(lua) => Error::from(lua),
            ParserError::Internal(message) => Error::internal(message),
        }
    }
}

/// Maps the Lua crate's internal type back onto this one variant for
/// variant, so `Display`, `source()` chains, and `RunError`/`CompletionError`
/// classification are unchanged by the extraction. The Lua crate's error type
/// is not `#[non_exhaustive]` (the two crates version together), so this match
/// is total.
impl From<LuaError> for Error {
    fn from(error: LuaError) -> Error {
        match error {
            LuaError::Lua(message) => Error::Lua(message),
            LuaError::LuaRuntime { message, source } => Error::LuaRuntime { message, source },
            LuaError::LuaCompile {
                location,
                source_line,
                lua_source,
                message,
                source,
            } => Error::LuaCompile {
                location,
                source_line,
                lua_source,
                message,
                source,
            },
            LuaError::LuaQuota { resource } => Error::LuaQuota { resource },
            LuaError::ContextExhausted { reason } => Error::ContextExhausted { reason },
            LuaError::Interrupted => Error::Interrupted,
            LuaError::Tool { message, source } => Error::Tool { message, source },
            LuaError::Store { message, source } => Error::Store { message, source },
            LuaError::Internal(message) => Error::internal(message),
            LuaError::Raised(raised) => Error::from_raised(raised),
        }
    }
}

/// The `task` field of a raised task-error table, when it parses.
fn raised_task(raised: &promptforge_lua::Raised) -> Option<TaskId> {
    raised
        .fields
        .get("task")
        .and_then(promptforge_lua::ErrorField::as_str)
        .and_then(|task| task.parse().ok())
}

/// The `path` field of a raised error table, when it is a string.
fn raised_field<'a>(raised: &'a promptforge_lua::Raised, name: &str) -> Option<&'a str> {
    raised
        .fields
        .get(name)
        .and_then(promptforge_lua::ErrorField::as_str)
}

/// Rebuilds the structured [`promptforge_vfs::VfsError`] a raised store
/// error table stands in for, from its `reason`, its fields, and its
/// message. The table was built from the same variant by this crate's own
/// [`ErrorValue`](promptforge_lua::ErrorValue) rendering, so the
/// reconstruction is exact where the fields round-trip; a malformed table
/// degrades to the closest variant rather than panicking.
fn raised_store_error(raised: &promptforge_lua::Raised) -> promptforge_vfs::VfsError {
    use promptforge_vfs::{PathReason, VfsError};
    let path = raised_field(raised, "path").unwrap_or_default().to_owned();
    match raised_field(raised, "reason") {
        Some("not_found") => VfsError::NotFound { path },
        Some("anchor") => VfsError::Anchor {
            path,
            anchor: raised_field(raised, "anchor")
                .unwrap_or_default()
                .to_owned(),
            count: raised
                .fields
                .get("count")
                .and_then(promptforge_lua::ErrorField::as_integer)
                .and_then(|count| usize::try_from(count).ok())
                .unwrap_or(0),
        },
        Some("invalid_path") => VfsError::InvalidPath {
            path,
            reason: raised_field(raised, "rule")
                .and_then(PathReason::from_tag)
                .unwrap_or(PathReason::Empty),
        },
        Some("invalid_range") => VfsError::InvalidRange {
            path,
            reason: "the bounds are invalid",
        },
        Some("not_utf8") => VfsError::NotUtf8 { path },
        Some("is_a_directory") => VfsError::IsADirectory { path },
        Some("not_a_directory") => VfsError::NotADirectory { path },
        Some("directory_not_empty") => VfsError::DirectoryNotEmpty { path },
        Some("already_exists") => VfsError::AlreadyExists { path },
        // `permission_denied` and `unsupported` carry only `path` beside
        // the message, so their text fields degrade to empty here.
        Some("permission_denied") => VfsError::PermissionDenied {
            path,
            reason: String::new(),
        },
        Some("unsupported") => VfsError::Unsupported {
            path,
            detail: String::new(),
        },
        Some("conflict") => VfsError::Conflict {
            path,
            detail: raised.message.clone(),
        },
        _ => VfsError::Backend {
            message: raised.message.clone(),
        },
    }
}

impl Error {
    /// Maps a structured error table that surfaced as a block's failure
    /// onto the variant its kind names, so a Lua-side raise classifies as
    /// the Rust-raised error it stands in for. A kind whose variant needs
    /// structure the table does not hold (the tool-scope errors, the task
    /// errors, `internal`) keeps its message as a Lua failure; those
    /// classifications arrive with the shims that raise them.
    fn from_raised(raised: promptforge_lua::Raised) -> Error {
        match raised.kind {
            promptforge_lua::ErrorKind::ToolLoopExhausted => Error::ToolLoopExhausted,
            promptforge_lua::ErrorKind::ContextExhausted => match raised.overflow_reason() {
                Some(reason) => Error::ContextExhausted { reason },
                None => Error::Lua(raised.message),
            },
            promptforge_lua::ErrorKind::EmptyModelReply => Error::EmptyModelReply {
                finish_reason: raised
                    .fields
                    .get("finish_reason")
                    .and_then(promptforge_lua::ErrorField::as_str)
                    .map(str::to_owned),
                detail: Cow::Owned(raised.message),
            },
            promptforge_lua::ErrorKind::Cancelled => Error::Interrupted,
            promptforge_lua::ErrorKind::Tool => Error::Tool {
                message: raised.message.clone(),
                source: Box::new(raised),
            },
            promptforge_lua::ErrorKind::TaskNotOwned => match raised_task(&raised) {
                Some(task) => Error::TaskNotOwned { task },
                None => Error::Lua(raised.message),
            },
            promptforge_lua::ErrorKind::TaskConsumed => match raised_task(&raised) {
                Some(task) => Error::TaskConsumed { task },
                None => Error::Lua(raised.message),
            },
            promptforge_lua::ErrorKind::Store => Error::Store {
                message: raised.message.clone(),
                source: raised_store_error(&raised),
            },
            promptforge_lua::ErrorKind::OutOfScopeTool
            | promptforge_lua::ErrorKind::UnboundTool
            | promptforge_lua::ErrorKind::TasksLive
            | promptforge_lua::ErrorKind::Lua
            | promptforge_lua::ErrorKind::Internal => Error::Lua(raised.message),
        }
    }
}
