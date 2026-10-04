//! Client configuration: the redacted bearer secret, the validated gateway
//! endpoint, and the error a bad setup reports.

use std::fmt;

/// The error returned when the gateway client cannot be set up because an
/// environment variable, the bearer key, or the endpoint URL is missing or
/// unusable.
///
/// This is a setup error, not a model failure. It happens before the client
/// sends any request, so it never reaches the Engine and has no
/// [`CompletionErrorKind`](crate::CompletionErrorKind). The message names
/// the environment variable or the rule that failed, and never contains a key.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GatewayConfigError {
    /// A required environment variable was missing.
    #[error("missing environment variable: {0}")]
    MissingEnv(String),

    /// An environment variable was set but its value was not valid Unicode.
    #[error("environment variable is set but not valid Unicode: {0}")]
    InvalidEnv(String),

    /// A configuration value failed validation.
    #[error("{0}")]
    InvalidConfig(String),

    /// A configuration input was invalid, and the error keeps the concrete
    /// cause.
    ///
    /// The cause, such as a URL parse failure or an unusable secret, is
    /// available as the error's `source` instead of being copied into the
    /// message.
    #[error("{message}")]
    Config {
        /// A human-readable description of the problem. It does not repeat
        /// the text of the source error.
        message: String,
        /// The underlying error that caused this one.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// A bearer credential whose contents never appear in `Debug` or `Display`
/// output or in logs.
///
/// Wrap a secret, such as the gateway bearer key, in a `SecretString` as soon
/// as you read it, so an accidental `{:?}` or log line cannot leak it. The
/// client reads the value only to set the `Authorization` header of its
/// requests.
#[derive(Clone)]
#[non_exhaustive]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a non-empty secret so it is redacted everywhere it is formatted.
    ///
    /// # Errors
    /// Returns [`SecretError::Empty`] when `secret` is empty, so a client can
    /// never be built to authenticate with a blank bearer credential.
    pub fn new(secret: impl Into<String>) -> std::result::Result<SecretString, SecretError> {
        let secret = secret.into();
        if secret.is_empty() {
            return Err(SecretError::Empty);
        }
        Ok(SecretString(secret))
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

impl From<SecretError> for GatewayConfigError {
    fn from(error: SecretError) -> GatewayConfigError {
        // An unusable credential is a client setup problem. The concrete
        // `SecretError` is preserved as the source rather than flattened
        // into a string (AUDIT-DISCARDED-SOURCE).
        GatewayConfigError::Config {
            message: "gateway bearer key is unusable".to_owned(),
            source: Box::new(error),
        }
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

/// A validated base URL for the gateway's OpenAI-compatible API (its `/v1`
/// root).
///
/// Construction rejects a URL that has no `http` or `https` scheme or no
/// host, so a client can never be pointed at an unusable endpoint. Any
/// trailing slash is removed so request paths join cleanly.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayEndpoint {
    pub(crate) url: String,
    /// Whether the host names the local machine: `localhost`, or an IP whose
    /// `is_loopback()` holds. Decided at construction from the parsed host.
    loopback: bool,
}

impl GatewayEndpoint {
    /// Validates and normalizes a gateway base URL.
    ///
    /// The URL is checked with a strict URL parser.
    ///
    /// # Errors
    /// Returns a [`GatewayConfigError`] when `url` is not a valid absolute
    /// URL, does not use an `http` or `https` scheme, names no host, embeds
    /// credentials (a `user:pass@` component), or has a query or fragment.
    /// A query or fragment is rejected because an API root is a bare path.
    /// No error includes `url`, because a URL can contain a credential.
    pub fn new(url: &str) -> std::result::Result<GatewayEndpoint, GatewayConfigError> {
        let reject = GatewayConfigError::InvalidConfig;
        let trimmed = url.trim();
        // Preserve the concrete `url::ParseError` as a private source rather than
        // flattening it into the message (AUDIT-DISCARDED-SOURCE).
        let parsed = url::Url::parse(trimmed).map_err(|error| GatewayConfigError::Config {
            message: "gateway URL is not a valid URL".to_owned(),
            source: Box::new(error),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(reject(
                "gateway URL must use the http or https scheme".to_owned(),
            ));
        }
        let loopback = match parsed.host() {
            None | Some(url::Host::Domain("")) => {
                return Err(reject("gateway URL names no host".to_owned()));
            }
            // The URL parser lowercases the host of an http(s) URL, so the
            // literal comparison covers `LOCALHOST` too.
            Some(url::Host::Domain(domain)) => domain == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        };
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(reject(
                "gateway URL must not embed credentials (user:pass@)".to_owned(),
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(reject(
                "gateway URL must not include a query or fragment".to_owned(),
            ));
        }
        Ok(GatewayEndpoint {
            // Normalized by the URL parser; trim the trailing slash so request
            // paths (`{base}/chat/completions`) join cleanly.
            url: parsed.as_str().trim_end_matches('/').to_string(),
            loopback,
        })
    }

    /// Returns the normalized base URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns whether the endpoint's host is the local machine.
    ///
    /// It returns `true` for `localhost`, for `127.0.0.1` and the rest of
    /// `127.0.0.0/8`, and for `::1`. It returns `false` for every other name
    /// or address. A gateway on the local machine accepts callers that send
    /// no key by default, so
    /// [`GatewayChat::from_env`](crate::GatewayChat::from_env) makes the
    /// bearer key optional exactly when this returns `true`.
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        self.loopback
    }
}

impl TryFrom<&str> for GatewayEndpoint {
    type Error = GatewayConfigError;

    fn try_from(url: &str) -> std::result::Result<GatewayEndpoint, GatewayConfigError> {
        GatewayEndpoint::new(url)
    }
}
