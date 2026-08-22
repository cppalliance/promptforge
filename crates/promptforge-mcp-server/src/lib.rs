//! PromptForge MCP server.
//!
//! Runs PromptForge prompts for an agentic harness (Cursor, Claude Code). A
//! prompt is a command: it runs because a caller named it to `run_prompt`, so
//! no prompt is published as a tool of its own and `tools/list` is the same
//! fixed set of built-ins whatever the catalog holds. Execution happens here,
//! against the gateway, so a prompt is never an MCP prompt.
//!
//! [`Config`] parses the `prompts.toml` that names the bind address, the shared
//! token, the prompts directory, the gateway, and which prompts the harness sees;
//! [`Catalog::resolve`] turns that configuration and the prompts directory into
//! the set of prompts a harness may call, either refusing to start over an
//! incomplete result or keeping the failures visible as broken entries;
//! [`PreparedTools::load`] binds the gateway model catalog and the tool picker
//! once at boot; and [`PromptForgeServer`] runs a call to completion against the
//! gateway, reports its progress, and admits the run so a call that outlasts the
//! client's patience is collectable by `run_id` afterwards. `serve_http` and
//! `serve_stdio` are the two transports the handler is reached on: the first
//! puts it behind a shared bearer at `/mcp` with an unauthenticated `/healthz`
//! beside it, the second speaks JSON-RPC over standard input and output.
//! [`Watcher`] keeps the catalog current while the server runs, so writing a
//! prompt is an edit-and-call loop rather than an edit-restart-call one: no
//! client is notified and none needs to be, since the tool list never moves and
//! every call reads the catalog fresh. `Retrieval` is what answers
//! `need_prompt`: a plain-English capability in, up to three candidate prompts
//! out, rebuilt on the same swap when a save changed what it ranks on.

#![deny(unreachable_pub)]

mod catalog;
mod config;
mod error;
mod generation;
#[cfg(test)]
mod levels;
mod progress;
mod registry;
mod relpath;
mod result;
mod retrieval;
mod server;
mod tools;
mod transport;
mod watch;

pub use crate::catalog::{Catalog, CatalogHandle, OnBroken};
pub use crate::config::Config;
pub use crate::error::{
    CatalogError, CatalogErrorKind, ConfigError, ConfigErrorKind, FaultKind, FaultRef, Faults,
    PreparedToolsError, PreparedToolsErrorKind, RunError, RunErrorKind, WatchError, WatchErrorKind,
};
pub(crate) use crate::retrieval::Retrieval;
pub use crate::server::{PreparedTools, PromptForgeServer};
pub(crate) use crate::transport::{serve_http, serve_stdio};
pub use crate::watch::Watcher;

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One parsed command line: which transport to serve and which file to read.
///
/// The configuration path is a typed [`PathBuf`] rather than a `String`, so a
/// non-UTF-8 path survives from the argument vector to [`Config::load`] without
/// a lossy round trip, and the transport choice is a `bool` rather than a flag
/// re-parsed downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerArgs {
    /// Serve over standard input and output rather than binding a port.
    stdio: bool,
    /// The configuration file to load.
    config: PathBuf,
}

impl ServerArgs {
    /// The command line the process takes: `serve <config>` or
    /// `serve --stdio <config>`, and nothing else.
    ///
    /// The arguments are the process arguments with the program name already
    /// dropped. A shape that is not one of the two accepted forms - a missing
    /// subcommand, a missing configuration, a flag out of position, or a
    /// trailing extra argument - returns `None` so the binary can print its
    /// usage and exit cleanly rather than panic. The configuration token is
    /// taken as an [`OsString`](std::ffi::OsString) and kept verbatim, so a path
    /// the platform allows but Unicode does not is preserved.
    #[must_use]
    pub fn parse<I>(arguments: I) -> Option<ServerArgs>
    where
        I: IntoIterator<Item = std::ffi::OsString>,
    {
        let mut rest = arguments.into_iter();
        if rest.next()?.to_str() != Some("serve") {
            return None;
        }
        let first = rest.next()?;
        let (stdio, config) = if first.to_str() == Some("--stdio") {
            (true, rest.next()?)
        } else {
            (false, first)
        };
        if rest.next().is_some() {
            return None;
        }
        Some(ServerArgs {
            stdio,
            config: PathBuf::from(config),
        })
    }

    /// Whether the process serves over stdio rather than binding a port. Read by
    /// the binary to route logging off standard output before the runtime.
    #[must_use]
    pub fn stdio(&self) -> bool {
        self.stdio
    }

    /// The configuration file this invocation names.
    #[must_use]
    pub fn config(&self) -> &Path {
        &self.config
    }
}

