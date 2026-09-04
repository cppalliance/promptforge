// Release builds are a GUI app: no console window when launched from the
// installer. Debug builds keep the console so the eprintln diagnostics show.
// The tradeoff: in release those diagnostics (boot errors) have nowhere to
// print.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The `promptforge-workshop` binary: the PromptForge Workshop desktop app.
//!
//! Hosts the workshop server in-process on a loopback listener with an
//! OS-assigned port - the server resolves the gateway endpoint itself,
//! attaching to a running gateway through its connection file or to the
//! gateway a discovered `workshop.toml` names - and opens a Tauri window
//! pointed at the in-process listener's URL. Closing the window stops the
//! in-process server only: the gateway is a separate process and keeps
//! running. With no gateway running and no explicit config, boot fails
//! with the plain no-gateway error. Development against the standalone
//! `workshop-server` binary flow is unchanged.

// The only unsafe module in the crate: the WebView2 COM surface that
// reads real OS paths out of dropped File objects and grants the
// microphone has no safe wrapper.
#[cfg(target_os = "windows")]
#[expect(unsafe_code, reason = "raw WebView2 COM has no safe wrapper")]
mod bridge;
mod config;
mod drops;
#[cfg(target_os = "linux")]
mod linux_media;
mod navigation;

use std::ffi::OsStr;
use std::process::ExitCode;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context as _;
use tauri::ipc::CapabilityBuilder;
use tauri::{Manager as _, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt as _;
use workshop_server::ServerHandle;

/// How long the app waits for the in-process server's health endpoint
/// before giving up.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);

/// The managed slot holding the server until the `RunEvent::Exit` handler
/// shuts it down. `shutdown` consumes the handle, so the slot hands it over
/// exactly once.
type ServerSlot = Mutex<Option<ServerHandle>>;

/// The permission set the workshop page holds. The grant itself is built
/// in setup with the exact bound port: the OS assigns the port at boot, so
/// no capability file can name the origin.
const WINDOW_PERMISSIONS: &[&str] = &[
    "core:default",
    "core:window:default",
    "core:window:allow-minimize",
    "core:window:allow-close",
    "core:window:allow-start-dragging",
    "core:window:allow-toggle-maximize",
    "core:webview:allow-set-webview-zoom",
    "core:event:default",
    "dialog:allow-open",
    "updater:default",
    "process:allow-restart",
];

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

/// `promptforge-workshop --version`: print the version and exit, without
/// booting the server or opening a window. The release workflows smoke-test
/// the installed package with it. The release build is GUI-subsystem on
/// Windows, but a piped or inherited stdout still carries the line.
fn print_version() -> ExitCode {
    println!("promptforge-workshop {}", env!("CARGO_PKG_VERSION"));
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
        // its own setup could spawn a second in-process server and race
        // the first instance's gateway attach.
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
            let server = handle
                .try_state::<ServerSlot>()
                .map(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take());
            if let Some(Some(server)) = server {
                match server.shutdown() {
                    Ok(workshop_server::Termination::Graceful) => {}
                    Ok(termination) => {
                        eprintln!("the workshop server was forced down past its drain window: {termination:?}");
                    }
                    Err(error) => {
                        eprintln!("could not shut the workshop server down cleanly: {error:?}");
                    }
                }
            }
        }
    });
    Ok(())
}

/// The setup hook: boots the in-process server, installs the window's
/// exact-port capability, and opens the window on the server's URL. A
/// boot failure prints its full error chain and exits the process
/// directly: returning `Err` would surface as Tauri's "Failed to setup
/// app" panic, losing the chain and the failure exit code.
fn boot_and_open(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    match boot() {
        Ok((server, url)) => {
            // The capability must exist before the window does: the
            // authority resolves a window's grants at creation.
            app.add_capability(window_capability(&url))?;
            app.manage(ServerSlot::new(Some(server)));
            open_window(app, &url)
        }
        Err(error) => {
            eprintln!("{error:?}");
            std::process::exit(1);
        }
    }
}

/// Spawns the in-process workshop server - it resolves the gateway
/// endpoint itself, connection file first, explicit `workshop.toml`
/// config second - and waits out its health probe. A failure after the
/// spawn shuts the server down before propagating.
fn boot() -> anyhow::Result<(ServerHandle, url::Url)> {
    let config = config::load().context("load the workshop configuration")?;
    let server = workshop_server::spawn(config).context("start the in-process workshop server")?;
    match shared_sidecar::wait_for_health(server.url(), HEALTH_TIMEOUT)
        .context("wait for the in-process workshop server")
    {
        Ok(()) => {
            let url = url::Url::parse(server.url()).context("parse the workshop URL")?;
            Ok((server, url))
        }
        Err(error) => {
            if let Err(shutdown_error) = server.shutdown() {
                eprintln!("{shutdown_error:?}");
            }
            Err(error)
        }
    }
}

/// The `main` window's capability: the permission set granted to the
/// exact loopback origin the in-process server bound. Building it in
/// setup is what lets the bind stay an OS-assigned port - a capability
/// file could name only a wildcard port, which would hand the window's
/// Tauri API surface to any other loopback server on the machine.
fn window_capability(url: &url::Url) -> CapabilityBuilder {
    let builder = CapabilityBuilder::new("workshop-main")
        .local(false)
        .remote(url.origin().ascii_serialization())
        .window("main");
    WINDOW_PERMISSIONS
        .iter()
        .fold(builder, |builder, permission| {
            builder.permission(*permission)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    use tauri::ipc::RuntimeCapability as _;
    use tauri::utils::acl::capability::CapabilityFile;

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
    fn the_window_capability_names_the_exact_bound_origin() {
        let url = url::Url::parse("http://127.0.0.1:49152/").expect("the URL parses");
        let CapabilityFile::Capability(capability) = window_capability(&url).build() else {
            panic!("the builder produces a single capability");
        };
        assert!(
            !capability.local,
            "the window loads no local content; the grant is remote-only"
        );
        let remote = capability.remote.expect("the remote origins are set");
        assert_eq!(
            remote.urls,
            vec!["http://127.0.0.1:49152".to_string()],
            "the grant names the exact bound origin - a wildcard port would hand \
             the Tauri API surface to any loopback server"
        );
        assert_eq!(capability.windows, vec!["main".to_string()]);
        let permissions = format!("{:?}", capability.permissions);
        for permission in WINDOW_PERMISSIONS {
            assert!(
                permissions.contains(permission),
                "the capability carries {permission}"
            );
        }
    }
}
