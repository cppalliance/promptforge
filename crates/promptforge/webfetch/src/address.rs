//! The address-admission policy applied at DNS-resolution (connect) time.
//!
//! [`addr_allowed`] enforces the host-agnostic policy on a single resolved
//! [`IpAddr`]: it denies every address in the built-in blocked ranges (the
//! [`BLOCKED_CIDRS`] table below) and in the config's denied ranges, and permits
//! everything else. It never consults the exact-host exceptions, so an address
//! with no host context can never win a bypass.
//!
//! [`addr_allowed_for_host`] is the host-aware admission check the resolver
//! runs. It permits what the general policy permits and, in addition, admits a
//! blocked address only when the pair `(host, ip)` appears in the config's exact
//! exceptions. Keying the escape hatch on both the host and the address means a
//! name that resolves inward (for example a DNS-rebinding answer of
//! `evil.com -> 127.0.0.1`) does not inherit another host's exception.
//!
//! Every classified address is first normalized: an IPv4-embedded IPv6 form
//! (IPv4-mapped `::ffff:a.b.c.d` or IPv4-compatible `::a.b.c.d`) is reduced to
//! its embedded IPv4 value and reclassified, so a non-global IPv4 wearing an
//! IPv6 hat cannot slip past the table.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::LazyLock;

use ipnet::IpNet;

use crate::config::FetchConfig;

/// The built-in blocked CIDR ranges, IPv4 then IPv6.
///
/// These are the destinations a fetch must never reach: every address that is
/// not globally reachable under the pinned IANA special-purpose registry
/// snapshot (2025), covering this-network, private, shared/CGNAT, loopback,
/// link-local, protocol-assignment, documentation, benchmarking, multicast, and
/// reserved ranges. The config's denied ranges add deployment ranges on top of
/// it, and an exact host-plus-address exception carves a single address back
/// out.
pub(crate) const BLOCKED_CIDRS: &[&str] = &[
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
    "::/96",          // IPv4-compatible (deprecated, RFC 5156)
    "::ffff:0:0/96",  // IPv4-mapped (loopback and RFC1918 in a v6 hat)
    "64:ff9b::/96",   // NAT64
    "64:ff9b:1::/48", // NAT64
    "100::/64",       // discard-only
    "2001:db8::/32",  // documentation
    "2002::/16",      // 6to4
    "3fff::/20",      // documentation (RFC 9637)
    "fc00::/7",       // unique local
    "fe80::/10",      // link-local
    "fec0::/10",      // deprecated site-local (RFC 3879)
    "ff00::/8",       // multicast
];

/// The parsed form of [`BLOCKED_CIDRS`], built once on first use.
#[expect(
    clippy::expect_used,
    reason = "every entry in BLOCKED_CIDRS is a compile-time CIDR literal, so parsing cannot fail at runtime"
)]
static BLOCKED_NETS: LazyLock<Vec<IpNet>> = LazyLock::new(|| {
    BLOCKED_CIDRS
        .iter()
        .map(|cidr| {
            cidr.parse::<IpNet>()
                .expect("every entry in BLOCKED_CIDRS is a valid CIDR literal")
        })
        .collect()
});

/// Returns the IPv4 value embedded in an IPv4-mapped or IPv4-compatible IPv6
/// address, or `None` for any other address.
///
/// `Ipv6Addr::to_ipv4` covers both `::ffff:a.b.c.d` (mapped) and `::a.b.c.d`
/// (compatible), the two forms that re-encode an IPv4 destination.
fn embedded_ipv4(ip: IpAddr) -> Option<Ipv4Addr> {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4(),
        IpAddr::V4(_) => None,
    }
}

/// Returns the built-in range that blocks `ip`, or `None` if none does.
///
/// The address is checked directly, then, if it re-encodes an IPv4 address, its
/// embedded IPv4 value is checked too, so a non-global IPv4 in an IPv6 hat is
/// caught even where the table has no matching IPv6 entry.
fn builtin_blocked(ip: IpAddr) -> Option<String> {
    if let Some(net) = BLOCKED_NETS.iter().find(|net| net.contains(&ip)) {
        return Some(net.to_string());
    }
    if let Some(v4) = embedded_ipv4(ip) {
        let v4 = IpAddr::V4(v4);
        if let Some(net) = BLOCKED_NETS.iter().find(|net| net.contains(&v4)) {
            return Some(format!("{net} (embedded ipv4 {v4})"));
        }
    }
    None
}

