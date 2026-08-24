//! Application entry points: the [`run`] function and the assembled [`Gateway`].
//!
//! `run` is the binary path: it loads configuration, provisions local children,
//! binds, and serves until Ctrl-C. `Gateway` is the in-process assembly seam
//! used by `run` and by integration tests, which bind their own listener and
//! drive [`Gateway::serve`] with a caller-owned shutdown signal.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::TcpListener;

use promptforge_gateway_config::{Config, ConfigError, ProfileName, ServerConfig};

use crate::api_error::{ServeError, StartupError};
use crate::local::LocalRuntime;
use crate::routing::Routing;
use crate::{AppState, build_router};

/// Options for running the gateway. Built by the binary from parsed args.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Path to the boot TOML (the catalog); profiles dir is its sibling `profiles/`.
    pub config_path: PathBuf,
    /// The profile to boot into; the initial loaded set.
    pub profile: ProfileName,
}

impl ServeOptions {
    /// Build serve options from the boot config path and the profile name.
    #[must_use]
    pub fn new(config_path: PathBuf, profile: ProfileName) -> ServeOptions {
        ServeOptions {
            config_path,
            profile,
        }
    }
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
    /// The config's `[server]` is retained as the boot-owned server settings;
    /// profile switches are checked against it (the socket and the gateway
    /// bearer key are fixed for the process lifetime).
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
    /// api_key = "secret"
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
            crate::ProfileSelection {
                name: profiles.active.map(|name| name.to_string()),
                model_allowlist: config.model_allowlist().map(<[String]>::to_vec),
            },
            config.server().clone(),
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
pub fn run(options: &ServeOptions) -> Result<(), StartupError> {
    let (config, profiles) = load_startup(options)?;
    let bind = config.bind_addr();
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

/// The profiles directory for a boot config path: its sibling `profiles/`.
/// A bare filename resolves to `./profiles`.
pub(crate) fn profiles_dir_for(config_path: &Path) -> PathBuf {
    config_path.parent().map_or_else(
        || PathBuf::from("profiles"),
        |parent| parent.join("profiles"),
    )
}

/// The boot-owned `[server]` rule: a profile's merged `[server]` must equal
/// the boot file's `[server]` as values after interpolation, so the socket
/// and the gateway bearer key are fixed for the process lifetime.
///
/// `bind` mismatches name both addresses; `api_key` mismatches report the
/// difference without printing either secret.
pub(crate) fn check_server_matches_boot(
    boot: &ServerConfig,
    candidate: &ServerConfig,
    profile: &ProfileName,
) -> Result<(), ConfigError> {
    if candidate.bind() != boot.bind() {
        return Err(ConfigError::validation(format!(
            "profile {profile} [server] bind mismatch: profile has {}, boot file has {}",
            candidate.bind(),
            boot.bind()
        )));
    }
    if candidate.api_key().expose() != boot.api_key().expose() {
        return Err(ConfigError::validation(format!(
            "profile {profile} [server] api_key mismatch: the profile's key differs from the boot file's (both values redacted)"
        )));
    }
    Ok(())
}

/// Boot into the named profile and build the admin profiles context.
///
/// Order matters: the two env files load first (the profile's `<name>.env`,
/// then the boot file's sibling env file; dotenvy never overrides, so the
/// earlier file wins and both lose to the process environment), then the
/// profile resolves with its include chain, then the boot file's `[server]`
/// is extracted and compared. The resolved chain is logged, with a warning
/// when the boot file is not in it (the likely-mistake case: an operator
/// edits the boot file and nothing changes).
fn load_startup(options: &ServeOptions) -> Result<(Config, ProfilesContext), StartupError> {
    let profiles_dir = profiles_dir_for(&options.config_path);

    let profile_path = profiles_dir.join(format!("{}.toml", options.profile));
    load_env_file(&profile_path.with_extension("env"));
    load_env_file(&options.config_path.with_extension("env"));

    let available =
        promptforge_gateway_config::list_profiles(&profiles_dir).map_err(StartupError::config)?;
    let wanted = options.profile.to_string();
    if !available.contains(&wanted) {
        let message = if available.is_empty() {
            format!(
                "unknown profile {wanted}; no profiles found in {}",
                profiles_dir.display()
            )
        } else {
            format!(
                "unknown profile {wanted}; available profiles: {}",
                available.join(", ")
            )
        };
        return Err(StartupError::config(ConfigError::validation(message)));
    }

    let (config, chain) = Config::load_profile_with_chain(&profiles_dir, &options.profile)
        .map_err(StartupError::config)?;
    let boot_server = promptforge_gateway_config::load_server(&options.config_path)
        .map_err(StartupError::config)?;
    check_server_matches_boot(&boot_server, config.server(), &options.profile)
        .map_err(StartupError::config)?;

    let rendered = chain
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(" -> ");
    tracing::info!(profile = %options.profile, chain = %rendered, "resolved profile include chain");
    if !chain_contains(&chain, &options.config_path) {
        tracing::warn!(
            "boot file {} is not in the include chain of profile {}; edits to it have no effect",
            options.config_path.display(),
            options.profile
        );
    }

    Ok((
        config,
        ProfilesContext::new(Some(profiles_dir), Some(options.profile.clone())),
    ))
}

/// Load an env file into the process environment, skipping missing files.
/// dotenvy never overrides variables that are already set. A malformed or
/// unreadable file is ignored: any variable it failed to set surfaces at
/// interpolation as an unresolved-`${VAR}` error naming the variable.
pub(crate) fn load_env_file(env_path: &Path) {
    if env_path.exists() {
        let _ = dotenvy::from_path(env_path);
    }
}

/// Whether `path` appears in `chain`, comparing canonicalized paths so a
/// chain entry like `profiles/../gateway.toml` matches a CLI `gateway.toml`.
fn chain_contains(chain: &[PathBuf], path: &Path) -> bool {
    let wanted = canonical(path);
    chain.iter().any(|entry| canonical(entry) == wanted)
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::path::{Path, PathBuf};

    use super::{
        ServeOptions, ShutdownTrigger, check_server_matches_boot, classify_shutdown, load_startup,
        profiles_dir_for,
    };
    use promptforge_gateway_config::{Config, ProfileName};

    #[test]
    fn classify_shutdown_distinguishes_interrupt_from_handler_failure() {
        assert_eq!(classify_shutdown(&Ok(())), ShutdownTrigger::Interrupted);
        assert_eq!(
            classify_shutdown(&Err(std::io::Error::other("no handler"))),
            ShutdownTrigger::HandlerFailed
        );
    }

    #[test]
    fn profiles_dir_is_the_boot_files_sibling() {
        assert_eq!(
            profiles_dir_for(Path::new("/etc/pf/gateway.toml")),
            PathBuf::from("/etc/pf/profiles")
        );
        assert_eq!(
            profiles_dir_for(Path::new("gateway.toml")),
            PathBuf::from("profiles"),
            "a bare filename resolves to ./profiles"
        );
    }

    /// A boot catalog that needs no `${VAR}` interpolation.
    const CATALOG: &str = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "boot-key"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// A catalog whose boot key comes from the environment.
    const CATALOG_ENV_KEY: &str = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "${PFG_S4_BOOT_KEY}"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#;

    /// A tempdir holding `gateway.toml` plus an empty `profiles/` dir.
    fn boot_fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        write(tmp.path(), "gateway.toml", CATALOG);
        std::fs::create_dir(tmp.path().join("profiles")).unwrap();
        let config_path = tmp.path().join("gateway.toml");
        (tmp, config_path)
    }

    /// The error's Display plus every source in the chain, for assertions.
    fn error_text(error: &crate::api_error::StartupError) -> String {
        let mut text = error.to_string();
        let mut source = error.source();
        while let Some(cause) = source {
            text.push_str("; ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        text
    }

    #[test]
    fn boots_into_a_profile_with_active_reported() {
        let (tmp, config_path) = boot_fixture();
        write(
            &tmp.path().join("profiles"),
            "main.toml",
            "include = [\"../gateway.toml\"]\n",
        );

        let options = ServeOptions::new(config_path, ProfileName::parse("main").unwrap());
        let (config, context) = load_startup(&options).unwrap();
        assert_eq!(config.models()[0].name(), "m");
        assert_eq!(context.active, Some(ProfileName::parse("main").unwrap()));
        assert_eq!(context.dir, Some(tmp.path().join("profiles")));
    }

    #[test]
    fn boot_loads_profile_env_first_so_it_wins_over_boot_env() {
        let tmp = tempfile::TempDir::new().unwrap();
        write(tmp.path(), "gateway.toml", CATALOG_ENV_KEY);
        write(tmp.path(), "gateway.env", "PFG_S4_BOOT_KEY=from-boot-env\n");
        let profiles = tmp.path().join("profiles");
        std::fs::create_dir(&profiles).unwrap();
        write(&profiles, "main.toml", "include = [\"../gateway.toml\"]\n");
        write(&profiles, "main.env", "PFG_S4_BOOT_KEY=from-profile-env\n");

        let options = ServeOptions::new(
            tmp.path().join("gateway.toml"),
            ProfileName::parse("main").unwrap(),
        );
        let (config, _context) = load_startup(&options).unwrap();
        assert_eq!(
            config.server().api_key().expose(),
            "from-profile-env",
            "the profile env loads first and dotenvy never overrides"
        );
    }

    #[test]
    fn self_contained_profile_boots_when_server_matches() {
        // Value equality, not provenance: a profile replicating [server]
        // verbatim passes even though the boot file is not in its chain.
        let (tmp, config_path) = boot_fixture();
        write(&tmp.path().join("profiles"), "solo.toml", CATALOG);

        let options = ServeOptions::new(config_path, ProfileName::parse("solo").unwrap());
        let (config, _context) = load_startup(&options).unwrap();
        assert_eq!(config.models()[0].name(), "m");
    }

    #[test]
    fn server_key_mismatch_fails_boot() {
        let (tmp, config_path) = boot_fixture();
        write(
            &tmp.path().join("profiles"),
            "main.toml",
            "include = [\"../gateway.toml\"]\n\n[server]\napi_key = \"other-key\"\n",
        );

        let options = ServeOptions::new(config_path, ProfileName::parse("main").unwrap());
        let error = load_startup(&options).unwrap_err();
        let text = error_text(&error);
        assert!(text.contains("api_key mismatch"), "got: {text}");
        assert!(!text.contains("other-key"), "secrets stay redacted: {text}");
    }

    #[test]
    fn server_bind_mismatch_fails_boot_naming_both_values() {
        let (tmp, config_path) = boot_fixture();
        write(
            &tmp.path().join("profiles"),
            "main.toml",
            "include = [\"../gateway.toml\"]\n\n[server]\nbind = \"127.0.0.1:9999\"\n",
        );

        let options = ServeOptions::new(config_path, ProfileName::parse("main").unwrap());
        let error = load_startup(&options).unwrap_err();
        let text = error_text(&error);
        assert!(text.contains("bind mismatch"), "got: {text}");
        assert!(text.contains("127.0.0.1:9999"), "profile value: {text}");
        assert!(text.contains("127.0.0.1:8081"), "boot value: {text}");
    }

    #[test]
    fn unknown_profile_error_lists_available_profiles() {
        let (tmp, config_path) = boot_fixture();
        let profiles = tmp.path().join("profiles");
        write(&profiles, "alpha.toml", "include = [\"../gateway.toml\"]\n");
        write(&profiles, "beta.toml", "include = [\"../gateway.toml\"]\n");

        let options = ServeOptions::new(config_path, ProfileName::parse("ghost").unwrap());
        let error = load_startup(&options).unwrap_err();
        let text = error_text(&error);
        assert!(
            text.contains("ghost"),
            "names the requested profile: {text}"
        );
        assert!(text.contains("alpha"), "lists alpha: {text}");
        assert!(text.contains("beta"), "lists beta: {text}");
    }

    #[test]
    fn missing_profiles_dir_is_a_boot_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        write(tmp.path(), "gateway.toml", CATALOG);

        let options = ServeOptions::new(
            tmp.path().join("gateway.toml"),
            ProfileName::parse("main").unwrap(),
        );
        let error = load_startup(&options).unwrap_err();
        let text = error_text(&error);
        assert!(text.contains("no profiles found"), "got: {text}");
    }

    #[test]
    fn server_check_compares_values_not_provenance() {
        // Two separately parsed copies of the same [server] are equal as
        // values; no shared origin is required.
        let boot = Config::from_toml_str(CATALOG).unwrap().server().clone();
        let same = Config::from_toml_str(CATALOG).unwrap().server().clone();
        let profile = ProfileName::parse("p").unwrap();
        check_server_matches_boot(&boot, &same, &profile).unwrap();
    }
}
