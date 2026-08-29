//! The `promptforge-workshop` binary: the PromptForge Workshop desktop app.
//!
//! Loads the gateway boot config `gateway.toml` (see [`discover`] for the
//! search order), generating a default config and its `default` profile in
//! the user profile's `.promptforge` directory on first run, boots the
//! merged gateway (which hosts the workshop UI on a second loopback
//! listener) in-process, waits for the workshop's health endpoint to
//! answer, and opens a window pointed at it through
//! [`promptforge_desktop_shell::run`]. Closing the window shuts the
//! gateway down cleanly. Development against an external gateway uses the
//! standalone `promptforge-workshop-server` binary and its `workshop.toml`
//! instead of this shell.

mod discover;
mod health;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context as _;
use promptforge_gateway::{GatewayHandle, ProfileName, ServeOptions};

/// How long the shell waits for the hosted workshop's health endpoint
/// before giving up.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let config_path = match discover::discover_config()? {
        Some(config_path) => config_path,
        None => generate_in_profile()?,
    };
    let profile =
        ProfileName::parse(discover::DEFAULT_PROFILE).context("parse the default profile name")?;
    let gateway = promptforge_gateway::spawn(&ServeOptions::new(config_path, profile))
        .context("start the merged gateway")?;

    // The gateway is shut down whether the window ran, failed, or never
    // opened because the health probe timed out.
    let window_result = workshop_url(&gateway).and_then(|url| {
        health::wait_for_health(&url, HEALTH_TIMEOUT)
            .context("wait for the hosted workshop")
            .and_then(|()| promptforge_desktop_shell::run(&url))
    });
    let shutdown_result = gateway.shutdown().context("stop the gateway");
    // A shutdown failure stacked on a window failure is reported, not lost.
    if let (Err(_), Err(shutdown_error)) = (&window_result, &shutdown_result) {
        eprintln!("{shutdown_error:?}");
    }
    window_result.and(shutdown_result)
}

/// The hosted workshop's URL from the gateway handle. The shell compiles
/// the gateway's `workshop` feature in, so `None` means the discovered
/// boot config carries no `[workshop]` section - a configuration the
/// shell has no page to open a window on.
fn workshop_url(gateway: &GatewayHandle) -> anyhow::Result<String> {
    workshop_url_from(gateway.workshop_url())
}

/// Maps the gateway's optional hosted-workshop URL to the URL the window
/// opens, or to the user-facing error for a boot config with no
/// `[workshop]` section.
fn workshop_url_from(url: Option<&str>) -> anyhow::Result<String> {
    url.map(str::to_string).context(
        "the boot config has no [workshop] section, so the gateway hosts no workshop UI; \
         add a [workshop] section to gateway.toml",
    )
}

/// First run: writes the default `gateway.toml` and its `default` profile
/// into the user profile's `.promptforge` directory.
fn generate_in_profile() -> anyhow::Result<PathBuf> {
    let home = std::env::home_dir().context("locate the user profile directory")?;
    let path = discover::profile_config_path(&home);
    let path = discover::generate_default(&path).context("write the default configuration")?;
    eprintln!(
        "no gateway.toml found; wrote default config to {}",
        path.display()
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_is_named_promptforge_workshop() {
        assert_eq!(env!("CARGO_PKG_NAME"), "promptforge-workshop");
    }

    #[test]
    fn workshop_url_from_passes_the_url_through() {
        assert_eq!(
            workshop_url_from(Some("http://127.0.0.1:7910/")).unwrap(),
            "http://127.0.0.1:7910/"
        );
    }

    #[test]
    fn workshop_url_from_names_the_missing_workshop_section() {
        let error = workshop_url_from(None).unwrap_err();
        assert!(
            error.to_string().contains("[workshop]"),
            "the error must tell the user which section to add, got: {error}"
        );
    }
}
