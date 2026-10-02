//! Client configuration: the redacted bearer secret, the validated gateway
//! endpoint, and the error a bad setup reports.

use std::fmt;

/// Why the gateway client could not be set up: a missing or unusable
/// environment variable, bearer key, or endpoint URL.
///
/// This is a setup error, not a model failure. It happens before any round
/// is sent, so it never reaches the Engine and has no
/// [`CompletionErrorKind`](crate::CompletionErrorKind). The message names
/// the variable or the rule that failed, and never a key.
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

    /// A configuration input was invalid, and the concrete cause (a URL
    /// parse failure, an unusable secret) is kept as the source instead of
    /// being flattened into the message.
    #[error("{message}")]
    Config {
        /// The human-readable diagnostic, with no raw source dump.
        message: String,
        /// The originating failure, kept as the cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

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
    /// use harness_gateway_client::SecretString;
    ///
    /// let secret = SecretString::new("bearer-token")?;
    /// assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
    /// assert_eq!(format!("{secret}"), "<redacted>");
    /// assert!(SecretString::new("").is_err());
    /// # Ok::<(), harness_gateway_client::SecretError>(())
    /// ```
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

/// A validated gateway API base URL (the OpenAI-compatible `/v1` root).
///
/// Construction rejects a URL without an `http`/`https` scheme or host, so a
/// client can never be pointed at an unusable endpoint. A trailing slash is
/// trimmed so request paths join cleanly.
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
    /// # Errors
    /// Returns a [`GatewayConfigError`] when `url` is not a valid absolute
    /// URL, does not use an `http`/`https` scheme, names no host, embeds
    /// credentials (a `user:pass@` component), or has a query or fragment (an
    /// API root is a bare path). Parsing goes through a strict URL type (F12)
    /// rather than a hand-rolled prefix/host scan.
    ///
    /// # Examples
    ///
    /// ```
    /// use harness_gateway_client::GatewayEndpoint;
    ///
    /// let endpoint = GatewayEndpoint::new("https://gateway.example.com/v1/")?;
    /// assert_eq!(endpoint.url(), "https://gateway.example.com/v1");
    /// assert!(GatewayEndpoint::new("ftp://example.com").is_err());
    /// assert!(GatewayEndpoint::new("http://user:pass@host/v1").is_err());
    /// # Ok::<(), harness_gateway_client::GatewayConfigError>(())
    /// ```
    pub fn new(url: &str) -> std::result::Result<GatewayEndpoint, GatewayConfigError> {
        let reject = GatewayConfigError::InvalidConfig;
        let trimmed = url.trim();
        // Preserve the concrete `url::ParseError` as a private source rather than
        // flattening it into the message (AUDIT-DISCARDED-SOURCE).
        let parsed = url::Url::parse(trimmed).map_err(|error| GatewayConfigError::Config {
            message: format!("gateway URL is not a valid URL: {trimmed:?}"),
            source: Box::new(error),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(reject(format!(
                "gateway URL must use the http or https scheme: {trimmed:?}"
            )));
        }
        let loopback = match parsed.host() {
            None | Some(url::Host::Domain("")) => {
                return Err(reject(format!("gateway URL names no host: {trimmed:?}")));
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
    /// True for `localhost`, `127.0.0.1` (and the rest of `127.0.0.0/8`), and
    /// `::1`; false for every other name or address. A loopback gateway admits
    /// keyless same-machine callers by default, so
    /// [`GatewayClient::from_env`](crate::GatewayClient::from_env) makes the
    /// bearer key optional exactly when this holds.
    ///
    /// # Examples
    ///
    /// ```
    /// use harness_gateway_client::GatewayEndpoint;
    ///
    /// assert!(GatewayEndpoint::new("http://127.0.0.1:8081/v1")?.is_loopback());
    /// assert!(GatewayEndpoint::new("http://[::1]:8081/v1")?.is_loopback());
    /// assert!(GatewayEndpoint::new("http://localhost:8081/v1")?.is_loopback());
    /// assert!(!GatewayEndpoint::new("http://192.168.1.20:8081/v1")?.is_loopback());
    /// assert!(!GatewayEndpoint::new("https://gateway.example.com/v1")?.is_loopback());
    /// # Ok::<(), harness_gateway_client::GatewayConfigError>(())
    /// ```
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
