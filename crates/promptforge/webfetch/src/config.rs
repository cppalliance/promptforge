//! Validated configuration for the `web_fetch` tool's security policy.
//!
//! [`FetchConfig`] is an opaque, immutable policy value: its fields are private
//! validated newtypes, so a constructed value can never hold an invalid state.
//! Build one with [`FetchConfig::builder`] and [`FetchConfigBuilder::build`],
//! which validates every field and reports a [`ConfigError`], or take the
//! built-in safe policy with [`FetchConfig::default`].
//!
//! The policy governs what a fetch may do: the URL-policy knobs (`allow_http`,
//! `allow_ports`, `allow_ip_literals`), the address policy (denied CIDR ranges
//! and exact host-plus-address exceptions), the size caps (`max_bytes`,
//! `max_chars`), the redirect cap, the timeouts, and the `User-Agent`. The set
//! of accepted content types is fixed rather than configured.

use std::net::IpAddr;
use std::time::Duration;

use ipnet::IpNet;
use reqwest::header::HeaderValue;

/// The default ports a fetch may target: HTTP and HTTPS.
const DEFAULT_ALLOW_PORTS: [u16; 2] = [80, 443];

/// The default cap on redirect hops a single fetch may follow.
const DEFAULT_MAX_REDIRECTS: usize = 5;

/// The hard ceiling on the redirect cap accepted by the builder.
const MAX_REDIRECTS_CEILING: usize = 20;

/// The default time allowed to establish a TCP connection.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The default cap on the total time a single request may take.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// The default time an idle connection is kept in the pool before it is closed.
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The hard ceiling on the connect timeout accepted by the builder.
const MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// The hard ceiling on the whole-request timeout accepted by the builder.
const MAX_TIMEOUT: Duration = Duration::from_secs(300);

/// The hard ceiling on the pool-idle timeout accepted by the builder.
const MAX_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// The default `User-Agent` header sent on every request.
const DEFAULT_USER_AGENT: &str = "promptforge-webfetch/0.0";

/// The default cap on a response body's decompressed size, in bytes (8 MiB).
const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The hard ceiling on `max_bytes` accepted by the builder (64 MiB).
const MAX_BYTES_CEILING: usize = 64 * 1024 * 1024;

/// The default cap on the returned text length, in characters.
const DEFAULT_MAX_CHARS: usize = 40_000;

/// The hard ceiling on `max_chars` accepted by the builder.
const MAX_CHARS_CEILING: usize = 10_000_000;

/// A `User-Agent` string validated to be a legal HTTP header value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserAgent(String);

/// A response-body byte cap, guaranteed in `1..=MAX_BYTES_CEILING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MaxBytes(usize);

/// A returned-text character cap, guaranteed in `1..=MAX_CHARS_CEILING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MaxChars(usize);

/// A redirect-hop cap, guaranteed in `0..=MAX_REDIRECTS_CEILING`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MaxRedirects(usize);

/// A strictly positive [`Duration`], never `Duration::ZERO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PositiveDuration(Duration);

/// An exact host-plus-address exception, with the host canonicalized.
///
/// The host is lowercased, trimmed, and stripped of a single trailing dot, so a
/// case- or trailing-dot variant of the configured host still matches the
/// resolver's representation. Keyed on both host and address, so a rebinding
/// answer for another name cannot inherit this exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostAddressException {
    /// The canonical (lowercased, dot-stripped) host this exception names.
    host: String,
    /// The exact address the exception admits for that host.
    addr: IpAddr,
}

impl HostAddressException {
    /// Returns whether `(host, addr)` matches this exception.
    ///
    /// `host` is canonicalized the same way the entry's host was, so the
    /// comparison is case- and trailing-dot-insensitive.
    #[must_use]
    pub(crate) fn matches(&self, host: &str, addr: IpAddr) -> bool {
        self.addr == addr && self.host == canonical_host(host)
    }
}

/// Canonicalizes a DNS host for exact-exception comparison.
fn canonical_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// An opaque configuration error.
///
/// Its representation is private and free to change. The [`Display`] rendering
/// names the field and the constraint that was violated.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ConfigError(#[from] ConfigErrorRepr);

