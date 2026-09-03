// Release builds are a GUI app: no console window when launched from the
// installer. Debug builds keep the console so the eprintln diagnostics show.
// The tradeoff: in release those diagnostics (boot errors, the first-run
// config notice) have nowhere to print.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The `workshop` binary: the PromptForge Workshop desktop app.
//!
//! Loads the gateway boot config `gateway.toml` (see [`discover`] for the
//! search order), generating a default config with its `default` profile in
//! the user profile's `.promptforge` directory on first run, boots the
//! merged gateway (which hosts the workshop UI on a second loopback
//! listener) in-process, waits for the workshop's health endpoint to
//! answer, and opens a Tauri window pointed at it. Closing the window exits
//! the app and shuts the gateway down cleanly. Development against an
//! external gateway uses the standalone `workshop-server`
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

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context as _;
use gateway::{GatewayHandle, ProfileName, ServeOptions};
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
    if std::env::args_os().any(|arg| arg == "--version") {
        return print_version();
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            ExitCode::FAILURE
        }
    }
}

/// `workshop --version`: print the version and exit, without
/// booting the gateway or opening a window. The release workflows smoke-test
/// the installed package with it. The release build is GUI-subsystem on
/// Windows, but a piped or inherited stdout still carries the line.
fn print_version() -> ExitCode {
    println!("workshop {}", env!("CARGO_PKG_VERSION"));
    ExitCode::SUCCESS
}

fn update_supported(os: &str, appimage: Option<&OsStr>) -> bool {
    os != "linux" || appimage.is_some_and(|value| !value.is_empty())
}

