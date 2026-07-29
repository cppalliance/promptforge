//! Configuration for the `web_fetch` tool's security policy.
//!
//! [`FetchConfig`] carries the knobs that govern what a fetch may do. This step
//! reads the URL-policy fields (`allow_http`, `allow_ports`, `allow_ip_literals`)
//! plus the address-policy fields (`deny_extra`, `allow_exact`, `max_redirects`);
//! later steps extend the struct with size, content-type, and timeout settings.
//! Construct it with [`FetchConfig::default`] and override individual fields as
//! needed.

use std::net::IpAddr;

use ipnet::IpNet;

/// The default ports a fetch may target: HTTP and HTTPS.
const DEFAULT_ALLOW_PORTS: [u16; 2] = [80, 443];

/// The default cap on redirect hops a single fetch may follow.
const DEFAULT_MAX_REDIRECTS: usize = 5;

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
        }
    }
}
