//! Gateway configuration read once at the process boundary.
//!
//! [`GatewayEnv`] pairs the gateway base URL with a [`GatewayKey`], a bearer
//! credential whose contents never reach `Debug` output or a log line. The
//! binary validates the environment once at startup and lends the resulting
//! [`GatewayEnv`] inward, so no runtime path reads the credential twice or
//! renders it in the clear.

use anyhow::{Result, bail};

/// A bearer credential that renders as `<redacted>` in `Debug`.
///
/// The raw value is exposed only through [`GatewayKey::expose`], a
/// crate-internal accessor used to build the gateway client and the credentialed
/// tools. Keeping the secret in this newtype means an accidental `{:?}` on a
/// [`GatewayEnv`] cannot leak it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct GatewayKey(String);

impl GatewayKey {
    /// Wraps a raw bearer credential.
    pub(crate) fn new(secret: impl Into<String>) -> GatewayKey {
        GatewayKey(secret.into())
    }

    /// Borrows the raw secret for client and tool construction.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for GatewayKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GatewayKey(<redacted>)")
    }
}

/// Gateway URL and bearer required for every run.
///
/// The [`key`](GatewayEnv::key) is a [`GatewayKey`], so deriving `Debug` here
/// renders the credential as `<redacted>` rather than in the clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayEnv {
    /// The gateway API root (`PROMPTFORGE_GATEWAY_URL`).
    pub(crate) base_url: String,
    /// The bearer credential (`PROMPTFORGE_GATEWAY_KEY`).
    pub(crate) key: GatewayKey,
}

/// Reads and validates `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY`
/// from the process environment.
///
/// # Errors
///
/// Returns a friendly error naming each missing variable when either is unset
/// or empty.
pub(crate) fn require_gateway_env() -> Result<GatewayEnv> {
    require_gateway_env_from(|name| std::env::var(name).ok())
}

/// [`require_gateway_env`] with an injected variable lookup for offline tests.
pub(crate) fn require_gateway_env_from(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<GatewayEnv> {
    let base_url = lookup("PROMPTFORGE_GATEWAY_URL").filter(|value| !value.is_empty());
    let key = lookup("PROMPTFORGE_GATEWAY_KEY").filter(|value| !value.is_empty());
    match (base_url, key) {
        (Some(base_url), Some(key)) => Ok(GatewayEnv {
            base_url,
            key: GatewayKey::new(key),
        }),
        (None, None) => bail!(
            "missing environment variables PROMPTFORGE_GATEWAY_URL and PROMPTFORGE_GATEWAY_KEY\n\
             start promptforge-gateway first, then export both before running promptforge-dev"
        ),
        (None, Some(_)) => bail!(
            "missing environment variable PROMPTFORGE_GATEWAY_URL\n\
             start promptforge-gateway first, then export PROMPTFORGE_GATEWAY_URL and PROMPTFORGE_GATEWAY_KEY"
        ),
        (Some(_), None) => bail!(
            "missing environment variable PROMPTFORGE_GATEWAY_KEY\n\
             start promptforge-gateway first, then export PROMPTFORGE_GATEWAY_URL and PROMPTFORGE_GATEWAY_KEY"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{GatewayEnv, GatewayKey, require_gateway_env_from};

    fn lookup_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn missing_both_gateway_vars_fails() {
        let error =
            require_gateway_env_from(lookup_from(&[])).expect_err("both vars missing must fail");
        let message = format!("{error:#}");
        assert!(
            message.contains("PROMPTFORGE_GATEWAY_URL")
                && message.contains("PROMPTFORGE_GATEWAY_KEY"),
            "unexpected missing-env message: {message}"
        );
    }

    #[test]
    fn missing_url_alone_fails_with_a_friendly_message() {
        let error = require_gateway_env_from(lookup_from(&[("PROMPTFORGE_GATEWAY_KEY", "secret")]))
            .expect_err("URL missing must fail");
        assert!(
            format!("{error:#}")
                .starts_with("missing environment variable PROMPTFORGE_GATEWAY_URL\n"),
            "unexpected missing-url message: {error:#}"
        );
    }

    #[test]
    fn missing_key_alone_fails_with_a_friendly_message() {
        let error =
            require_gateway_env_from(lookup_from(&[("PROMPTFORGE_GATEWAY_URL", "http://x/v1")]))
                .expect_err("key missing must fail");
        assert!(
            format!("{error:#}")
                .starts_with("missing environment variable PROMPTFORGE_GATEWAY_KEY\n"),
            "unexpected missing-key message: {error:#}"
        );
    }

    #[test]
    fn empty_gateway_vars_count_as_missing() {
        let error = require_gateway_env_from(lookup_from(&[
            ("PROMPTFORGE_GATEWAY_URL", ""),
            ("PROMPTFORGE_GATEWAY_KEY", ""),
        ]))
        .expect_err("empty vars must fail");
        assert!(
            format!("{error:#}").contains("PROMPTFORGE_GATEWAY_URL"),
            "unexpected empty-env message: {error:#}"
        );
    }

    #[test]
    fn present_gateway_vars_are_accepted() {
        let gateway = require_gateway_env_from(lookup_from(&[
            ("PROMPTFORGE_GATEWAY_URL", "http://10.0.0.7:9999/v1"),
            ("PROMPTFORGE_GATEWAY_KEY", "dev-secret"),
        ]))
        .expect("both vars present must succeed");
        assert_eq!(gateway.base_url, "http://10.0.0.7:9999/v1");
        assert_eq!(gateway.key.expose(), "dev-secret");
    }

    #[test]
    fn debug_never_reveals_the_bearer_key() {
        let env = GatewayEnv {
            base_url: "http://gw/v1".to_owned(),
            key: GatewayKey::new("super-secret-token"),
        };
        let rendered = format!("{env:?}");
        assert!(
            !rendered.contains("super-secret-token"),
            "the key must never appear in Debug output: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "Debug output must mark the key as redacted: {rendered}"
        );
    }
}
