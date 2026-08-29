//! The hosted workshop: the workshop UI server on a second, loopback-only
//! listener in the gateway process, compiled in behind the `workshop`
//! feature and started when the boot config carries a `[workshop]` section.
//!
//! The workshop reaches the gateway through its own HTTP client over
//! loopback: the client URL and bearer key are derived from the boot
//! `[server]` section, so no credential is duplicated in `[workshop]` and
//! none can drift. Without the feature, this module is a stub whose
//! [`spawn_if_configured`] hosts nothing and whose [`WorkshopHandle`] is
//! never constructed.

#[cfg(feature = "workshop")]
mod hosted {
    use std::net::SocketAddr;
    use std::path::Path;

    use promptforge_gateway_config::{
        Config, ConfigError, ServerConfig, WorkshopConfig, WorkshopVoiceConfig,
    };

    use crate::api_error::StartupError;

    /// A running hosted workshop server, held by [`crate::GatewayHandle`].
    #[derive(Debug)]
    pub(crate) struct WorkshopHandle {
        inner: promptforge_workshop_server::ServerHandle,
    }

    impl WorkshopHandle {
        /// The base URL of the workshop listener.
        pub(crate) fn url(&self) -> &str {
            self.inner.url()
        }

        /// Stops the workshop and waits for it, bounded by the workshop's
        /// own drain watchdog. Stop outcomes are logged rather than
        /// returned: they are not actionable by the caller and must never
        /// preempt the gateway's own shutdown, which runs next.
        pub(crate) fn shutdown(self) {
            let url = self.inner.url().to_string();
            match self.inner.shutdown() {
                Ok(promptforge_workshop_server::Termination::Graceful) => {
                    tracing::info!("workshop at {url} stopped gracefully");
                }
                Ok(promptforge_workshop_server::Termination::Forced) => {
                    tracing::warn!("workshop at {url} was forced down after its drain window");
                }
                // Termination is non-exhaustive; a future ending is still a
                // stop worth one line.
                Ok(termination) => {
                    tracing::info!("workshop at {url} stopped ({termination:?})");
                }
                Err(error) => {
                    tracing::warn!("workshop at {url} stopped with an error: {error}");
                }
            }
        }
    }

    /// Spawns the workshop server when the boot config carries a
    /// `[workshop]` section; `bound` is the gateway listener's address,
    /// already bound. Logs the workshop URL and, when `open_browser` is
    /// set, opens the system browser at it (the headless-server-with-UI
    /// frame; the desktop shell drives its own window instead).
    ///
    /// # Errors
    /// Returns a config-kind [`StartupError`] when the workshop bind is not
    /// a loopback address, and a workshop-kind one when the server itself
    /// fails to start (a bad tape path, a taken port).
    pub(crate) fn spawn_if_configured(
        config: &Config,
        config_path: &Path,
        bound: SocketAddr,
    ) -> Result<Option<WorkshopHandle>, StartupError> {
        spawn_with_opener(config, config_path, bound, |url| open::that(url))
    }

    /// The testable core of [`spawn_if_configured`], with the browser
    /// opener injected: production opens the system browser, tests record
    /// the URL instead of opening a real one.
    fn spawn_with_opener(
        config: &Config,
        config_path: &Path,
        bound: SocketAddr,
        open_url: impl FnOnce(&str) -> std::io::Result<()>,
    ) -> Result<Option<WorkshopHandle>, StartupError> {
        let Some(workshop) = config.workshop() else {
            return Ok(None);
        };
        // The workshop UI writes workspace files and drives profile
        // switches, so its listener stays loopback-only; only the gateway's
        // own listener may bind wider.
        if !workshop.bind().ip().is_loopback() {
            return Err(StartupError::config(ConfigError::validation(format!(
                "[workshop] bind {} is not a loopback address; the workshop listener is loopback-only",
                workshop.bind()
            ))));
        }
        // Both routers expose /health and /v1/models. With two listeners the
        // duplication is harmless - each port answers with its own - but it
        // is the known blocker for the documented future option of nesting
        // the workshop under a path on the gateway listener.
        let handle = promptforge_workshop_server::spawn(ws_config(
            config.server(),
            workshop,
            config_path,
            bound,
        ))
        .map_err(StartupError::workshop)?;
        tracing::info!("workshop serving on {}", handle.url());
        if workshop.open_browser() {
            // A browser that will not open is not worth failing a serving
            // process over; the URL is logged above either way.
            if let Err(error) = open_url(handle.url()) {
                tracing::warn!(
                    "could not open the system browser at {}: {error}",
                    handle.url()
                );
            }
        }
        Ok(Some(WorkshopHandle { inner: handle }))
    }

