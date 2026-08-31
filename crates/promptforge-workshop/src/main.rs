//! The `promptforge-workshop` binary: the PromptForge Workshop desktop app.
//!
//! Loads the gateway boot config `gateway.toml` (see [`discover`] for the
//! search order), generating a default config with its `default` profile in
//! the user profile's `.promptforge` directory on first run, boots the
//! merged gateway (which hosts the workshop UI on a second loopback
//! listener) in-process, waits for the workshop's health endpoint to
//! answer, and opens a Tauri window pointed at it. Closing the window exits
//! the app and shuts the gateway down cleanly. Development against an
//! external gateway uses the standalone `promptforge-workshop-server`
//! binary and its `workshop.toml` instead of this app.

// The only unsafe module in the crate: the WebView2 COM surface that
// reads real OS paths out of dropped File objects and grants the
// microphone has no safe wrapper.
#[cfg(target_os = "windows")]
#[expect(unsafe_code, reason = "raw WebView2 COM has no safe wrapper")]
mod bridge;
mod discover;
mod drops;
mod health;
#[cfg(target_os = "linux")]
mod linux_media;
mod navigation;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context as _;
use promptforge_gateway::{GatewayHandle, ProfileName, ServeOptions};
use tauri::{Manager as _, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt as _;

/// How long the app waits for the hosted workshop's health endpoint
/// before giving up.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);

/// The managed slot holding the gateway until the `RunEvent::Exit` handler
/// shuts it down. `shutdown` consumes the handle, so the slot hands it over
/// exactly once.
type GatewaySlot = Mutex<Option<GatewayHandle>>;

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
    let builder = tauri::Builder::default()
        // Registered first so a second launch exits inside `build`, before
        // its own setup could spawn a gateway onto the fixed port bind.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .setup(boot_and_open);
    // Off Windows, OS file drops arrive as Tauri's own drag-drop event and
    // are dispatched into the same promptforge:file-drop grant flow. On
    // Windows the drag-drop handler is disabled (see open_window) and drops
    // arrive over the WebView2 web-message bridge instead.
    #[cfg(not(target_os = "windows"))]
    let builder = builder.on_window_event(|window, event| {
        if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event
            && let Some(webview) = window.get_webview_window(window.label())
        {
            drops::dispatch_file_drop(&webview, paths);
        }
    });
    let app = builder
        .build(tauri::generate_context!())
        .context("build the desktop application")?;
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            let gateway = handle
                .try_state::<GatewaySlot>()
                .map(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take());
            if let Some(Some(gateway)) = gateway
                && let Err(error) = gateway.shutdown()
            {
                eprintln!("could not shut the gateway down cleanly: {error:?}");
            }
        }
    });
    Ok(())
}

/// The setup hook: boots the gateway, waits for the hosted workshop's
/// health, and opens the window on it. A boot failure prints its full
/// error chain and exits the process directly: returning `Err` would
/// surface as Tauri's "Failed to setup app" panic, losing the chain and
/// the failure exit code.
fn boot_and_open(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    match boot() {
        Ok((gateway, url)) => {
            app.manage(GatewaySlot::new(Some(gateway)));
            open_window(app, &url)
        }
        Err(error) => {
            eprintln!("{error:?}");
            std::process::exit(1);
        }
    }
}

/// Discovers or generates the boot config, spawns the merged gateway, and
/// waits out the hosted workshop's health probe. A failure after the spawn
/// shuts the gateway down before propagating.
fn boot() -> anyhow::Result<(GatewayHandle, url::Url)> {
    let config_path = match discover::discover_config()? {
        Some(config_path) => config_path,
        None => generate_in_profile()?,
    };
    let profile =
        ProfileName::parse(discover::DEFAULT_PROFILE).context("parse the default profile name")?;
    let gateway = promptforge_gateway::spawn(&ServeOptions::new(config_path, profile))
        .context("start the merged gateway")?;
    match workshop_url(&gateway).and_then(|url| {
        health::wait_for_health(&url, HEALTH_TIMEOUT)
            .context("wait for the hosted workshop")
            .map(|()| url)
    }) {
        Ok(url) => {
            let parsed = url::Url::parse(&url).context("parse the workshop URL")?;
            Ok((gateway, parsed))
        }
        Err(error) => {
            if let Err(shutdown_error) = gateway.shutdown() {
                eprintln!("{shutdown_error:?}");
            }
            Err(error)
        }
    }
}

/// Creates the workshop window on `url`: hidden while built so the
/// window-state restore never flashes, undecorated on Windows where the
/// custom HTML title bar replaces the native frame (macOS and Linux keep
/// their decorated windows), then shown.
fn open_window(app: &mut tauri::App, url: &url::Url) -> Result<(), Box<dyn std::error::Error>> {
    let server_origin = url.origin();
    let opener = app.handle().clone();
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.clone()))
        .title("PromptForge")
        .inner_size(1024.0, 768.0)
        .visible(false)
        .on_navigation(move |target| {
            match navigation::classify_navigation(&server_origin, target.as_str()) {
                navigation::Navigation::Allow => true,
                navigation::Navigation::OpenExternally => {
                    if let Err(error) = opener.opener().open_url(target.as_str(), None::<&str>) {
                        eprintln!("could not open {target} in the system browser: {error}");
                    }
                    false
                }
            }
        });
    // Tauri's drag-drop handler suppresses HTML5 drag events on Windows
    // (https://github.com/tauri-apps/tauri/issues/15138), so Windows keeps
    // the handler off and takes Explorer drops over the WebView2
    // web-message bridge instead (bridge.rs).
    #[cfg(target_os = "windows")]
    let builder = builder.disable_drag_drop_handler();
    let window = builder.build()?;
    // An attach failure only degrades Explorer drops and the mic grant,
    // never the app.
    #[cfg(target_os = "windows")]
    if let Err(error) = bridge::attach(&window) {
        eprintln!(
            "could not attach the WebView2 bridge; Explorer drops and the microphone grant are unavailable: {error}"
        );
    }
    #[cfg(target_os = "linux")]
    if let Err(error) = linux_media::grant_media_permissions(&window) {
        eprintln!(
            "could not enable WebKitGTK media capture; the microphone is unavailable: {error}"
        );
    }
    // Decorations come off after construction, not at build time: the
    // Groupy-compatible title bar pattern proven by other Tauri apps.
    #[cfg(target_os = "windows")]
    window.set_decorations(false)?;
    window.show()?;
    Ok(())
}

/// The hosted workshop's URL from the gateway handle. The app compiles
/// the gateway's `workshop` feature in, so `None` means the discovered
/// boot config carries no `[workshop]` section - a configuration the
/// app has no page to open a window on.
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

/// First run: writes `gateway.toml` with its `default` profile into the user
/// profile's `.promptforge` directory.
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