/// The private representation behind [`ConfigError`].
#[derive(Debug, thiserror::Error)]
enum ConfigErrorRepr {
    /// The user agent is not a legal HTTP header value.
    #[error("user agent is not a valid http header value")]
    UserAgent(#[source] reqwest::header::InvalidHeaderValue),

    /// A limit was zero, which would disable the bound it governs.
    #[error("{field} must be greater than zero")]
    ZeroLimit {
        /// The name of the offending limit.
        field: &'static str,
    },

    /// A limit exceeded its hard operational ceiling.
    #[error("{field} ({value}) exceeds the maximum of {ceiling}")]
    OverCeiling {
        /// The name of the offending limit.
        field: &'static str,
        /// The rejected value.
        value: usize,
        /// The ceiling it exceeded.
        ceiling: usize,
    },

    /// A timeout was zero, which the policy does not allow.
    #[error("{field} must be a positive duration")]
    ZeroTimeout {
        /// The name of the offending timeout.
        field: &'static str,
    },

    /// A timeout exceeded its hard operational ceiling.
    #[error("{field} ({value:?}) exceeds the maximum of {ceiling:?}")]
    TimeoutOverCeiling {
        /// The name of the offending timeout.
        field: &'static str,
        /// The rejected duration.
        value: Duration,
        /// The ceiling it exceeded.
        ceiling: Duration,
    },

    /// A denied-CIDR string did not parse.
    #[error("invalid deny cidr {cidr}")]
    Cidr {
        /// The rejected CIDR text.
        cidr: String,
        /// The parse failure.
        #[source]
        source: ipnet::AddrParseError,
    },

    /// An exact-host exception named an empty or malformed host.
    #[error("invalid exact host {host:?}")]
    Host {
        /// The rejected host text.
        host: String,
    },

    /// The HTTP client could not be built for the validated policy.
    #[error("http client construction failed")]
    ClientBuild(#[source] reqwest::Error),
}

impl ConfigError {
    /// Builds a `ConfigError` from a reqwest client-build failure.
    pub(crate) fn client_build(source: reqwest::Error) -> ConfigError {
        ConfigError(ConfigErrorRepr::ClientBuild(source))
    }
}

/// Security policy for the `web_fetch` tool.
///
/// A `FetchConfig` is immutable and validated: every field is a private newtype
/// that a constructor already checked, so no field can hold an invalid state.
/// Take the built-in safe policy with [`FetchConfig::default`], or customize one
/// through [`FetchConfig::builder`].
///
/// # Examples
/// ```
/// use promptforge_webfetch::FetchConfig;
///
/// let policy = FetchConfig::default();
/// let custom = FetchConfig::builder().allow_http(true).build()?;
/// assert_ne!(policy, custom);
/// # Ok::<(), promptforge_webfetch::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchConfig {
    /// Whether to permit `http://` URLs; `https://` is always allowed.
    allow_http: bool,
    /// The ports a fetch may target (matched against the URL's effective port).
    allow_ports: Vec<u16>,
    /// Whether to permit a host given as a bare IP literal.
    allow_ip_literals: bool,
    /// Extra CIDR ranges denied on top of the built-in blocked ranges.
    deny_extra: Vec<IpNet>,
    /// Exact host-plus-address exceptions allowed even when otherwise blocked.
    allow_exact: Vec<HostAddressException>,
    /// The maximum number of redirect hops a single fetch may follow.
    max_redirects: MaxRedirects,
    /// The largest response body accepted, counted on decompressed bytes.
    max_bytes: MaxBytes,
    /// The ceiling on returned text length, in characters.
    max_chars: MaxChars,
    /// The time allowed to establish a TCP connection on any hop.
    connect_timeout: PositiveDuration,
    /// The cap on the total time a single request may take.
    timeout: PositiveDuration,
    /// How long an idle pooled connection is kept before it is closed.
    pool_idle_timeout: PositiveDuration,
    /// The validated `User-Agent` header sent on every request.
    user_agent: UserAgent,
}

impl FetchConfig {
    /// Starts a builder seeded with the built-in default policy.
    ///
    /// # Examples
    /// ```
    /// use promptforge_webfetch::FetchConfig;
    ///
    /// let policy = FetchConfig::builder().max_chars(10_000).build()?;
    /// # Ok::<(), promptforge_webfetch::ConfigError>(())
    /// ```
    #[must_use]
    pub fn builder() -> FetchConfigBuilder {
        FetchConfigBuilder::default()
    }

