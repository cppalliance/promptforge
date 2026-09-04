//! Persistent file and console logging for the desktop workshop shell.
//!
//! Directs structured diagnostics to rolling daily log files in the
//! idiomatic per-user operating system log directory:
//! - macOS: `~/Library/Logs/PromptForge/`
//! - Windows: `%LOCALAPPDATA%\PromptForge\logs\`
//! - Linux / Other: `${XDG_STATE_HOME:-~/.local/state}/promptforge/logs/`
//!
//! An override path can be set with the `PROMPTFORGE_LOG_DIR` environment
//! variable.

use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// The base name prefix for desktop log files.
const LOG_FILE_PREFIX: &str = "workshop.log";

/// The default log filter directives when `RUST_LOG` is unset.
const DEFAULT_LOG_FILTER: &str = "info,whisper_cpp=warn";

/// An opaque guard holding the background log worker thread active until
/// process exit.
#[derive(Debug, Default)]
pub(crate) struct LoggingGuard {
    _worker: Option<WorkerGuard>,
}

/// Initializes the global tracing subscriber for the desktop application,
/// writing to both the rotating file appender and stderr (when attached).
///
/// Also registers a panic hook that logs unhandled panics and backtraces
/// to the logger before propagating.
pub(crate) fn init_logging() -> LoggingGuard {
    let dir = resolve_log_dir();
    let (file_layer, guard) = match std::fs::create_dir_all(&dir) {
        Ok(()) => {
            let file_appender = tracing_appender::rolling::daily(&dir, LOG_FILE_PREFIX);
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
            let layer = tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false);
            (Some(layer), Some(guard))
        }
        Err(error) => {
            eprintln!("could not create log directory {}: {error}", dir.display());
            (None, None)
        }
    };

    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER));

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer.and_then(file_layer));

    let _ = subscriber.try_init();
    install_panic_hook();

    LoggingGuard { _worker: guard }
}

/// Installs a panic hook that records the panic location, message, and backtrace
/// through `tracing::error!`.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let payload = panic_info.payload();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "non-string panic payload"
        };
        let location = panic_info.location().map_or_else(
            || "unknown location".to_string(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );

        let backtrace = std::backtrace::Backtrace::capture();
        tracing::error!(
            location = %location,
            panic = %message,
            backtrace = %backtrace,
            "fatal unhandled panic"
        );
        default_hook(panic_info);
    }));
}

/// Resolves the log directory in precedence order: `PROMPTFORGE_LOG_DIR`
/// environment variable, then the operating system's standard per-user
/// log directory, falling back to `<home>/.promptforge/logs`.
pub(crate) fn resolve_log_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PROMPTFORGE_LOG_DIR")
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir);
    }
    let home = std::env::home_dir();
    resolve_platform_log_dir(home.as_deref())
}

/// Resolves the platform-specific log directory given an optional home directory.
fn resolve_platform_log_dir(home: Option<&Path>) -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = home {
        return home.join("Library").join("Logs").join("PromptForge");
    }

    #[cfg(target_os = "windows")]
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_app_data)
            .join("PromptForge")
            .join("logs");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    if let Ok(state_home) = std::env::var("XDG_STATE_HOME") {
        if !state_home.trim().is_empty() {
            return PathBuf::from(state_home).join("promptforge").join("logs");
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    if let Some(home) = home {
        return home
            .join(".local")
            .join("state")
            .join("promptforge")
            .join("logs");
    }

    match home {
        Some(home) => home.join(".promptforge").join("logs"),
        None => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_platform_log_dir_appends_expected_hierarchy() {
        let fake_home = Path::new("/mock/home/user");
        let resolved = resolve_platform_log_dir(Some(fake_home));
        #[cfg(target_os = "macos")]
        assert_eq!(
            resolved,
            PathBuf::from("/mock/home/user/Library/Logs/PromptForge")
        );
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert!(resolved.ends_with("promptforge/logs"));
    }

    #[test]
    fn fallback_with_no_home_returns_current_or_profile_dir() {
        let resolved = resolve_platform_log_dir(None);
        #[cfg(target_os = "macos")]
        assert_eq!(resolved, PathBuf::from("."));
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(resolved, PathBuf::from("."));
    }
}
