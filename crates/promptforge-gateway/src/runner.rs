//! Application entry points: the [`run`] function and the assembled [`Gateway`].
//!
//! `run` is the binary path: it loads configuration, provisions local children,
//! binds, and serves until Ctrl-C. `Gateway` is the in-process assembly seam
//! used by `run` and by integration tests, which bind their own listener and
//! drive [`Gateway::serve`] with a caller-owned shutdown signal.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::api_error::{ServeError, StartupError};
use crate::config::Config;
use crate::local::LocalRuntime;
use crate::profile::ProfileName;
use crate::routing::Routing;
use crate::{AppState, build_router};

/// Options for running the gateway. Built by the binary from parsed args.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Directory holding named profile TOML files (used by admin routes).
    pub profiles_dir: PathBuf,
    /// Where startup configuration comes from.
    pub source: ConfigSource,
}

impl ServeOptions {
    /// Build serve options from a profiles directory and a config source.
    #[must_use]
    pub fn new(profiles_dir: PathBuf, source: ConfigSource) -> ServeOptions {
        ServeOptions {
            profiles_dir,
            source,
        }
    }
}

/// Where startup configuration comes from.
///
/// A profile and an explicit path are mutually exclusive by construction, which
/// replaces the "profile and/or path, but actually not both" contract the old
/// binary advertised then rejected at runtime.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// A named profile under the profiles directory.
    Profile(ProfileName),
    /// An explicit config file path.
    Path(PathBuf),
}

/// Optional admin-profile directory plus the active profile name.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct ProfilesContext {
    /// Directory of named profiles, enabling the admin switch routes.
    pub dir: Option<PathBuf>,
    /// The active profile name, reported by `GET /admin/status`.
    pub active: Option<ProfileName>,
}

impl ProfilesContext {
    /// Build a profiles context from an optional directory and active name.
    #[must_use]
    pub fn new(dir: Option<PathBuf>, active: Option<ProfileName>) -> ProfilesContext {
        ProfilesContext { dir, active }
    }
}

/// A fully assembled, owning gateway.
///
/// Holds the live routing table, the server key, the web-search capability, and
/// the local model runtime, so dropping a `Gateway` terminates every managed
/// `llama-server` child. The type is opaque; assemble it with
/// [`Gateway::from_config`].
#[derive(Debug)]
#[non_exhaustive]
pub struct Gateway {
    state: AppState,
}

impl Gateway {
    /// Assemble from a validated config. Provisions and starts local models.
    ///
    /// # Errors
    /// Returns [`StartupError`] when local provisioning or routing construction
    /// fails.
    ///
    /// # Examples
    /// ```
    /// use promptforge_gateway::{Config, Gateway, ProfilesContext};
    ///
    /// let toml = r#"
    /// [server]
    /// bind = "127.0.0.1:0"
    /// key = "secret"
    ///
    /// [[endpoint]]
    /// id = "e"
    /// protocol = "openai"
    /// base_url = "http://127.0.0.1:9"
    /// api_key = ""
    ///
    /// [[model]]
    /// name = "m"
    /// description = "a model"
    /// context = 8192
    /// upstream = "u"
    /// endpoints = ["e"]
    /// "#;
    /// let config = Config::from_toml_str(toml).unwrap();
    /// let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    /// let _router = gateway.router();
    /// ```
    pub fn from_config(
        config: &Config,
        profiles: ProfilesContext,
    ) -> Result<Gateway, StartupError> {
        let local = LocalRuntime::start(config).map_err(StartupError::provisioning)?;
        let routing = Routing::from_config(config)
            .map_err(StartupError::config)?
            .merge(local.models().iter().cloned())
            .map_err(StartupError::config)?;
        let state = AppState::from_parts(
            Arc::new(routing),
            config.server_key(),
            local,
            config.web_search_config(),
            profiles.dir,
            profiles.active.map(|name| name.to_string()),
        );
        Ok(Gateway { state })
    }

