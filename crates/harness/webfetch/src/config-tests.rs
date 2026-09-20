use std::net::IpAddr;
use std::time::Duration;

use super::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_CHARS, DEFAULT_MAX_REDIRECTS, FetchConfig, MAX_BYTES_CEILING,
    MAX_CHARS_CEILING, MAX_CONNECT_TIMEOUT, MAX_POOL_IDLE_TIMEOUT, MAX_REDIRECTS_CEILING,
    MAX_TIMEOUT,
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
    assert_eq!(cfg.user_agent(), "harness-webfetch/0.0");
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
