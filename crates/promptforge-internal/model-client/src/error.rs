//! The crate's error type and the transport's timeout marker.
//!
//! [`Error`] is what a failed model round is built from. Every public
//! boundary returns its own typed error ([`crate::model::CompletionError`],
//! [`crate::model::ModelIdError`]); those wrappers classify this type and
//! preserve it as their source. It is public so a transport can build the
//! [`CompletionError`](crate::model::CompletionError) it answers with, and
//! so `promptforge-engine` can map every variant back onto its own
//! internal type verbatim. It is not marked `#[non_exhaustive]`, so that
//! mapping stays total.

/// A type-erased owned error cause used by the internal error type.
pub(crate) type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// Why a model round or a model-list fetch failed, as a transport builds
/// it.
///
/// A transport does not return this directly: it converts it with
/// [`CompletionError::from`](crate::model::CompletionError) into the
/// [`CompletionError`](crate::model::CompletionError) its round fails
/// with, which classifies it into a
/// [`CompletionErrorKind`](crate::model::CompletionErrorKind) and keeps it
/// as the error's source.
///
/// A transport builds these variants:
/// - while reading its configuration: [`MissingEnv`](Error::MissingEnv),
///   [`InvalidEnv`](Error::InvalidEnv), [`Config`](Error::Config), and
///   [`InvalidConfig`](Error::InvalidConfig)
/// - when the host turned gateway access off:
///   [`GatewayDisabled`](Error::GatewayDisabled)
/// - when a send or a read fails: [`Http`](Error::Http), wrapping a
///   timeout in [`ClientTimeout`](Timeout) first
/// - on a non-success status: [`Backend`](Error::Backend) with the body
///   bounded and escaped, or [`BackendBodyRead`](Error::BackendBodyRead)
///   when that body cannot be read
/// - on a body it decodes itself and cannot understand:
///   [`MalformedResponse`](Error::MalformedResponse) or
///   [`MalformedResponseSource`](Error::MalformedResponseSource)
///
/// The read loop and the engine raise the rest. The engine maps every
/// variant onto its own error type, so the enum is not
/// `#[non_exhaustive]`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A required environment variable was missing.
    #[error("missing environment variable: {0}")]
    MissingEnv(String),

    /// An environment variable was set but its value was not valid Unicode.
    #[error("environment variable is set but not valid Unicode: {0}")]
    InvalidEnv(String),

    /// A client or endpoint configuration value failed semantic validation.
    #[error("{0}")]
    InvalidConfig(String),

    /// A client or endpoint configuration input was invalid, retaining the
    /// concrete cause (a secret or URL validation failure) as a private
    /// `#[source]` (client F13 / AUDIT-DISCARDED-SOURCE) instead of flattening
    /// it into the message.
    #[error("{message}")]
    Config {
        /// The human-readable configuration diagnostic (no raw source dump).
        message: String,
        /// The originating validation failure (secret or URL parse), kept as
        /// the cause.
        #[source]
        source: BoxedSource,
    },

    /// Gateway access was explicitly disabled by the host.
    #[error("gateway access is disabled")]
    GatewayDisabled,

    /// The HTTP request to the model backend failed at the transport layer.
    #[error("http transport failure")]
    Http(#[source] BoxedSource),

    /// The backend returned a non-success status.
    ///
    /// The `Display` is deliberately body-free (F5): the bounded,
    /// control-escaped body is stored only in the private `body` field,
    /// reachable through the explicit
    /// [`crate::model::CompletionError::backend_body`] opt-in, so a raw or
    /// hostile payload cannot forge log lines or leak into an error message.
    #[error("non-success backend status {status}")]
    Backend {
        /// The HTTP status code returned by the backend.
        status: u16,
        /// The bounded, control-escaped response body, for opt-in diagnostics.
        body: String,
    },

    /// The backend response could not be understood (missing choices, etc.).
    #[error("malformed response: {0}")]
    MalformedResponse(String),

    /// The backend response could not be decoded, preserving the decoder cause.
    ///
    /// Like [`Error::MalformedResponse`] but retains the underlying decode
    /// failure (for example a [`serde_json::Error`]) as the `#[source]` cause
    /// rather than flattening it into the message (MODEL-009 / client F11), so
    /// the error chain survives through the public wrappers' `source()`.
    #[error("malformed response: {message}")]
    MalformedResponseSource {
        /// The human-readable diagnostic (no raw body).
        message: String,
        /// The originating decode failure, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// Reading a non-success backend response body failed at the transport
    /// layer.
    ///
    /// Retains the transport's own read error as the `#[source]` cause
    /// (MODEL-010) rather than flattening the read failure into display
    /// text, so the error chain (timeout, connection reset) survives. The
    /// status the backend had already returned is preserved for
    /// classification.
    #[error("unreadable backend error body (status {status})")]
    BackendBodyRead {
        /// The non-success HTTP status whose body could not be read.
        status: u16,
        /// The originating transport read failure, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// The model returned neither non-empty tool calls nor non-empty text.
    ///
    /// Reasoning side-channel text, when present, is never promoted into the
    /// answer; `detail` may note that it was ignored, without pasting it. The
    /// choice's `finish_reason` is included so the tool loop can classify the
    /// empty turn (a `"stop"` exit differs from a truncation or a missing
    /// reason).
    #[error("{detail}")]
    EmptyModelReply {
        /// Fixed phrase naming the empty product (and ignored reasoning).
        detail: &'static str,
        /// The choice's `finish_reason`, when the backend supplied one.
        finish_reason: Option<String>,
    },

    /// A lock on the shared model set was poisoned.
    ///
    /// `Display` is the bare message so the engine can reclassify the
    /// failure onto its own Lua-layer variant without a wording change.
    #[error("{0}")]
    ModelSetLock(String),
}

/// A transport failure that was a timeout.
///
/// A transport wraps its own timeout error in this marker before boxing it
/// into [`Error::Http`] or [`Error::BackendBodyRead`], so
/// [`CompletionError::is_timeout`](crate::model::CompletionError::is_timeout)
/// still answers after the concrete type is erased, and the codec never
/// names the transport's HTTP client. The transport's error stays
/// reachable as the `#[source]`.
#[derive(Debug, thiserror::Error)]
#[error("request timed out")]
pub struct Timeout(#[source] pub BoxedSource);

/// Crate-internal result alias over [`Error`].
pub(crate) type Result<T> = std::result::Result<T, Error>;
