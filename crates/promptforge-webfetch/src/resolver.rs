//! A DNS resolver that filters answers through the address policy.
//!
//! [`GuardedResolver`] implements reqwest's [`Resolve`] trait, so it runs at
//! connect time on every hop. It resolves a host, then keeps only the addresses
//! [`addr_allowed_for_host`] permits and hands those to reqwest. It filters
//! rather than rejecting on the first blocked answer, so a host that returns one
//! public and one private address is still reachable at its public address,
//! while a host that returns only blocked addresses fails with
//! [`FetchError::NoAllowedAddress`]. Because it re-resolves on every call and
//! caches no verdict, a DNS-rebinding answer is caught on the hop that returns
//! it.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::address::{addr_allowed_for_host, blocked_range};
use crate::config::FetchConfig;
use crate::error::FetchError;

/// The boxed error type reqwest's resolver contract returns.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The future a [`Lookup`] returns: resolved addresses or an I/O error.
pub(crate) type LookupFuture =
    Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>>;

/// Resolves a host name to a set of socket addresses.
///
/// This is the seam the [`GuardedResolver`] filters. The production
/// implementation is [`SystemLookup`]; tests substitute a stub so the filtering
/// and no-caching behavior can be checked without real DNS.
pub(crate) trait Lookup: Send + Sync + 'static {
    /// Resolves `host` to zero or more socket addresses.
    ///
    /// The port carried by each address is irrelevant: reqwest overrides it with
    /// the URL's port. Implementations resolve `host` with any placeholder port.
    fn lookup(&self, host: String) -> LookupFuture;
}

/// The production [`Lookup`], backed by [`tokio::net::lookup_host`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemLookup;

impl Lookup for SystemLookup {
    fn lookup(&self, host: String) -> LookupFuture {
        Box::pin(async move {
            let addrs = tokio::net::lookup_host(format!("{host}:0")).await?;
            Ok(addrs.collect())
        })
    }
}

/// A guarded DNS resolver that filters answers through [`addr_allowed_for_host`].
///
/// Construct the production resolver with [`GuardedResolver::system`], or wrap
/// any [`Lookup`] with [`GuardedResolver::new`]. The policy is held behind an
/// [`Arc`] so each resolving future clones only a pointer, not the whole config.
#[derive(Debug, Clone)]
pub(crate) struct GuardedResolver<L = SystemLookup> {
    /// The underlying host-to-address lookup.
    inner: L,
    /// The address policy applied to each resolved address.
    config: Arc<FetchConfig>,
}

impl GuardedResolver<SystemLookup> {
    /// Builds a guarded resolver over the system resolver.
    #[must_use]
    pub(crate) fn system(config: Arc<FetchConfig>) -> GuardedResolver<SystemLookup> {
        GuardedResolver {
            inner: SystemLookup,
            config,
        }
    }
}

impl<L: Lookup> GuardedResolver<L> {
    /// Builds a guarded resolver over `inner` with the address policy `config`.
    ///
    /// The custom-lookup form is an internal test seam.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn new(inner: L, config: Arc<FetchConfig>) -> GuardedResolver<L> {
        GuardedResolver { inner, config }
    }

    /// Filters `addrs` for `host` through the address policy.
    ///
    /// Returns the allowed addresses, logging each dropped address at debug
    /// level with its blocking range. Returns [`FetchError::NoAllowedAddress`]
    /// when nothing survives.
    fn filter(
        host: &str,
        addrs: Vec<SocketAddr>,
        config: &FetchConfig,
    ) -> Result<Vec<SocketAddr>, FetchError> {
        let mut allowed = Vec::new();
        for sa in addrs {
            let ip = sa.ip();
            if addr_allowed_for_host(host, ip, config) {
                allowed.push(sa);
            } else if let Some(range) = blocked_range(ip, config) {
                // Full detail goes to the log; the model never sees it.
                tracing::debug!(
                    "{}",
                    FetchError::BlockedAddress {
                        host: host.to_string(),
                        addr: ip,
                        range,
                    }
                );
            }
        }
        if allowed.is_empty() {
            return Err(FetchError::NoAllowedAddress {
                host: host.to_string(),
            });
        }
        Ok(allowed)
    }
}

