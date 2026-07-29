//! The address-admission policy applied at DNS-resolution (connect) time.
//!
//! [`addr_allowed`] enforces the host-agnostic policy on a single resolved
//! [`IpAddr`]: it denies every address in the built-in blocked ranges (the
//! [`BLOCKED_CIDRS`] table below) and in [`FetchConfig::deny_extra`], and
//! permits everything else. It never consults [`FetchConfig::allow_exact`], so
//! an address with no host context can never win a bypass.
//!
//! [`addr_allowed_for_host`] is the host-aware admission check the resolver
//! runs. It permits what the general policy permits and, in addition, admits a
//! blocked address only when the pair `(host, ip)` appears verbatim in
//! [`FetchConfig::allow_exact`]. Keying the escape hatch on both the host and
//! the address means a name that resolves inward (for example a DNS-rebinding
//! answer of `evil.com -> 127.0.0.1`) does not inherit another host's exception.
//! The check runs on the addresses a host resolves to, not on the URL string,
//! so a name that resolves inward and a rebinding answer are both caught.

use std::net::IpAddr;
use std::sync::LazyLock;

use ipnet::IpNet;

use crate::config::FetchConfig;

/// The built-in blocked CIDR ranges, IPv4 then IPv6.
///
/// These are private, loopback, link-local, documentation, benchmarking,
/// multicast, and reserved ranges that a fetch must never reach. The table is
/// the authoritative list; [`FetchConfig::deny_extra`] adds deployment ranges
/// on top of it, and [`FetchConfig::allow_exact`] carves a single address back
/// out.
pub const BLOCKED_CIDRS: &[&str] = &[
    // IPv4
    "0.0.0.0/8",          // this network and the unspecified address
    "10.0.0.0/8",         // RFC1918 private
    "100.64.0.0/10",      // CGNAT
    "127.0.0.0/8",        // loopback (the whole block)
    "169.254.0.0/16",     // link-local, includes cloud metadata 169.254.169.254
    "172.16.0.0/12",      // RFC1918 private
    "192.0.0.0/24",       // IETF protocol assignments
    "192.0.2.0/24",       // TEST-NET-1
    "192.88.99.0/24",     // 6to4 relay anycast
    "192.168.0.0/16",     // RFC1918 private
    "198.18.0.0/15",      // benchmarking
    "198.51.100.0/24",    // TEST-NET-2
    "203.0.113.0/24",     // TEST-NET-3
    "224.0.0.0/4",        // multicast
    "240.0.0.0/4",        // reserved
    "255.255.255.255/32", // broadcast
    // IPv6
    "::/128",         // unspecified
    "::1/128",        // loopback
    "::ffff:0:0/96",  // IPv4-mapped (loopback and RFC1918 in a v6 hat)
    "64:ff9b::/96",   // NAT64
    "64:ff9b:1::/48", // NAT64
    "100::/64",       // discard-only
    "2001:db8::/32",  // documentation
    "2002::/16",      // 6to4
    "fc00::/7",       // unique local
    "fe80::/10",      // link-local
    "ff00::/8",       // multicast
];

/// The parsed form of [`BLOCKED_CIDRS`], built once on first use.
static BLOCKED_NETS: LazyLock<Vec<IpNet>> = LazyLock::new(|| {
    BLOCKED_CIDRS
        .iter()
        .map(|cidr| {
            cidr.parse::<IpNet>()
                .expect("every entry in BLOCKED_CIDRS is a valid CIDR literal")
        })
        .collect()
});

/// Returns the CIDR that blocks `ip`, or `None` if no range blocks it.
///
/// A built-in range is reported by its CIDR text; a [`FetchConfig::deny_extra`]
/// range is prefixed with `deny_extra ` so a log reader can tell the source.
/// This does not consult [`FetchConfig::allow_exact`]; it reports only what
/// would block the address, for the log rendering of a dropped address.
#[must_use]
pub(crate) fn blocked_range(ip: IpAddr, config: &FetchConfig) -> Option<String> {
    if let Some(net) = BLOCKED_NETS.iter().find(|net| net.contains(&ip)) {
        return Some(net.to_string());
    }
    if let Some(net) = config.deny_extra.iter().find(|net| net.contains(&ip)) {
        return Some(format!("deny_extra {net}"));
    }
    None
}

/// Returns whether a fetch may connect to `ip` under the host-agnostic policy.
///
/// The address is allowed only when no built-in blocked range and no
/// [`FetchConfig::deny_extra`] range covers it. This function is host-agnostic
/// and never consults [`FetchConfig::allow_exact`]: an address with no host
/// context gets no exception. Use [`addr_allowed_for_host`] to apply the
/// host-keyed `allow_exact` escape hatch.
#[must_use]
pub fn addr_allowed(ip: IpAddr, config: &FetchConfig) -> bool {
    blocked_range(ip, config).is_none()
}