    /// Builds the workshop server's config from the gateway's boot config:
    /// the gateway client derived from `[server]`, the tape path anchored
    /// to the boot config's directory, and `[workshop]`'s own listener and
    /// voice settings mirrored across.
    fn ws_config(
        server: &ServerConfig,
        workshop: &WorkshopConfig,
        config_path: &Path,
        bound: SocketAddr,
    ) -> promptforge_workshop_server::Config {
        let boot_dir = config_path.parent().unwrap_or(Path::new("."));
        promptforge_workshop_server::Config {
            gateway: promptforge_workshop_server::GatewayConfig {
                base_url: client_url(server, bound),
                api_key: server.api_key().expose().to_string(),
            },
            tape: promptforge_workshop_server::TapeConfig {
                path: workshop.tape_path(boot_dir),
            },
            server: promptforge_workshop_server::ServerConfig {
                bind: workshop.bind().to_string(),
                open_browser: workshop.open_browser(),
            },
            voice: workshop.voice().map_or_else(
                promptforge_workshop_server::VoiceConfig::default,
                voice_config,
            ),
        }
    }

    /// The gateway URL the workshop's client dials: the boot `[server]`'s
    /// loopback-adjusted client URL. A port-0 bind is ephemeral - the
    /// derived URL would name the undialable port 0 - so the actually
    /// bound port is swapped in.
    fn client_url(server: &ServerConfig, bound: SocketAddr) -> String {
        let url = server.client_url();
        if server.bind().port() != 0 {
            return url;
        }
        // client_url() always renders as http://<host>:<port>, so the last
        // colon separates the port; the None arm cannot occur.
        match url.rsplit_once(':') {
            Some((head, _)) => format!("{head}:{}", bound.port()),
            None => url,
        }
    }

    /// Mirrors `[workshop.voice]` onto the workshop server's own voice
    /// settings, field for field.
    fn voice_config(voice: &WorkshopVoiceConfig) -> promptforge_workshop_server::VoiceConfig {
        promptforge_workshop_server::VoiceConfig {
            interim_model: voice.interim_model().to_path_buf(),
            final_model: voice.final_model().to_path_buf(),
            interim_source: voice.interim_source().to_string(),
            final_source: voice.final_source().to_string(),
            window_seconds: voice.window_seconds(),
            interval_ms: voice.interval_ms(),
            vocabulary: voice.vocabulary().to_vec(),
        }
    }

    #[cfg(test)]
    mod tests {
        use std::net::SocketAddr;
        use std::path::Path;

        use super::{client_url, spawn_if_configured, spawn_with_opener, ws_config};
        use crate::api_error::StartupErrorKind;
        use promptforge_gateway_config::Config;

        fn config(toml: &str) -> Config {
            Config::from_toml_str(toml).expect("fixture parses")
        }

        fn bound(address: &str) -> SocketAddr {
            address.parse().expect("fixture address parses")
        }

