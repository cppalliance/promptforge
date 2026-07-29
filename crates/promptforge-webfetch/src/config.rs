//! Configuration for the `web_fetch` tool's security policy.
//!
//! [`FetchConfig`] carries the knobs that govern what a fetch may do. This step
//! reads the URL-policy fields (`allow_http`, `allow_ports`, `allow_ip_literals`);
//! later steps extend the struct with address, size, content-type, and timeout
//! settings. Construct it with [`FetchConfig::default`] and override individual
//! fields as needed.

/// The default ports a fetch may target: HTTP and HTTPS.
const DEFAULT_ALLOW_PORTS: [u16; 2] = [80, 443];

/// Security policy for the `web_fetch` tool.
///
/// Built with [`FetchConfig::default`], whose values refuse plain HTTP, permit
/// only the standard web ports, and reject bare IP-literal hosts.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FetchConfig {
    /// Whether to permit `http://` URLs; `https://` is always allowed.
    pub allow_http: bool,
    /// The ports a fetch may target (matched against the URL's effective port).
    pub allow_ports: Vec<u16>,
    /// Whether to permit a host given as a bare IP literal.
    pub allow_ip_literals: bool,
}

impl Default for FetchConfig {
    fn default() -> FetchConfig {
        FetchConfig {
            allow_http: false,
            allow_ports: DEFAULT_ALLOW_PORTS.to_vec(),
            allow_ip_literals: false,
        }
    }
}