/// Returns the CIDR that blocks `ip`, or `None` if no range blocks it.
///
/// A built-in range is reported by its CIDR text; a config-supplied denied range
/// is prefixed with `deny_extra ` so a log reader can tell the source. This does
/// not consult the exact-host exceptions; it reports only what would block the
/// address, for the log rendering of a dropped address.
#[must_use]
pub(crate) fn blocked_range(ip: IpAddr, config: &FetchConfig) -> Option<String> {
    if let Some(range) = builtin_blocked(ip) {
        return Some(range);
    }
    if let Some(net) = config.deny_extra().iter().find(|net| net.contains(&ip)) {
        return Some(format!("deny_extra {net}"));
    }
    if let Some(v4) = embedded_ipv4(ip) {
        let v4 = IpAddr::V4(v4);
        if let Some(net) = config.deny_extra().iter().find(|net| net.contains(&v4)) {
            return Some(format!("deny_extra {net} (embedded ipv4 {v4})"));
        }
    }
    None
}

/// Returns whether a fetch may connect to `ip` under the host-agnostic policy.
///
/// The address is allowed only when no built-in blocked range and no
/// config-supplied denied range covers it (directly or through an embedded IPv4
/// value). This function is host-agnostic and never consults the exact-host
/// exceptions: an address with no host context gets no exception. Use
/// [`addr_allowed_for_host`] to apply the host-keyed escape hatch.
#[must_use]
pub(crate) fn addr_allowed(ip: IpAddr, config: &FetchConfig) -> bool {
    blocked_range(ip, config).is_none()
}