        #[test]
        fn ws_config_derives_the_client_and_anchors_the_tape() {
            let config = config(
                r#"
[server]
bind = "0.0.0.0:8081"
api_key = "boot-key"

[workshop]
bind = "127.0.0.1:7911"
open_browser = true

[workshop.tape]
path = "tapes/session.jsonl"

[workshop.voice]
interim_model = "models/tiny.bin"
final_model = "models/small.bin"
interim_source = "https://example.com/tiny.bin"
final_source = "https://example.com/small.bin"
window_seconds = 8
interval_ms = 250
vocabulary = ["MCP", "GGUF"]
"#,
            );
            let workshop = config.workshop().expect("workshop section present");
            let ws = ws_config(
                config.server(),
                workshop,
                Path::new("/etc/pf/gateway.toml"),
                bound("0.0.0.0:8081"),
            );
            assert_eq!(
                ws.gateway.base_url, "http://127.0.0.1:8081",
                "an unspecified gateway bind derives a loopback client URL"
            );
            assert_eq!(
                ws.gateway.api_key, "boot-key",
                "the workshop reuses the gateway bearer key"
            );
            assert_eq!(
                ws.tape.path,
                Path::new("/etc/pf").join("tapes").join("session.jsonl"),
                "a relative tape path anchors to the boot config's directory"
            );
            assert_eq!(ws.server.bind, "127.0.0.1:7911");
            assert!(ws.server.open_browser);
            assert_eq!(ws.voice.interim_model, Path::new("models/tiny.bin"));
            assert_eq!(ws.voice.final_model, Path::new("models/small.bin"));
            assert_eq!(ws.voice.interim_source, "https://example.com/tiny.bin");
            assert_eq!(ws.voice.final_source, "https://example.com/small.bin");
            assert_eq!(ws.voice.window_seconds, 8);
            assert_eq!(ws.voice.interval_ms, 250);
            assert_eq!(ws.voice.vocabulary, ["MCP", "GGUF"]);
        }

        #[test]
        fn an_absent_voice_section_maps_to_the_workshop_defaults() {
            let config =
                config("[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"k\"\n\n[workshop]\n");
            let workshop = config.workshop().expect("workshop section present");
            let ws = ws_config(
                config.server(),
                workshop,
                Path::new("gateway.toml"),
                bound("127.0.0.1:8081"),
            );
            assert_eq!(
                ws.voice,
                promptforge_workshop_server::VoiceConfig::default()
            );
            assert_eq!(
                ws.tape.path,
                Path::new("").join("tape.jsonl"),
                "an absent tape section anchors the default filename to the boot dir"
            );
        }

        #[test]
        fn the_client_url_swaps_a_port_zero_bind_for_the_bound_port() {
            let ephemeral = config("[server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n");
            assert_eq!(
                client_url(ephemeral.server(), bound("127.0.0.1:49321")),
                "http://127.0.0.1:49321",
                "a port-0 bind is undialable; the bound port replaces it"
            );

            let fixed = config("[server]\nbind = \"0.0.0.0:8081\"\napi_key = \"k\"\n");
            assert_eq!(
                client_url(fixed.server(), bound("0.0.0.0:8081")),
                "http://127.0.0.1:8081",
                "a fixed bind keeps the config-derived URL"
            );
        }

        #[test]
        fn a_non_loopback_workshop_bind_is_refused() {
            let config = config(
                "[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"k\"\n\n[workshop]\nbind = \"0.0.0.0:7910\"\n",
            );
            let error =
                spawn_if_configured(&config, Path::new("gateway.toml"), bound("127.0.0.1:8081"))
                    .expect_err("a non-loopback workshop bind must fail");
            assert_eq!(error.kind(), StartupErrorKind::Config);
        }

        #[test]
        fn no_workshop_section_hosts_nothing() {
            let config = config("[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"k\"\n");
            let hosted =
                spawn_if_configured(&config, Path::new("gateway.toml"), bound("127.0.0.1:8081"))
                    .expect("no workshop section is not an error");
            assert!(hosted.is_none());
        }

        /// An ephemeral workshop config; `open_browser` as given. The
        /// tempdir anchors the tape path outside the source tree.
        fn opener_fixture(
            tmp: &tempfile::TempDir,
            open_browser: &str,
        ) -> (Config, std::path::PathBuf) {
            let config = config(&format!(
                "[server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\n[workshop]\nbind = \"127.0.0.1:0\"\n{open_browser}"
            ));
            (config, tmp.path().join("gateway.toml"))
        }