impl<L: Lookup> Resolve for GuardedResolver<L> {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let fut = self.inner.lookup(host.clone());
        let config = Arc::clone(&self.config);
        Box::pin(async move {
            let addrs = fut.await.map_err(|source| -> BoxError {
                Box::new(FetchError::Dns {
                    host: host.clone(),
                    source,
                })
            })?;
            let allowed = GuardedResolver::<L>::filter(&host, addrs, &config)
                .map_err(|err| -> BoxError { Box::new(err) })?;
            let iter: Addrs = Box::new(allowed.into_iter());
            Ok(iter)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use reqwest::dns::{Name, Resolve};

    use super::{GuardedResolver, Lookup, LookupFuture, SystemLookup};
    use crate::config::FetchConfig;
    use crate::error::FetchError;

    /// The exported resolver types must stay `Send + Sync` for reqwest.
    const _: fn() = || {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GuardedResolver<SystemLookup>>();
        assert_send_sync::<SystemLookup>();
    };

    /// A stub [`Lookup`] that hands back queued answers, one per call.
    struct StubLookup {
        answers: Mutex<VecDeque<Vec<SocketAddr>>>,
    }

    impl StubLookup {
        fn new(answers: impl IntoIterator<Item = Vec<SocketAddr>>) -> StubLookup {
            StubLookup {
                answers: Mutex::new(answers.into_iter().collect()),
            }
        }
    }

    impl Lookup for StubLookup {
        fn lookup(&self, _host: String) -> LookupFuture {
            let next = self
                .answers
                .lock()
                .expect("stub lookup mutex poisoned")
                .pop_front()
                .unwrap_or_default();
            Box::pin(async move { Ok(next) })
        }
    }

    /// Parses `s` into a [`SocketAddr`], panicking with context on failure.
    fn sa(s: &str) -> SocketAddr {
        s.parse().expect("test socket address must parse")
    }

    /// Builds a resolver [`Name`] from `host`.
    fn name(host: &str) -> Name {
        host.parse().expect("test host must be a valid dns name")
    }

    /// Wraps `config` in an [`Arc`] for a [`GuardedResolver`].
    fn shared(config: FetchConfig) -> Arc<FetchConfig> {
        Arc::new(config)
    }

    /// Resolves `resolver` for `host`, returning the address list or the error.
    async fn resolve_once<L: Lookup>(
        resolver: &GuardedResolver<L>,
        host: &str,
    ) -> Result<Vec<SocketAddr>, Box<dyn std::error::Error + Send + Sync>> {
        let addrs = resolver.resolve(name(host)).await?;
        Ok(addrs.collect())
    }

    #[tokio::test]
    async fn public_then_loopback_succeeds_then_fails_no_caching() {
        let stub = StubLookup::new([vec![sa("93.184.216.34:0")], vec![sa("127.0.0.1:0")]]);
        let resolver = GuardedResolver::new(stub, shared(FetchConfig::default()));

        let first = resolve_once(&resolver, "example.com")
            .await
            .expect("the public answer must resolve");
        assert_eq!(first, vec![sa("93.184.216.34:0")]);

        let err = resolve_once(&resolver, "example.com")
            .await
            .expect_err("the loopback answer must be refused on the second call");
        assert!(
            err.downcast_ref::<FetchError>()
                .is_some_and(|e| matches!(e, FetchError::NoAllowedAddress { .. })),
            "expected NoAllowedAddress, got: {err}"
        );
    }

    #[tokio::test]
    async fn multi_answer_yields_only_the_public_address() {
        let stub = StubLookup::new([vec![sa("93.184.216.34:0"), sa("127.0.0.1:0")]]);
        let resolver = GuardedResolver::new(stub, shared(FetchConfig::default()));

        let addrs = resolve_once(&resolver, "example.com")
            .await
            .expect("the public address must survive filtering");
        assert_eq!(
            addrs,
            vec![sa("93.184.216.34:0")],
            "only the public address must be returned"
        );
    }

    #[tokio::test]
    async fn only_private_fails_with_no_allowed_address() {
        let stub = StubLookup::new([vec![sa("10.0.0.5:0"), sa("127.0.0.1:0")]]);
        let resolver = GuardedResolver::new(stub, shared(FetchConfig::default()));

        let err = resolve_once(&resolver, "internal.example")
            .await
            .expect_err("a host with only private addresses must fail");
        assert!(
            err.downcast_ref::<FetchError>().is_some_and(
                |e| matches!(e, FetchError::NoAllowedAddress { host } if host == "internal.example")
            ),
            "expected NoAllowedAddress for the host, got: {err}"
        );
    }

    #[tokio::test]
    async fn allow_exact_lets_the_whitelisted_address_through() {
        let cfg = FetchConfig::builder()
            .allow_host_address("localhost", "127.0.0.1".parse().expect("ip"))
            .build()
            .expect("valid config");
        let stub = StubLookup::new([vec![sa("127.0.0.1:0")]]);
        let resolver = GuardedResolver::new(stub, shared(cfg));

        let addrs = resolve_once(&resolver, "localhost")
            .await
            .expect("allow_exact must let the loopback address through");
        assert_eq!(addrs, vec![sa("127.0.0.1:0")]);
    }

    #[tokio::test]
    async fn allow_exact_refuses_a_different_host_for_the_same_address() {
        let cfg = FetchConfig::builder()
            .allow_host_address("localhost", "127.0.0.1".parse().expect("ip"))
            .build()
            .expect("valid config");
        let stub = StubLookup::new([vec![sa("127.0.0.1:0")]]);
        let resolver = GuardedResolver::new(stub, shared(cfg));

        let err = resolve_once(&resolver, "evil.com")
            .await
            .expect_err("a different host must not inherit localhost's allow_exact entry");
        assert!(
            err.downcast_ref::<FetchError>().is_some_and(
                |e| matches!(e, FetchError::NoAllowedAddress { host } if host == "evil.com")
            ),
            "expected NoAllowedAddress for the host, got: {err}"
        );
    }
}