/// Loads the configuration, resolves the catalog, prepares the tools, and
/// serves the chosen transport until the process is stopped.
///
/// This is the whole of the boot sequence, kept in the library so the binary is
/// a thin shell that only parses arguments, routes logging, and renders a
/// failure. Boot either produces a complete catalog or refuses to serve: an
/// incomplete catalog is rejected here rather than started with prompts
/// silently missing.
///
/// # Errors
/// Returns a [`RunError`] classified by [`kind`](RunError::kind) for the first
/// boot step that could not proceed: loading the configuration, resolving the
/// catalog, preparing the tools, starting the watcher, building the runtime, or
/// serving.
pub fn run(args: &ServerArgs) -> Result<(), RunError> {
    let source = args.config.as_path();
    let config = Config::load(source)?;
    // Boot refuses an incomplete catalog: a service that starts with nine of
    // ten prompts is one whose catalog silently disagrees with its own
    // configuration, and a client sees only a missing tool.
    let catalog = Catalog::resolve(&config, OnBroken::Reject)?;
    // Prepare the optional prompt-retrieval index before the runtime for the
    // same blocking-CPU reason. Unlike the required execution picker below, a
    // failed retrieval index costs `need_prompt` and nothing else. The catalog
    // and the index over it are bound into one live generation, so the watcher
    // replaces both together and no reader sees a torn pair.
    let retrieval = Retrieval::start(&catalog);
    let config = Arc::new(config);
    let catalog = Arc::new(CatalogHandle::with_retrieval(catalog, retrieval));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(RunError::runtime)?;
    // Tool/model capability binding is synchronous and model-backed. Prepare
    // the live tool catalog, picker, and gateway model catalog once, then share
    // the immutable result across every run.
    let tools = Arc::new(runtime.block_on(PreparedTools::load(&config))?);
    let stdio = args.stdio;
    runtime.block_on(async move {
        // Started inside the runtime, because the debounce window is a task, and
        // held for as long as the transport serves: dropping the guard stops the
        // watches.
        let _watcher = Watcher::start(source, Arc::clone(&config), Arc::clone(&catalog))?;
        if stdio {
            serve_stdio(config, catalog, tools, shutdown_signal()).await?;
        } else {
            serve_http(config, catalog, tools, shutdown_signal()).await?;
        }
        Ok::<(), RunError>(())
    })?;
    Ok(())
}

/// Resolves when the process is asked to stop, which both transports take as
/// their cue to drain and close. Ctrl-C is the one signal handled portably
/// across the platforms this binary runs on; a service manager that sends it
/// gets the same graceful path an operator's keystroke does. A listener that
/// cannot be installed is logged and then never resolves, so a failure to arm
/// the handler degrades to "serve until killed" rather than an immediate exit.
async fn shutdown_signal() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => tracing::info!("shutdown signal received; draining"),
        Err(error) => {
            tracing::error!("listen for the shutdown signal: {error}");
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::ServerArgs;

    /// `ServerArgs::parse` over borrowed string arguments, which is how a test
    /// spells a command line.
    fn parse(arguments: &[&str]) -> Option<ServerArgs> {
        ServerArgs::parse(arguments.iter().map(OsString::from))
    }

    #[test]
    fn parses_the_http_shape() {
        let args = parse(&["serve", "prompts.toml"]).expect("serve <config> is accepted");
        assert!(!args.stdio);
        assert_eq!(args.config, std::path::Path::new("prompts.toml"));
    }

    #[test]
    fn parses_the_stdio_shape() {
        let args = parse(&["serve", "--stdio", "prompts.toml"]).expect("serve --stdio <config>");
        assert!(args.stdio);
        assert_eq!(args.config, std::path::Path::new("prompts.toml"));
    }

    #[test]
    fn rejects_a_flag_after_the_config() {
        // The position is part of the shape: `--stdio` is the second argument or
        // it is not the flag, since a configuration may legitimately be named
        // anything.
        assert!(parse(&["serve", "prompts.toml", "--stdio"]).is_none());
    }

    #[test]
    fn rejects_a_trailing_extra_argument() {
        assert!(parse(&["serve", "prompts.toml", "extra"]).is_none());
        assert!(parse(&["serve", "--stdio", "prompts.toml", "extra"]).is_none());
    }

    #[test]
    fn rejects_a_missing_config_and_a_missing_subcommand() {
        assert!(parse(&[]).is_none());
        assert!(parse(&["serve"]).is_none());
        assert!(parse(&["serve", "--stdio"]).is_none());
        assert!(parse(&["run", "prompts.toml"]).is_none());
        assert!(parse(&["--stdio", "prompts.toml"]).is_none());
    }
}