/// Returns whether a fetch to `host` may connect to `ip` under `config`.
///
/// The pair is allowed when the host-agnostic policy [`addr_allowed`] permits
/// `ip`, and additionally when `(host, ip)` appears verbatim in
/// [`FetchConfig::allow_exact`] - the explicit escape hatch that overrides a
/// block only for the exact host that named the address. A blocked address
/// whose host does not match its `allow_exact` entry stays blocked, so a
/// rebinding host cannot inherit another host's exception.
#[must_use]
pub fn addr_allowed_for_host(host: &str, ip: IpAddr, config: &FetchConfig) -> bool {
    if config
        .allow_exact
        .iter()
        .any(|(allowed_host, allowed_ip)| allowed_host == host && *allowed_ip == ip)
    {
        return true;
    }
    addr_allowed(ip, config)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{BLOCKED_CIDRS, BLOCKED_NETS, addr_allowed, addr_allowed_for_host};
    use crate::config::FetchConfig;

    /// Parses `s` into an [`IpAddr`], panicking with context on failure.
    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test address literal must parse")
    }

    #[test]
    fn every_blocked_cidr_parses() {
        assert_eq!(
            BLOCKED_NETS.len(),
            BLOCKED_CIDRS.len(),
            "all blocked CIDR literals must parse"
        );
    }

    /// For each range, an address just inside is blocked and one just outside
    /// is permitted. Each `outside` value is chosen to fall in no blocked range.
    #[test]
    fn addr_table_inside_blocked_outside_allowed() {
        let cfg = FetchConfig::default();

        // (range, inside -> blocked, outside -> allowed).
        let cases: &[(&str, &str, &str)] = &[
            ("0.0.0.0/8", "0.0.0.1", "1.0.0.1"),
            ("10.0.0.0/8", "10.255.255.255", "11.0.0.1"),
            ("100.64.0.0/10", "100.64.0.0", "100.63.255.255"),
            ("127.0.0.0/8", "127.0.0.1", "126.255.255.255"),
            ("169.254.0.0/16", "169.254.169.254", "169.253.255.255"),
            ("172.16.0.0/12", "172.31.255.255", "172.32.0.1"),
            ("192.0.0.0/24", "192.0.0.1", "191.255.255.255"),
            ("192.0.2.0/24", "192.0.2.1", "192.0.3.1"),
            ("192.88.99.0/24", "192.88.99.1", "192.88.100.1"),
            ("192.168.0.0/16", "192.168.1.1", "192.169.0.1"),
            ("198.18.0.0/15", "198.19.255.255", "198.20.0.1"),
            ("198.51.100.0/24", "198.51.100.1", "198.51.101.1"),
            ("203.0.113.0/24", "203.0.113.1", "203.0.114.1"),
            // 224/4 (multicast) and 240/4 (reserved) are adjacent up to
            // 255.255.255.255; the only address below both is < 224.
            ("224.0.0.0/4", "239.255.255.255", "223.255.255.255"),
            ("240.0.0.0/4", "240.0.0.1", "223.255.255.255"),
            ("255.255.255.255/32", "255.255.255.255", "223.255.255.255"),
            // IPv6
            ("::/128", "::", "2606:4700:4700::1111"),
            ("::1/128", "::1", "2001:4860:4860::8888"),
            ("64:ff9b::/96", "64:ff9b::1", "64:ff9b:2::1"),
            ("64:ff9b:1::/48", "64:ff9b:1::1", "64:ff9c::1"),
            ("100::/64", "100::1", "100:0:0:1::1"),
            ("2001:db8::/32", "2001:db8::1", "2001:db9::1"),
            ("2002::/16", "2002::1", "2003::1"),
            ("fc00::/7", "fdff::1", "2001:4860::1"),
            ("fe80::/10", "febf::1", "2606:4700::1"),
            ("ff00::/8", "ffff::1", "2607:f8b0::1"),
        ];

        for (range, inside, outside) in cases {
            assert!(
                !addr_allowed(ip(inside), &cfg),
                "{inside} is inside {range} and must be denied"
            );
            assert!(
                addr_allowed(ip(outside), &cfg),
                "{outside} is outside {range} and must be allowed"
            );
        }
    }

    #[test]
    fn ipv4_mapped_loopback_is_blocked() {
        let cfg = FetchConfig::default();
        assert!(
            !addr_allowed(ip("::ffff:127.0.0.1"), &cfg),
            "an IPv4-mapped loopback must be blocked by ::ffff:0:0/96"
        );
    }

    #[test]
    fn nat64_address_is_blocked() {
        let cfg = FetchConfig::default();
        assert!(
            !addr_allowed(ip("64:ff9b::7f00:1"), &cfg),
            "a NAT64-embedded address must be blocked by 64:ff9b::/96"
        );
    }

    #[test]
    fn cloud_metadata_address_is_blocked() {
        let cfg = FetchConfig::default();
        assert!(
            !addr_allowed(ip("169.254.169.254"), &cfg),
            "the cloud metadata address must be blocked by 169.254.0.0/16"
        );
    }

    #[test]
    fn allow_exact_admits_only_the_named_host() {
        let target = ip("127.0.0.1");
        let cfg = FetchConfig {
            allow_exact: vec![("localhost".to_string(), target)],
            ..FetchConfig::default()
        };
        // The exact (host, addr) pair is admitted.
        assert!(
            addr_allowed_for_host("localhost", target, &cfg),
            "the exact (host, addr) pair must be admitted"
        );
        // The same address under a different host is refused: a rebinding host
        // must not inherit another host's allow_exact entry.
        assert!(
            !addr_allowed_for_host("evil.com", target, &cfg),
            "a different host must not inherit the allow_exact exception"
        );
        // The host-agnostic policy never honors allow_exact.
        assert!(
            !addr_allowed(target, &cfg),
            "addr_allowed alone must grant no allow_exact bypass"
        );
        // A different blocked address is still denied even for the named host.
        assert!(
            !addr_allowed_for_host("localhost", ip("127.0.0.2"), &cfg),
            "allow_exact must not widen to the rest of the range"
        );
    }

    #[test]
    fn deny_extra_blocks_an_otherwise_public_address() {
        let cfg = FetchConfig {
            deny_extra: vec!["203.0.114.0/24".parse().expect("valid cidr")],
            ..FetchConfig::default()
        };
        assert!(
            !addr_allowed(ip("203.0.114.5"), &cfg),
            "a deny_extra range must block an otherwise-public address"
        );
    }
}
