//! Configuration for the `web_fetch` tool's security policy.
//!
//! [`FetchConfig`] carries the knobs that govern what a fetch may do: the
//! URL-policy fields (`allow_http`, `allow_ports`, `allow_ip_literals`), the
//! address-policy fields (`deny_extra`, `allow_exact`, `max_redirects`), the
//! size caps (`max_bytes`, `max_chars`), the timeouts (`connect_timeout`,
//! `timeout`, `pool_idle_timeout`), and the `user_agent`. The set of accepted
//! content types is fixed rather than configured.
//! Construct it with [`FetchConfig::default`] and override individual fields as
//! needed.

use std::net::IpAddr;
use std::time::Duration;

use ipnet::IpNet;

/// The default ports a fetch may target: HTTP and HTTPS.
const DEFAULT_ALLOW_PORTS: [u16; 2] = [80, 443];

/// The default cap on redirect hops a single fetch may follow.
const DEFAULT_MAX_REDIRECTS: usize = 5;

/// The default time allowed to establish a TCP connection.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The default cap on the total time a single request may take.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// The default time an idle connection is kept in the pool before it is closed.
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The default `User-Agent` header sent on every request.
const DEFAULT_USER_AGENT: &str = "promptforge-webfetch/0.1";

/// The default cap on a response body's decompressed size, in bytes (8 MiB).
const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The default cap on the returned text length, in characters.
const DEFAULT_MAX_CHARS: usize = 40_000;

/// Security policy for the `web_fetch` tool.
///
/// Built with [`FetchConfig::default`], whose values refuse plain HTTP, permit
/// only the standard web ports, reject bare IP-literal hosts, add no extra
/// denied ranges, allow no internal address, and cap redirects at five.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FetchConfig {
    /// Whether to permit `http://` URLs; `https://` is always allowed.
    pub allow_http: bool,
    /// The ports a fetch may target (matched against the URL's effective port).
    pub allow_ports: Vec<u16>,
    /// Whether to permit a host given as a bare IP literal.
    pub allow_ip_literals: bool,
    /// Extra CIDR ranges to deny, on top of the built-in blocked ranges.
    ///
    /// A deployment adds its own internal ranges here. Empty by default.
    pub deny_extra: Vec<IpNet>,
    /// Exact host-plus-address pairs allowed even when otherwise blocked.
    ///
    /// Each entry is a host and one exact [`IpAddr`], never a range. This is the
    /// only supported way to reach an internal host. Empty by default.
    pub allow_exact: Vec<(String, IpAddr)>,
    /// The maximum number of redirect hops a single fetch may follow.
    pub max_redirects: usize,
    /// The largest response body accepted, counted on decompressed bytes.
    ///
    /// A structured response larger than this is refused outright; the counter
    /// runs over the decompressed stream, so a compressed payload that expands
    /// past the cap is refused on its expanded size, not its wire size.
    pub max_bytes: usize,
    /// The default cap on the returned text length, in characters.
    ///
    /// A per-call `max_chars` input overrides this for a single fetch. Text
    /// longer than the effective cap is truncated on a character boundary and
    /// the return is flagged as truncated.
    pub max_chars: usize,
    /// The time allowed to establish a TCP connection on any hop.
    ///
    /// Applied as [`reqwest::ClientBuilder::connect_timeout`].
    pub connect_timeout: Duration,
    /// The cap on the total time a single request may take, connect through
    /// body.
    ///
    /// Applied as [`reqwest::ClientBuilder::timeout`]. A request that exceeds it
    /// is aborted and surfaces as [`FetchError::Timeout`].
    ///
    /// [`FetchError::Timeout`]: crate::error::FetchError::Timeout
    pub timeout: Duration,
    /// How long an idle pooled connection is kept before it is closed.
    ///
    /// Applied as [`reqwest::ClientBuilder::pool_idle_timeout`]. A short value
    /// bounds the DNS-rebinding window left by connection reuse, since a
    /// kept-alive socket is not re-resolved.
    pub pool_idle_timeout: Duration,
    /// The `User-Agent` header sent on every request.
    ///
    /// Set with [`reqwest::ClientBuilder::user_agent`]. No cookie store is
    /// installed and no credential or `Authorization` header is sent by
    /// default, so the client carries no ambient identity beyond this string.
    pub user_agent: String,
}

impl Default for FetchConfig {
    fn default() -> FetchConfig {
        FetchConfig {
            allow_http: false,
            allow_ports: DEFAULT_ALLOW_PORTS.to_vec(),
            allow_ip_literals: false,
            deny_extra: Vec::new(),
            allow_exact: Vec::new(),
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
