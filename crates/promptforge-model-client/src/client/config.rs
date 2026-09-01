//! Client configuration: the redacted bearer secret and the validated
//! gateway endpoint.

use std::fmt;

use crate::Error;
use crate::model::CompletionError;

/// A bearer credential whose contents never appear in `Debug`, `Display`, or
/// logs.
///
/// Wrap any secret (the gateway bearer key) in a `SecretString` at the boundary
/// so an accidental `{:?}` or log line cannot leak it; only crate-internal
/// transport code reads the exposed value to set the `Authorization` header.
#[derive(Clone)]
#[non_exhaustive]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a non-empty secret so it is redacted everywhere it is formatted.
    ///
    /// # Errors
    /// Returns [`SecretError::Empty`] when `secret` is empty (F12), so a client
    /// can never be built to authenticate with a blank bearer credential.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_model_client::client::SecretString;
    ///
    /// let secret = SecretString::new("bearer-token")?;
    /// assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
    /// assert_eq!(format!("{secret}"), "<redacted>");
    /// assert!(SecretString::new("").is_err());
    /// # Ok::<(), promptforge_model_client::client::SecretError>(())
    /// ```
    pub fn new(secret: impl Into<String>) -> std::result::Result<SecretString, SecretError> {
        let secret = secret.into();
        if secret.is_empty() {
            return Err(SecretError::Empty);
        }
        Ok(SecretString(secret))
    }

    /// Builds the empty sentinel used only by the disabled client, which never
    /// sends the credential. Crate-internal so no real credential path can
    /// produce a blank secret.
    pub(crate) fn disabled_placeholder() -> SecretString {
        SecretString(String::new())
    }

    /// Borrows the raw secret. Crate-internal so no downstream code can read a
    /// credential back out of the type.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

/// The reason a [`SecretString`] could not be constructed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SecretError {
    /// The supplied credential was empty.
    #[error("secret must not be empty")]
    Empty,
}

impl From<SecretError> for CompletionError {
    fn from(error: SecretError) -> CompletionError {
        // Classifies as `Config`: an unusable credential is a client
        // configuration problem, not a transport or backend failure. The
        // concrete `SecretError` is preserved as the private source rather than
        // flattened into a string (AUDIT-DISCARDED-SOURCE).
        CompletionError::from(Error::Config {
            message: "gateway bearer key is unusable".to_owned(),
            source: Box::new(error),
        })
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A validated gateway API base URL (the OpenAI-shaped `/v1` root).
///
/// Construction rejects a URL without an `http`/`https` scheme or host, so a
/// client can never be pointed at an unusable endpoint. A trailing slash is
/// trimmed so request paths join cleanly.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayEndpoint {
    pub(crate) url: String,
}

impl GatewayEndpoint {
    /// Validates and normalizes a gateway base URL.
    ///
    /// # Errors
    /// Returns a `Config`-kind [`CompletionError`] when `url` is not a valid
    /// absolute URL, does not use an `http`/`https` scheme, names no host,
    /// embeds credentials (a `user:pass@` component), or carries a query or
    /// fragment (an API root is a bare path). Parsing goes through a strict URL
    /// type (F12) rather than a hand-rolled prefix/host scan.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_model_client::client::GatewayEndpoint;
    ///
    /// let endpoint = GatewayEndpoint::new("https://gateway.example.com/v1/")?;
    /// assert_eq!(endpoint.url(), "https://gateway.example.com/v1");
    /// assert!(GatewayEndpoint::new("ftp://example.com").is_err());
    /// assert!(GatewayEndpoint::new("http://user:pass@host/v1").is_err());
    /// # Ok::<(), promptforge_model_client::model::CompletionError>(())
    /// ```
    pub fn new(url: &str) -> std::result::Result<GatewayEndpoint, CompletionError> {
        let reject = |detail: String| CompletionError::from(Error::InvalidConfig(detail));
        let trimmed = url.trim();
        // Preserve the concrete `url::ParseError` as a private source rather than
        // flattening it into the message (AUDIT-DISCARDED-SOURCE).
        let parsed = url::Url::parse(trimmed).map_err(|error| {
            CompletionError::from(Error::Config {
                message: format!("gateway URL is not a valid URL: {trimmed:?}"),
                source: Box::new(error),
            })
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(reject(format!(
                "gateway URL must use the http or https scheme: {trimmed:?}"
            )));
        }
        match parsed.host_str() {
            None | Some("") => {
                return Err(reject(format!("gateway URL names no host: {trimmed:?}")));
            }
            Some(_) => {}
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(reject(
                "gateway URL must not embed credentials (user:pass@)".to_owned(),
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(reject(
                "gateway URL must not carry a query or fragment".to_owned(),
            ));
        }
        Ok(GatewayEndpoint {
            // Normalized by the URL parser; trim the trailing slash so request
            // paths (`{base}/chat/completions`) join cleanly.
            url: parsed.as_str().trim_end_matches('/').to_string(),
        })
    }

    /// Returns the normalized base URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl TryFrom<&str> for GatewayEndpoint {
    type Error = CompletionError;

    fn try_from(url: &str) -> std::result::Result<GatewayEndpoint, CompletionError> {
        GatewayEndpoint::new(url)
    }
}