    /// The Axum router for this gateway.
    ///
    /// This is the crate's one deliberate, documented Axum integration point;
    /// the crate is an application, not a general library, so exposing an
    /// [`axum::Router`] here is intentional.
    pub fn router(&self) -> axum::Router {
        build_router(self.state.clone())
    }

    /// Serve on a caller-owned listener until `shutdown` completes.
    ///
    /// Tests pass an ephemeral [`TcpListener`] they bound themselves (no port
    /// race), read back `local_addr`, and drive a rendezvous instead of
    /// sleeping.
    ///
    /// # Errors
    /// Returns [`ServeError`] when the HTTP server fails.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServeError> {
        axum::serve(listener, build_router(self.state))
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(ServeError::io)
    }
}

/// Load config, provision local children, bind, and serve until Ctrl-C.
///
/// Owns the tokio runtime; the binary stays a thin arg-parsing shell.
///
/// # Errors
/// Returns [`StartupError`] when config loading, provisioning, binding, or
/// serving fails; classify with [`StartupError::kind`].
pub fn run(options: ServeOptions) -> Result<(), StartupError> {
    let (config, active) = load_startup(&options)?;
    let bind = config.bind_addr();
    let profiles = ProfilesContext::new(Some(options.profiles_dir), active);
    let gateway = Gateway::from_config(&config, profiles)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(StartupError::bind)?;
    runtime.block_on(async move {
        let listener = TcpListener::bind(bind).await.map_err(StartupError::bind)?;
        tracing::info!("promptforge-gateway serving on {bind}");
        gateway
            .serve(listener, shutdown_signal())
            .await
            .map_err(StartupError::serve)
    })
}

/// What awaiting the Ctrl-C signal produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownTrigger {
    /// A genuine interrupt was received; shut down gracefully.
    Interrupted,
    /// The signal handler could not be installed; do not spuriously shut down.
    HandlerFailed,
}

/// Classifies the result of awaiting Ctrl-C, distinguishing a real interrupt
/// from a failure to install the signal handler (MAIN-003).
fn classify_shutdown(result: &std::io::Result<()>) -> ShutdownTrigger {
    match result {
        Ok(()) => ShutdownTrigger::Interrupted,
        Err(_) => ShutdownTrigger::HandlerFailed,
    }
}

/// Resolves only on a genuine Ctrl-C interrupt.
///
/// If the signal handler cannot be installed, the error is logged and this
/// future never resolves, so a handler-install failure does not masquerade as
/// an interrupt and stop the server. The process can still be terminated by the
/// OS.
async fn shutdown_signal() {
    match classify_shutdown(&tokio::signal::ctrl_c().await) {
        ShutdownTrigger::Interrupted => {
            tracing::info!("received Ctrl-C; shutting down gracefully");
        }
        ShutdownTrigger::HandlerFailed => {
            tracing::error!("failed to install Ctrl-C handler; continuing to serve");
            std::future::pending::<()>().await;
        }
    }
}

fn load_startup(options: &ServeOptions) -> Result<(Config, Option<ProfileName>), StartupError> {
    match &options.source {
        ConfigSource::Profile(name) => {
            let config = crate::profile::load_named(&options.profiles_dir, name.as_str())
                .map_err(StartupError::config)?;
            Ok((config, Some(name.clone())))
        }
        ConfigSource::Path(path) => {
            let config = crate::profile::load_path(path).map_err(StartupError::config)?;
            let active = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| ProfileName::parse(stem).ok());
            Ok((config, active))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ShutdownTrigger, classify_shutdown};

    #[test]
    fn classify_shutdown_distinguishes_interrupt_from_handler_failure() {
        assert_eq!(classify_shutdown(&Ok(())), ShutdownTrigger::Interrupted);
        assert_eq!(
            classify_shutdown(&Err(std::io::Error::other("no handler"))),
            ShutdownTrigger::HandlerFailed
        );
    }
}