    /// Whether plain `http://` URLs are permitted.
    pub(crate) fn allow_http(&self) -> bool {
        self.allow_http
    }

    /// The ports a fetch may target.
    pub(crate) fn allow_ports(&self) -> &[u16] {
        &self.allow_ports
    }

    /// Whether a bare IP-literal host is permitted (syntax only; the address is
    /// still classified against the address policy).
    pub(crate) fn allow_ip_literals(&self) -> bool {
        self.allow_ip_literals
    }

    /// The extra denied CIDR ranges layered on the built-in table.
    pub(crate) fn deny_extra(&self) -> &[IpNet] {
        &self.deny_extra
    }

    /// The exact host-plus-address exceptions.
    pub(crate) fn allow_exact(&self) -> &[HostAddressException] {
        &self.allow_exact
    }

    /// The redirect-hop cap.
    pub(crate) fn max_redirects(&self) -> usize {
        self.max_redirects.0
    }

    /// The response-body byte cap.
    pub(crate) fn max_bytes(&self) -> usize {
        self.max_bytes.0
    }

    /// The returned-text character ceiling.
    pub(crate) fn max_chars(&self) -> usize {
        self.max_chars.0
    }

    /// The per-hop connect timeout.
    pub(crate) fn connect_timeout(&self) -> Duration {
        self.connect_timeout.0
    }

    /// The whole-request timeout.
    pub(crate) fn timeout(&self) -> Duration {
        self.timeout.0
    }

    /// The idle-connection pool timeout.
    pub(crate) fn pool_idle_timeout(&self) -> Duration {
        self.pool_idle_timeout.0
    }

    /// The validated `User-Agent` string.
    pub(crate) fn user_agent(&self) -> &str {
        &self.user_agent.0
    }
}

impl Default for FetchConfig {
    fn default() -> FetchConfig {
        // The constants below are all in range, so this construction is
        // infallible; the builder validates any caller-supplied override.
        FetchConfig {
            allow_http: false,
            allow_ports: DEFAULT_ALLOW_PORTS.to_vec(),
            allow_ip_literals: false,
            deny_extra: Vec::new(),
            allow_exact: Vec::new(),
            max_redirects: MaxRedirects(DEFAULT_MAX_REDIRECTS),
            max_bytes: MaxBytes(DEFAULT_MAX_BYTES),
            max_chars: MaxChars(DEFAULT_MAX_CHARS),
            connect_timeout: PositiveDuration(DEFAULT_CONNECT_TIMEOUT),
            timeout: PositiveDuration(DEFAULT_TIMEOUT),
            pool_idle_timeout: PositiveDuration(DEFAULT_POOL_IDLE_TIMEOUT),
            user_agent: UserAgent(DEFAULT_USER_AGENT.to_string()),
        }
    }
}

/// A fallible builder for [`FetchConfig`].
///
/// Every setter returns `self` for chaining and records a raw value; validation
/// happens once in [`FetchConfigBuilder::build`], which reports the first
/// offending field as a [`ConfigError`].
#[derive(Debug, Clone)]
pub struct FetchConfigBuilder {
    allow_http: bool,
    allow_ports: Vec<u16>,
    allow_ip_literals: bool,
    deny_cidrs: Vec<String>,
    allow_hosts: Vec<(String, IpAddr)>,
    max_redirects: usize,
    max_bytes: usize,
    max_chars: usize,
    connect_timeout: Duration,
    timeout: Duration,
    pool_idle_timeout: Duration,
    user_agent: String,
}

impl Default for FetchConfigBuilder {
    fn default() -> FetchConfigBuilder {
        FetchConfigBuilder {
            allow_http: false,
            allow_ports: DEFAULT_ALLOW_PORTS.to_vec(),
            allow_ip_literals: false,
            deny_cidrs: Vec::new(),
            allow_hosts: Vec::new(),
            max_redirects: DEFAULT_MAX_REDIRECTS,
            max_bytes: DEFAULT_MAX_BYTES,
            max_chars: DEFAULT_MAX_CHARS,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            timeout: DEFAULT_TIMEOUT,
            pool_idle_timeout: DEFAULT_POOL_IDLE_TIMEOUT,
            user_agent: DEFAULT_USER_AGENT.to_string(),
        }
    }
}

impl FetchConfigBuilder {
    /// Sets whether plain `http://` URLs are permitted.
    #[must_use]
    pub fn allow_http(mut self, yes: bool) -> FetchConfigBuilder {
        self.allow_http = yes;
        self
    }