/// Returns whether a fetch to `host` may connect to `ip` under `config`.
///
/// The pair is allowed when the host-agnostic policy [`addr_allowed`] permits
/// `ip`, and additionally when `(host, ip)` matches an exact host-plus-address
/// exception - the explicit escape hatch that overrides a block only for the
/// exact host that named the address. A blocked address whose host does not
/// match its exception stays blocked, so a rebinding host cannot inherit another
/// host's exception.
#[must_use]
pub(crate) fn addr_allowed_for_host(host: &str, ip: IpAddr, config: &FetchConfig) -> bool {
    if config
        .allow_exact()
        .iter()
        .any(|entry| entry.matches(host, ip))
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

    /// The canonical classification matrix: the first and last address of every
    /// blocked range is denied, embedded-IPv4 (mapped and compatible) forms are
    /// denied by their embedded value, and ordinary global unicast - including a
    /// global address adjacent to a blocked range - is allowed.
    #[test]
    fn classification_matrix() {
        let cfg = FetchConfig::default();

        // (range label, first address, last address): both boundaries denied.
        let boundaries: &[(&str, &str, &str)] = &[
            ("0.0.0.0/8", "0.0.0.0", "0.255.255.255"),
            ("10.0.0.0/8", "10.0.0.0", "10.255.255.255"),
            ("100.64.0.0/10", "100.64.0.0", "100.127.255.255"),
            ("127.0.0.0/8", "127.0.0.0", "127.255.255.255"),
            ("169.254.0.0/16", "169.254.0.0", "169.254.255.255"),
            ("172.16.0.0/12", "172.16.0.0", "172.31.255.255"),
            ("192.0.0.0/24", "192.0.0.0", "192.0.0.255"),
            ("192.0.2.0/24", "192.0.2.0", "192.0.2.255"),
            ("192.88.99.0/24", "192.88.99.0", "192.88.99.255"),
            ("192.168.0.0/16", "192.168.0.0", "192.168.255.255"),
            ("198.18.0.0/15", "198.18.0.0", "198.19.255.255"),
            ("198.51.100.0/24", "198.51.100.0", "198.51.100.255"),
            ("203.0.113.0/24", "203.0.113.0", "203.0.113.255"),
            ("224.0.0.0/4", "224.0.0.0", "239.255.255.255"),
            ("240.0.0.0/4", "240.0.0.0", "255.255.255.254"),
            ("255.255.255.255/32", "255.255.255.255", "255.255.255.255"),
            ("::/128", "::", "::"),
            ("::1/128", "::1", "::1"),
            ("::/96", "::0.0.0.1", "::255.255.255.255"),
            ("::ffff:0:0/96", "::ffff:0.0.0.0", "::ffff:255.255.255.255"),
            ("64:ff9b::/96", "64:ff9b::", "64:ff9b::ffff:ffff"),
            (
                "64:ff9b:1::/48",
                "64:ff9b:1::",
                "64:ff9b:1:ffff:ffff:ffff:ffff:ffff",
            ),
            ("100::/64", "100::", "100::ffff:ffff:ffff:ffff"),
            (
                "2001:db8::/32",
                "2001:db8::",
                "2001:db8:ffff:ffff:ffff:ffff:ffff:ffff",
            ),
            (
                "2002::/16",
                "2002::",
                "2002:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            ),
            (
                "3fff::/20",
                "3fff::",
                "3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff",
            ),
            (
                "fc00::/7",
                "fc00::",
                "fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            ),
            (
                "fe80::/10",
                "fe80::",
                "febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            ),
            (
                "fec0::/10",
                "fec0::",
                "feff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            ),
            (
                "ff00::/8",
                "ff00::",
                "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            ),
        ];
        for (range, first, last) in boundaries {
            assert!(
                !addr_allowed(ip(first), &cfg),
                "{first} (first of {range}) must be denied"
            );
            assert!(
                !addr_allowed(ip(last), &cfg),
                "{last} (last of {range}) must be denied"
            );
        }
    }

    /// Embedded-IPv4 forms are denied by their embedded value, and ordinary
    /// global unicast (including addresses adjacent to a blocked range) is
    /// allowed so the table does not over-block.
    #[test]
    fn classification_embedded_and_allowed() {
        let cfg = FetchConfig::default();

        // Embedded-IPv4 forms are denied by their embedded value: mapped and
        // compatible, across loopback, private, link-local, and public.
        let embedded_blocked = [
            "::127.0.0.1",            // compatible loopback
            "::10.0.0.1",             // compatible private
            "::169.254.169.254",      // compatible link-local
            "::1.1.1.1",              // compatible public (blocked wholesale by ::/96)
            "::ffff:127.0.0.1",       // mapped loopback
            "::ffff:10.0.0.1",        // mapped private
            "::ffff:169.254.169.254", // mapped link-local
            "::ffff:1.1.1.1",         // mapped public (blocked wholesale by ::ffff:0:0/96)
            "64:ff9b::7f00:1",        // NAT64-embedded loopback
        ];
        for addr in embedded_blocked {
            assert!(!addr_allowed(ip(addr), &cfg), "{addr} must be denied");
        }

        // Ordinary global unicast is allowed, including addresses one step
        // outside a blocked range so the table does not over-block.
        let allowed = [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "11.0.0.0",        // just past 10.0.0.0/8
            "172.32.0.0",      // just past 172.16.0.0/12
            "223.255.255.255", // just below 224.0.0.0/4
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "2001:db9::1",  // just past 2001:db8::/32
            "3fff:1000::1", // just past 3fff::/20, still global unicast
        ];
        for addr in allowed {
            assert!(addr_allowed(ip(addr), &cfg), "{addr} must be allowed");
        }
    }

    #[test]
    fn allow_exact_admits_only_the_named_host() {
        let target = ip("127.0.0.1");
        let cfg = FetchConfig::builder()
            .allow_host_address("localhost", target)
            .build()
            .expect("valid config");
        assert!(
            addr_allowed_for_host("localhost", target, &cfg),
            "the exact (host, addr) pair must be admitted"
        );
        assert!(
            !addr_allowed_for_host("evil.com", target, &cfg),
            "a different host must not inherit the allow_exact exception"
        );
        assert!(
            !addr_allowed(target, &cfg),
            "addr_allowed alone must grant no allow_exact bypass"
        );
        assert!(
            !addr_allowed_for_host("localhost", ip("127.0.0.2"), &cfg),
            "allow_exact must not widen to the rest of the range"
        );
    }

    #[test]
    fn deny_extra_blocks_an_otherwise_public_address() {
        let cfg = FetchConfig::builder()
            .deny_cidr("203.0.114.0/24")
            .build()
            .expect("valid config");
        assert!(
            !addr_allowed(ip("203.0.114.5"), &cfg),
            "a deny_extra range must block an otherwise-public address"
        );
    }
}