        #[test]
        fn the_open_browser_honor_opens_the_workshop_url() {
            let tmp = tempfile::TempDir::new().expect("tempdir");
            let (config, config_path) = opener_fixture(&tmp, "open_browser = true\n");
            let (tx, rx) = std::sync::mpsc::channel();
            let hosted =
                spawn_with_opener(&config, &config_path, bound("127.0.0.1:0"), move |url| {
                    tx.send(url.to_string()).expect("the receiver is alive");
                    Ok(())
                })
                .expect("the workshop spawns")
                .expect("a [workshop] section hosts a workshop");
            let opened = rx.recv().expect("the opener runs before spawn returns");
            assert_eq!(opened, hosted.url(), "the opener gets the workshop URL");
            hosted.shutdown();
        }

        #[test]
        fn the_opener_never_runs_without_the_open_browser_honor() {
            let tmp = tempfile::TempDir::new().expect("tempdir");
            let (config, config_path) = opener_fixture(&tmp, "");
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            let hosted =
                spawn_with_opener(&config, &config_path, bound("127.0.0.1:0"), move |url| {
                    tx.send(url.to_string()).expect("the receiver is alive");
                    Ok(())
                })
                .expect("the workshop spawns")
                .expect("a [workshop] section hosts a workshop");
            // spawn_with_opener is synchronous: an opener call would have
            // landed in the channel before it returned.
            assert!(
                rx.try_recv().is_err(),
                "the opener runs only when open_browser is set"
            );
            hosted.shutdown();
        }

        #[test]
        fn a_failing_opener_does_not_fail_the_spawn() {
            let tmp = tempfile::TempDir::new().expect("tempdir");
            let (config, config_path) = opener_fixture(&tmp, "open_browser = true\n");
            let hosted = spawn_with_opener(&config, &config_path, bound("127.0.0.1:0"), |_| {
                Err(std::io::Error::other("no display"))
            })
            .expect("a browser that will not open is not a startup failure")
            .expect("a [workshop] section hosts a workshop");
            hosted.shutdown();
        }
    }
}

#[cfg(feature = "workshop")]
pub(crate) use hosted::{WorkshopHandle, spawn_if_configured};

#[cfg(not(feature = "workshop"))]
mod absent {
    use std::net::SocketAddr;
    use std::path::Path;

    use promptforge_gateway_config::Config;

    use crate::api_error::StartupError;

    /// The never-constructed stand-in for the hosted workshop's handle, so
    /// the runner stays feature-blind.
    #[derive(Debug)]
    pub(crate) struct WorkshopHandle;

    /// The real handle signals its server on drop; the stand-in carries the
    /// same `Drop`-ness so the runner's feature-blind `drop` and `forget`
    /// calls mean the same thing under both builds.
    impl Drop for WorkshopHandle {
        fn drop(&mut self) {}
    }

    impl WorkshopHandle {
        /// The base URL of the workshop listener.
        #[expect(
            clippy::unused_self,
            reason = "the signature mirrors the workshop-feature variant"
        )]
        pub(crate) fn url(&self) -> &str {
            unreachable!("a WorkshopHandle is never constructed without the workshop feature")
        }

        /// Stops the workshop.
        #[expect(
            clippy::unused_self,
            reason = "the signature mirrors the workshop-feature variant"
        )]
        pub(crate) fn shutdown(self) {
            unreachable!("a WorkshopHandle is never constructed without the workshop feature")
        }
    }

    /// Hosts nothing: the `workshop` feature is not compiled in. A boot
    /// config that carries `[workshop]` anyway gets a warning, not an
    /// error, because the section is legal input for every build.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the signature mirrors the workshop-feature variant"
    )]
    pub(crate) fn spawn_if_configured(
        config: &Config,
        _config_path: &Path,
        _bound: SocketAddr,
    ) -> Result<Option<WorkshopHandle>, StartupError> {
        if config.workshop().is_some() {
            tracing::warn!(
                "the boot config carries a [workshop] section, but this gateway was built \
                 without the workshop feature; no workshop is hosted"
            );
        }
        Ok(None)
    }
}

#[cfg(not(feature = "workshop"))]
pub(crate) use absent::{WorkshopHandle, spawn_if_configured};