#[tauri::command]
fn desktop_update_supported() -> bool {
    update_supported(
        std::env::consts::OS,
        std::env::var_os("APPIMAGE").as_deref(),
    )
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
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .invoke_handler(tauri::generate_handler![desktop_update_supported])
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

/// The embedded startup splash document displayed while the in-process gateway
/// boots and provisions its initial model assets.
const LOADING_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>PromptForge</title>
<style>
  :root {
    --bg: #121214;
    --text: #e4e4e7;
    --text-muted: #71717a;
    --accent: #22c55e;
    --track: #27272a;
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body {
    background: var(--bg);
    color: var(--text);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    height: 100vh;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    user-select: none;
    -webkit-user-select: none;
    overflow: hidden;
  }
  .container {
    display: flex;
    flex-direction: column;
    align-items: center;
    max-width: 380px;
    width: 100%;
    padding: 32px 24px;
    text-align: center;
  }
  .logo {
    font-size: 26px;
    font-weight: 600;
    letter-spacing: -0.5px;
    margin-bottom: 28px;
    color: #ffffff;
  }
  .progress-track {
    width: 100%;
    height: 4px;
    background: var(--track);
    border-radius: 2px;
    overflow: hidden;
    margin-bottom: 18px;
    position: relative;
  }
  .progress-bar {
    position: absolute;
    top: 0;
    left: 0;
    bottom: 0;
    background: var(--accent);
    box-shadow: 0 0 10px rgba(34, 197, 94, 0.5);
    border-radius: 2px;
    animation: indeterminate 1.8s infinite ease-in-out;
  }
  @keyframes indeterminate {
    0% { left: -40%; width: 40%; }
    50% { left: 20%; width: 60%; }
    100% { left: 100%; width: 40%; }
  }
  .status {
    font-size: 13px;
    color: var(--text-muted);
    line-height: 1.5;
  }
</style>
</head>
<body>
  <div class="container">
    <div class="logo">PromptForge</div>
    <div class="progress-track">
      <div class="progress-bar"></div>
    </div>
    <div class="status">Starting PromptForge gateway & initializing models...</div>
  </div>
</body>
</html>"#;

/// Formats the embedded loading HTML into a data URL for initial webview presentation.
fn loading_url() -> anyhow::Result<url::Url> {
    let encoded =
        percent_encoding::utf8_percent_encode(LOADING_HTML, percent_encoding::NON_ALPHANUMERIC);
    let raw = format!("data:text/html;charset=utf-8,{encoded}");
    url::Url::parse(&raw).context("parse the splash screen data URL")
}

/// The shared origin slot holding the server origin once known, for navigation filtering.
type OriginSlot = std::sync::Arc<Mutex<Option<url::Origin>>>;

/// The setup hook: creates the application window immediately with the startup
/// splash screen, then boots the gateway on a background thread. Once the gateway's
/// health probe answers, the window navigates to the hosted workshop URL.
fn boot_and_open(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let origin_slot = create_window(app)?;
    let app_handle = app.handle().clone();
    std::thread::Builder::new()
        .name("workshop-boot".to_string())
        .spawn(move || match boot() {
            Ok((gateway, url)) => {
                app_handle.manage(GatewaySlot::new(Some(gateway)));
                let server_origin = url.origin();
                *origin_slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(server_origin);
                let handle_for_window = app_handle.clone();
                let _ = app_handle.run_on_main_thread(move || {
                    if let Some(window) = handle_for_window.get_webview_window("main") {
                        if let Err(error) = window.navigate(url) {
                            eprintln!("could not navigate to workshop: {error:?}");
                        }
                        let _ = window.set_focus();
                    }
                });
            }
            Err(error) => {
                eprintln!("{error:?}");
                show_boot_error(
                    &app_handle,
                    &format!("PromptForge failed to start:\n\n{error:#}"),
                );
                app_handle.exit(1);
            }
        })
        .context("spawn workshop boot thread")?;
    Ok(())
}

/// Shows a native error dialog on boot failure before exiting.
fn show_boot_error(handle: &tauri::AppHandle, message: &str) {
    use tauri_plugin_dialog::DialogExt as _;
    handle
        .dialog()
        .message(message)
        .title("PromptForge")
        .kind(tauri_plugin_dialog::MessageDialogKind::Error)
        .blocking_show();
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
    let gateway = gateway::spawn(&ServeOptions::new(config_path, profile))
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

/// Creates the workshop window immediately on launch: displays the startup
/// splash screen while built so the window-state restore never flashes, undecorated
/// on Windows where the custom HTML title bar replaces the native frame (macOS and
/// Linux keep their decorated windows), then shown and focused.
fn create_window(app: &mut tauri::App) -> Result<OriginSlot, Box<dyn std::error::Error>> {
    let initial_url = loading_url()?;
    let origin_slot: OriginSlot = std::sync::Arc::new(Mutex::new(None));
    let nav_slot = std::sync::Arc::clone(&origin_slot);
    let opener = app.handle().clone();
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(initial_url))
        .title("PromptForge")
        .inner_size(1024.0, 768.0)
        .visible(false)
        .on_navigation(move |target| {
            let allowed_origin = nav_slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(server_origin) = allowed_origin {
                match navigation::classify_navigation(&server_origin, target.as_str()) {
                    navigation::Navigation::Allow => true,
                    navigation::Navigation::OpenExternally => {
                        if let Err(error) = opener.opener().open_url(target.as_str(), None::<&str>)
                        {
                            eprintln!("could not open {target} in the system browser: {error}");
                        }
                        false
                    }
                }
            } else {
                if let Ok(parsed) = url::Url::parse(target.as_str())
                    && matches!(parsed.scheme(), "http" | "https")
                {
                    if let Err(error) = opener.opener().open_url(target.as_str(), None::<&str>) {
                        eprintln!("could not open {target} in the system browser: {error}");
                    }
                    return false;
                }
                true
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
    let _ = window.set_focus();
    Ok(origin_slot)
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
    fn crate_is_named_workshop() {
        assert_eq!(env!("CARGO_PKG_NAME"), "workshop");
    }

    #[test]
    fn linux_updates_require_an_appimage_runtime() {
        assert!(!update_supported("linux", None));
        assert!(!update_supported("linux", Some(OsStr::new(""))));
        assert!(update_supported(
            "linux",
            Some(OsStr::new("/tmp/PromptForge.AppImage"))
        ));
        assert!(update_supported("windows", None));
        assert!(update_supported("macos", None));
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
