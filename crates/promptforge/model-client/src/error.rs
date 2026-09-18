//! The crate's internal error substrate.
//!
//! [`Error`] mirrors the role `promptforge-api-runtime`'s substrate plays there: it is
//! never part of the documented API. Every public boundary returns its own
//! typed error ([`crate::model::CompletionError`], [`crate::client::SecretError`],
//! [`crate::model::ModelIdError`]); those wrappers classify this substrate and
//! preserve its source. The substrate is `#[doc(hidden)]` and re-exported only
//! so `promptforge-api-runtime` can map every variant back onto its own substrate
//! verbatim; it is not a stable API and is not marked `#[non_exhaustive]`, so
//! that mapping stays total.

/// A type-erased owned error cause used by the internal substrate.
pub(crate) type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// The crate's internal error substrate, spanning client transport and
/// catalog transport failures.
///
/// `#[doc(hidden)]`: this type exists in the public item tree only so the
/// companion `promptforge-api-runtime` crate can convert it back onto its own
/// substrate variant-for-variant. It is not host API.
#[derive(Debug, thiserror::Error)]
#[doc(hidden)]
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
    /// The `Display` is deliberately body-free (F5): the bounded, control-escaped
    /// body rides only in the private `body` field, reachable through the
    /// explicit [`crate::model::CompletionError::backend_body`] opt-in, so a raw
    /// or hostile payload cannot forge log lines or leak into an error message.
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
    /// Retains the [`reqwest::Error`] as the `#[source]` cause (MODEL-010)
    /// rather than flattening the read failure into display text, so the error
    /// chain (timeout, connection reset) survives. The status the backend had
    /// already returned is preserved for classification.
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
    /// choice's `finish_reason` rides along so the tool loop can classify the
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
    /// `Display` is the bare message so the companion crate can reclassify the
    /// failure (`promptforge-api-runtime` maps it onto its own Lua-layer variant)
    /// without a wording change.
    #[error("{0}")]
    ModelSetLock(String),
}

impl Error {
    /// Wrap a transport-layer error, hiding its concrete type from the API.
    pub(crate) fn http(source: reqwest::Error) -> Error {
        Error::Http(Box::new(source))
    }
}

/// Crate-internal result alias over the [`Error`] substrate.
pub(crate) type Result<T> = std::result::Result<T, Error>;
