//! The `promptforge-gateway` binary:
//! `promptforge-gateway serve [--profiles-dir DIR] [--profile NAME] [config.toml]`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use promptforge_gateway::config::Config;
use promptforge_gateway::local::LocalRuntime;
use promptforge_gateway::profile::{self, default_profiles_dir};
use promptforge_gateway::routing::Routing;
use promptforge_gateway::{AppState, build_router};

const USAGE: &str =
    "usage: promptforge-gateway serve [--profiles-dir DIR] [--profile NAME] [config.toml]";

/// Entry point. Builds the tokio runtime inside `main` (not via an attribute
/// macro) so the future service handler can construct it the same way.
fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    let mut args = std::env::args().skip(1);
    let Some("serve") = args.next().as_deref() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };

    let mut profiles_dir = None;
    let mut profile = None;
    let mut config_path = None;
    let mut rest = args.peekable();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--profiles-dir" => {
                let Some(dir) = rest.next() else {
                    eprintln!("error: --profiles-dir requires a directory path");
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                };
                profiles_dir = Some(PathBuf::from(dir));
            }
            "--profile" => {
                let Some(name) = rest.next() else {
                    eprintln!("error: --profile requires a name");
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                };
                profile = Some(name);
            }
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("error: unknown flag {other}");
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            other => {
                if config_path.is_some() {
                    eprintln!("error: unexpected argument {other}");
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
                config_path = Some(PathBuf::from(other));
            }
        }
    }

    if profile.is_none() && config_path.is_none() {
        eprintln!("error: provide --profile NAME and/or a config.toml path");
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }

    match serve(ServeArgs {
        profiles_dir: profiles_dir.unwrap_or_else(default_profiles_dir),
        profile,
        config_path,
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

struct ServeArgs {
    profiles_dir: PathBuf,
    profile: Option<String>,
    config_path: Option<PathBuf>,
}

/// Load the config, start local children, build the router, and serve.
fn serve(args: ServeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (config, profile_name) = load_startup_config(&args)?;
    let local = LocalRuntime::start(&config)?;
    let routing = Arc::new(Routing::from_config(&config)?.merge(local.models().iter().cloned())?);
    let bind = config.server.bind;
    let web_search = config
        .tools
        .as_ref()
        .and_then(|tools| tools.web_search.as_ref());
    let state = AppState::from_parts(
        routing,
        config.server.token,
        local,
        web_search,
        Some(args.profiles_dir),
        profile_name,
    );

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
    Ok(())
}

fn load_startup_config(
    args: &ServeArgs,
) -> Result<(Config, Option<String>), Box<dyn std::error::Error>> {
    if let Some(name) = &args.profile {
        if let Some(path) = &args.config_path {
            return Err(format!(
                "pass either --profile or a config path, not both (got --profile {name} and {})",
                path.display()
            )
            .into());
        }
        let config = profile::load_named(&args.profiles_dir, name)?;
        return Ok((config, Some(name.clone())));
    }

    let path = args.config_path.as_deref().ok_or("missing config path")?;
    let config = Config::load(Path::new(path))?;
    let name = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned);
    Ok((config, name))
}
