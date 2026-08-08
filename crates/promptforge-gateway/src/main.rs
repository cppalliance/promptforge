//! The `promptforge-gateway` binary: `promptforge-gateway serve <gateway.toml>`.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use promptforge_gateway::config::Config;
use promptforge_gateway::local::LocalRuntime;
use promptforge_gateway::routing::Routing;
use promptforge_gateway::{AppState, build_router};

/// Entry point. Builds the tokio runtime inside `main` (not via an attribute
/// macro) so the future service handler can construct it the same way.
fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    let mut args = std::env::args().skip(1);
    let Some("serve") = args.next().as_deref() else {
        eprintln!("usage: promptforge-gateway serve <gateway.toml>");
        return ExitCode::FAILURE;
    };
    let Some(path) = args.next() else {
        eprintln!("usage: promptforge-gateway serve <gateway.toml>");
        return ExitCode::FAILURE;
    };
    match serve(&path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Load the config, start local children, build the router, and serve.
fn serve(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load(Path::new(path))?;
    // Held until serve returns so Drop kills every llama-server child.
    let local = LocalRuntime::start(&config)?;
    let routing = Arc::new(Routing::from_config(&config)?.merge(local.models().iter().cloned())?);
    let bind = config.server.bind;
    let web_search = config.tools.and_then(|tools| tools.web_search);
    let mut state = AppState::new(routing, config.server.token);
    if let Some(cfg) = &web_search {
        state = state.with_web_search(cfg);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(bind).await?;
        tracing::info!("promptforge-gateway serving on {bind}");
        axum::serve(listener, build_router(state))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    drop(local);
    Ok(())
}
