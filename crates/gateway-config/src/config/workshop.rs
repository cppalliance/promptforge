//! Deprecated `[workshop]` hosting settings retained so older boot
//! configurations still parse.
//!
//! There is deliberately no `[workshop.gateway]` sub-table: the hosting
//! gateway derives the workshop's client URL from its own
//! `[server]` bind ([`ServerConfig::client_url`](super::ServerConfig::client_url))
//! and reuses the same api_key, so no credential is duplicated and none can
//! drift.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

fn default_workshop_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 7910))
}

/// The deprecated `[workshop]` hosting section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkshopConfig {
    /// The socket address the workshop listener binds. Defaults to
    /// `127.0.0.1:7910`.
    #[serde(default = "default_workshop_bind")]
    bind: SocketAddr,
    /// Whether the gateway opens the system browser at the workshop URL once
    /// it is serving. Defaults to false.
    #[serde(default)]
    open_browser: bool,
}

impl WorkshopConfig {
    /// Returns the socket address the workshop listener binds.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let workshop = config.workshop().expect("workshop section present");
    /// assert_eq!(workshop.bind().to_string(), "127.0.0.1:7910");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn bind(&self) -> SocketAddr {
        self.bind
    }

    /// Returns whether the gateway opens the system browser at the workshop
    /// URL once it is serving.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop]
    /// # open_browser = true
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.workshop().expect("workshop section present").open_browser());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn open_browser(&self) -> bool {
        self.open_browser
    }
}

#[cfg(test)]
mod tests {
    use super::WorkshopConfig;
    use crate::config::Config;

    /// A minimal valid `[server]` to prefix workshop fixtures with.
    const BASE: &str = "config-version = 2\n[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"k\"\n";

    fn parse(extra: &str) -> Config {
        Config::from_toml_str(&format!("{BASE}{extra}")).expect("fixture parses")
    }

    #[test]
    fn workshop_absent_is_none() {
        assert!(parse("").workshop().is_none());
    }

    #[test]
    fn workshop_empty_section_takes_defaults() {
        let config = parse("[workshop]\n");
        let workshop = config.workshop().expect("workshop section present");
        assert_eq!(workshop.bind().to_string(), "127.0.0.1:7910");
        assert!(!workshop.open_browser());
    }

    #[test]
    fn workshop_section_parses_explicit_fields() {
        let config = parse(
            r#"
[workshop]
bind = "127.0.0.1:7999"
open_browser = true
"#,
        );
        let workshop = config.workshop().expect("workshop section present");
        assert_eq!(workshop.bind().to_string(), "127.0.0.1:7999");
        assert!(workshop.open_browser());
    }

    #[test]
    fn workshop_rejects_unknown_fields_in_every_sub_table() {
        for section in ["[workshop]\nbogus = 1\n", "[workshop.stt]\nbogus = 1\n"] {
            let error = Config::from_toml_str(&format!("{BASE}{section}"))
                .expect_err("an unknown workshop field must fail");
            assert_eq!(error.kind(), crate::ConfigErrorKind::Parse, "in {section}");
        }
    }

    #[test]
    fn workshop_client_url_swaps_unspecified_bind_for_loopback() {
        let url = |bind: &str| {
            Config::from_toml_str(&format!(
                "config-version = 2\n[server]\nbind = \"{bind}\"\napi_key = \"k\"\n"
            ))
            .expect("fixture parses")
            .server()
            .client_url()
        };
        assert_eq!(url("0.0.0.0:8081"), "http://127.0.0.1:8081");
        assert_eq!(url("[::]:8081"), "http://[::1]:8081");
    }

    #[test]
    fn workshop_client_url_keeps_reachable_binds() {
        let url = |bind: &str| {
            Config::from_toml_str(&format!(
                "config-version = 2\n[server]\nbind = \"{bind}\"\napi_key = \"k\"\n"
            ))
            .expect("fixture parses")
            .server()
            .client_url()
        };
        assert_eq!(url("127.0.0.1:8081"), "http://127.0.0.1:8081");
        assert_eq!(url("192.168.1.5:9000"), "http://192.168.1.5:9000");
    }

    #[test]
    fn workshop_config_round_trips_through_json() {
        let config = parse(
            r#"
[workshop]
bind = "127.0.0.1:7999"
open_browser = true
"#,
        );
        let workshop = config.workshop().expect("workshop section present");
        let json = serde_json::to_value(workshop).expect("serializes");
        let back: WorkshopConfig = serde_json::from_value(json).expect("deserializes");
        assert_eq!(workshop, &back);
    }
}