    /// Replaces the set of ports a fetch may target.
    #[must_use]
    pub fn allow_ports(mut self, ports: impl IntoIterator<Item = u16>) -> FetchConfigBuilder {
        self.allow_ports = ports.into_iter().collect();
        self
    }

    /// Sets whether a bare IP-literal host is permitted.
    ///
    /// This grants literal *syntax* only: a permitted literal is still
    /// classified against the address policy, so a loopback, private,
    /// link-local, or otherwise non-global literal remains blocked.
    #[must_use]
    pub fn allow_ip_literals(mut self, yes: bool) -> FetchConfigBuilder {
        self.allow_ip_literals = yes;
        self
    }

    /// Adds a denied CIDR range, parsed from text by [`build`].
    ///
    /// [`build`]: FetchConfigBuilder::build
    #[must_use]
    pub fn deny_cidr(mut self, cidr: impl Into<String>) -> FetchConfigBuilder {
        self.deny_cidrs.push(cidr.into());
        self
    }

    /// Adds an exact host-plus-address escape hatch.
    ///
    /// The host is canonicalized and validated by [`build`]. This is the only
    /// supported way to reach an otherwise-blocked address.
    ///
    /// [`build`]: FetchConfigBuilder::build
    #[must_use]
    pub fn allow_host_address(
        mut self,
        host: impl Into<String>,
        addr: IpAddr,
    ) -> FetchConfigBuilder {
        self.allow_hosts.push((host.into(), addr));
        self
    }

    /// Sets the maximum number of redirect hops a single fetch may follow.
    #[must_use]
    pub fn max_redirects(mut self, n: usize) -> FetchConfigBuilder {
        self.max_redirects = n;
        self
    }

    /// Sets the largest response body accepted, in decompressed bytes.
    #[must_use]
    pub fn max_bytes(mut self, n: usize) -> FetchConfigBuilder {
        self.max_bytes = n;
        self
    }

    /// Sets the ceiling on returned text length, in characters.
    #[must_use]
    pub fn max_chars(mut self, n: usize) -> FetchConfigBuilder {
        self.max_chars = n;
        self
    }

    /// Sets the per-hop connect timeout.
    #[must_use]
    pub fn connect_timeout(mut self, d: Duration) -> FetchConfigBuilder {
        self.connect_timeout = d;
        self
    }

    /// Sets the whole-request timeout.
    #[must_use]
    pub fn timeout(mut self, d: Duration) -> FetchConfigBuilder {
        self.timeout = d;
        self
    }

    /// Sets the idle-connection pool timeout.
    #[must_use]
    pub fn pool_idle_timeout(mut self, d: Duration) -> FetchConfigBuilder {
        self.pool_idle_timeout = d;
        self
    }

    /// Sets the `User-Agent` header sent on every request.
    #[must_use]
    pub fn user_agent(mut self, ua: impl Into<String>) -> FetchConfigBuilder {
        self.user_agent = ua.into();
        self
    }

