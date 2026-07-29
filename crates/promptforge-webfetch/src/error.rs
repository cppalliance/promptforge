//! The `web_fetch` error type.
//!
//! [`FetchError`] is the single error type for every `web_fetch` failure mode.
//! This step introduces it whole: the URL-policy variants, a model-facing
//! rendering that later steps can trim of internal detail, and the conversion
//! into [`promptforge_core::Error`] (which is what the `Tool` trait returns).
//! Later steps add only the variants their own failure mode needs.

use promptforge_core::Error;

/// An error produced while fetching a URL.
///
/// The [`Display`](std::fmt::Display) rendering is the full log text; the
/// [`FetchError::model_facing`] rendering is what a model should see. They are
/// equal for the current variants but diverge once a variant carries internal
/// detail (a resolved private address, say) that must not reach the model.
///
/// The type is `Send + Sync + 'static` and never exposes a dependency's error
/// type: a URL parse failure is captured as a string, not as [`url::ParseError`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FetchError {
    /// The URL could not be parsed.
    #[non_exhaustive]
    #[error("invalid url: {0}")]
    InvalidUrl(String),

    /// The URL's scheme is not permitted by the policy.
    #[non_exhaustive]
    #[error("scheme not allowed: {0}")]
    BlockedScheme(String),

    /// The URL carries userinfo (a `user:pass@` component).
    #[error("url must not contain userinfo")]
    Userinfo,

    /// The URL's effective port is not on the allowlist.
    #[non_exhaustive]
    #[error("port not allowed: {0}")]
    BlockedPort(u16),

    /// The URL's host is a bare IP literal, which the policy forbids.
    #[non_exhaustive]
    #[error("ip literal host not allowed: {0}")]
    IpLiteral(String),
}

impl FetchError {
    /// Renders the error as text safe to return to the model.
    ///
    /// Currently identical to the [`Display`](std::fmt::Display) output. The
    /// method exists so later variants can omit internal detail (for example a
    /// resolved private address) from what the model sees while the full text
    /// still reaches the logs.
    #[must_use]
    pub fn model_facing(&self) -> String {
        self.to_string()
    }
}

impl From<FetchError> for Error {
    /// Maps a fetch failure onto the core error the `Tool` trait returns.
    ///
    /// URL-policy rejections are input errors caught before any network access,
    /// so they map to [`Error::Parse`] carrying the model-facing text.
    fn from(err: FetchError) -> Error {
        Error::Parse(err.model_facing())
    }
}