    /// Validates every field and produces an immutable [`FetchConfig`].
    ///
    /// # Errors
    /// Returns [`ConfigError`] for a header-invalid user agent, a zero or
    /// over-ceiling `max_bytes`/`max_chars`, an over-ceiling `max_redirects`, a
    /// zero timeout, a malformed denied CIDR, or a malformed exact host.
    ///
    /// # Examples
    /// ```
    /// use promptforge_webfetch::FetchConfig;
    ///
    /// let policy = FetchConfig::builder()
    ///     .deny_cidr("203.0.114.0/24")
    ///     .max_bytes(1024)
    ///     .build()?;
    /// # Ok::<(), promptforge_webfetch::ConfigError>(())
    /// ```
    pub fn build(self) -> Result<FetchConfig, ConfigError> {
        let user_agent = validate_user_agent(self.user_agent)?;
        let max_bytes = validate_limit("max_bytes", self.max_bytes, MAX_BYTES_CEILING)?;
        let max_chars = validate_limit("max_chars", self.max_chars, MAX_CHARS_CEILING)?;
        let max_redirects = validate_redirects(self.max_redirects)?;
        let connect_timeout =
            validate_timeout("connect_timeout", self.connect_timeout, MAX_CONNECT_TIMEOUT)?;
        let timeout = validate_timeout("timeout", self.timeout, MAX_TIMEOUT)?;
        let pool_idle_timeout = validate_timeout(
            "pool_idle_timeout",
            self.pool_idle_timeout,
            MAX_POOL_IDLE_TIMEOUT,
        )?;
        let deny_extra = validate_deny_cidrs(self.deny_cidrs)?;
        let allow_exact = validate_allow_hosts(self.allow_hosts)?;

        Ok(FetchConfig {
            allow_http: self.allow_http,
            allow_ports: self.allow_ports,
            allow_ip_literals: self.allow_ip_literals,
            deny_extra,
            allow_exact,
            max_redirects,
            max_bytes: MaxBytes(max_bytes),
            max_chars: MaxChars(max_chars),
            connect_timeout: PositiveDuration(connect_timeout),
            timeout: PositiveDuration(timeout),
            pool_idle_timeout: PositiveDuration(pool_idle_timeout),
            user_agent,
        })
    }
}

/// Validates a `User-Agent` string as a legal HTTP header value.
fn validate_user_agent(ua: String) -> Result<UserAgent, ConfigErrorRepr> {
    HeaderValue::from_str(&ua).map_err(ConfigErrorRepr::UserAgent)?;
    Ok(UserAgent(ua))
}

/// Validates a positive limit against a hard ceiling.
fn validate_limit(
    field: &'static str,
    value: usize,
    ceiling: usize,
) -> Result<usize, ConfigErrorRepr> {
    if value == 0 {
        return Err(ConfigErrorRepr::ZeroLimit { field });
    }
    if value > ceiling {
        return Err(ConfigErrorRepr::OverCeiling {
            field,
            value,
            ceiling,
        });
    }
    Ok(value)
}

/// Validates the redirect cap against its ceiling; zero is permitted.
fn validate_redirects(value: usize) -> Result<MaxRedirects, ConfigErrorRepr> {
    if value > MAX_REDIRECTS_CEILING {
        return Err(ConfigErrorRepr::OverCeiling {
            field: "max_redirects",
            value,
            ceiling: MAX_REDIRECTS_CEILING,
        });
    }
    Ok(MaxRedirects(value))
}

/// Validates a timeout as strictly positive and within its ceiling.
fn validate_timeout(
    field: &'static str,
    value: Duration,
    ceiling: Duration,
) -> Result<Duration, ConfigErrorRepr> {
    if value.is_zero() {
        return Err(ConfigErrorRepr::ZeroTimeout { field });
    }
    if value > ceiling {
        return Err(ConfigErrorRepr::TimeoutOverCeiling {
            field,
            value,
            ceiling,
        });
    }
    Ok(value)
}

/// Parses and validates the denied-CIDR strings into networks.
fn validate_deny_cidrs(cidrs: Vec<String>) -> Result<Vec<IpNet>, ConfigErrorRepr> {
    let mut nets = Vec::with_capacity(cidrs.len());
    for cidr in cidrs {
        let net = cidr
            .parse::<IpNet>()
            .map_err(|source| ConfigErrorRepr::Cidr {
                cidr: cidr.clone(),
                source,
            })?;
        if !nets.contains(&net) {
            nets.push(net);
        }
    }
    Ok(nets)
}

/// Canonicalizes and validates one exact-exception host.
///
/// Accepts an IP literal (for a literal-host exception) or a syntactically valid
/// DNS domain. Every other form - empty, whitespace-, slash-, colon-, at-, or
/// query-bearing - is rejected by the URL host parser, which enforces the URL
/// forbidden-host-code-point set. Returns the canonical host to store.
fn validate_host(raw: &str) -> Result<String, ConfigErrorRepr> {
    let host = canonical_host(raw);
    if host.is_empty() {
        return Err(ConfigErrorRepr::Host {
            host: raw.to_string(),
        });
    }
    // An IP literal is a legitimate exact-exception host (a literal-host URL).
    if host.parse::<IpAddr>().is_ok() {
        return Ok(host);
    }
    // Otherwise require a valid DNS domain. `url::Host::parse` rejects every
    // forbidden host code point (`:` outside brackets, `@`, `?`, `#`, `/`,
    // whitespace, ...), so `bad:host`, `bad@host`, and `bad?host` are refused,
    // while a non-domain address form is not a valid exact host here.
    match url::Host::parse(&host) {
        Ok(url::Host::Domain(domain)) => Ok(domain),
        _ => Err(ConfigErrorRepr::Host {
            host: raw.to_string(),
        }),
    }
}

/// Canonicalizes and validates the exact host-plus-address exceptions.
fn validate_allow_hosts(
    hosts: Vec<(String, IpAddr)>,
) -> Result<Vec<HostAddressException>, ConfigErrorRepr> {
    let mut out: Vec<HostAddressException> = Vec::with_capacity(hosts.len());
    for (raw, addr) in hosts {
        let host = validate_host(&raw)?;
        let entry = HostAddressException { host, addr };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::time::Duration;

    use super::{
        DEFAULT_MAX_BYTES, DEFAULT_MAX_CHARS, DEFAULT_MAX_REDIRECTS, FetchConfig,
        MAX_BYTES_CEILING, MAX_CHARS_CEILING, MAX_CONNECT_TIMEOUT, MAX_POOL_IDLE_TIMEOUT,
        MAX_REDIRECTS_CEILING, MAX_TIMEOUT,
    };

    #[test]
    fn default_policy_is_the_documented_safe_policy() {
        let cfg = FetchConfig::default();
        assert!(!cfg.allow_http());
        assert_eq!(cfg.allow_ports(), &[80, 443]);
        assert!(!cfg.allow_ip_literals());
        assert!(cfg.deny_extra().is_empty());
        assert!(cfg.allow_exact().is_empty());
        assert_eq!(cfg.max_redirects(), DEFAULT_MAX_REDIRECTS);
        assert_eq!(cfg.max_bytes(), DEFAULT_MAX_BYTES);
        assert_eq!(cfg.max_chars(), DEFAULT_MAX_CHARS);
        assert_eq!(cfg.connect_timeout(), Duration::from_secs(5));
        assert_eq!(cfg.timeout(), Duration::from_secs(20));
        assert_eq!(cfg.pool_idle_timeout(), Duration::from_secs(10));
        assert_eq!(cfg.user_agent(), "promptforge-webfetch/0.0");
    }

    #[test]
    fn builder_default_equals_default() {
        assert_eq!(
            FetchConfig::builder().build().expect("valid"),
            FetchConfig::default()
        );
    }

    #[test]
    fn rejects_newline_user_agent() {
        assert!(
            FetchConfig::builder()
                .user_agent("bad\r\nagent")
                .build()
                .is_err()
        );
        assert!(
            FetchConfig::builder()
                .user_agent("bad\nagent")
                .build()
                .is_err()
        );
    }

    #[test]
    fn rejects_zero_and_over_ceiling_limits() {
        assert!(FetchConfig::builder().max_bytes(0).build().is_err());
        assert!(FetchConfig::builder().max_chars(0).build().is_err());
        assert!(
            FetchConfig::builder()
                .max_bytes(MAX_BYTES_CEILING + 1)
                .build()
                .is_err()
        );
        assert!(
            FetchConfig::builder()
                .max_chars(MAX_CHARS_CEILING + 1)
                .build()
                .is_err()
        );
        assert!(
            FetchConfig::builder()
                .max_redirects(MAX_REDIRECTS_CEILING + 1)
                .build()
                .is_err()
        );
    }

    #[test]
    fn accepts_zero_redirects() {
        let cfg = FetchConfig::builder()
            .max_redirects(0)
            .build()
            .expect("zero redirects is valid");
        assert_eq!(cfg.max_redirects(), 0);
    }

    #[test]
    fn rejects_zero_timeouts() {
        assert!(
            FetchConfig::builder()
                .timeout(Duration::ZERO)
                .build()
                .is_err()
        );
        assert!(
            FetchConfig::builder()
                .connect_timeout(Duration::ZERO)
                .build()
                .is_err()
        );
        assert!(
            FetchConfig::builder()
                .pool_idle_timeout(Duration::ZERO)
                .build()
                .is_err()
        );
    }

    #[test]
    fn rejects_over_ceiling_timeouts() {
        assert!(
            FetchConfig::builder()
                .connect_timeout(MAX_CONNECT_TIMEOUT + Duration::from_secs(1))
                .build()
                .is_err(),
            "a connect timeout over its ceiling must be rejected"
        );
        assert!(
            FetchConfig::builder()
                .timeout(MAX_TIMEOUT + Duration::from_secs(1))
                .build()
                .is_err(),
            "a request timeout over its ceiling must be rejected"
        );
        assert!(
            FetchConfig::builder()
                .pool_idle_timeout(MAX_POOL_IDLE_TIMEOUT + Duration::from_secs(1))
                .build()
                .is_err(),
            "a pool-idle timeout over its ceiling must be rejected"
        );
        // Exactly the ceiling is accepted.
        assert!(
            FetchConfig::builder()
                .connect_timeout(MAX_CONNECT_TIMEOUT)
                .timeout(MAX_TIMEOUT)
                .pool_idle_timeout(MAX_POOL_IDLE_TIMEOUT)
                .build()
                .is_ok(),
            "exactly the ceiling must be accepted"
        );
    }

    #[test]
    fn rejects_malformed_cidr_and_host() {
        assert!(
            FetchConfig::builder()
                .deny_cidr("not-a-cidr")
                .build()
                .is_err()
        );
        let addr: IpAddr = "127.0.0.1".parse().expect("loopback parses");
        // Every non-domain, non-literal form is rejected by the DNS-host parser.
        for bad in [
            "", "bad host", "bad:host", "bad@host", "bad?host", "a/b", "x#y",
        ] {
            assert!(
                FetchConfig::builder()
                    .allow_host_address(bad, addr)
                    .build()
                    .is_err(),
                "malformed host {bad:?} must be rejected"
            );
        }
        // A valid domain and a valid IP literal are both accepted.
        let cfg = FetchConfig::builder()
            .allow_host_address("example.com", addr)
            .allow_host_address("127.0.0.1", addr)
            .build()
            .expect("a valid domain and IP literal are accepted");
        assert_eq!(cfg.allow_exact().len(), 2);
    }

    #[test]
    fn deduplicates_cidrs_and_hosts() {
        let addr: IpAddr = "127.0.0.1".parse().expect("loopback parses");
        let cfg = FetchConfig::builder()
            .deny_cidr("203.0.114.0/24")
            .deny_cidr("203.0.114.0/24")
            .allow_host_address("Localhost.", addr)
            .allow_host_address("localhost", addr)
            .build()
            .expect("valid");
        assert_eq!(cfg.deny_extra().len(), 1);
        assert_eq!(cfg.allow_exact().len(), 1);
        assert!(cfg.allow_exact()[0].matches("LOCALHOST", addr));
    }
}
