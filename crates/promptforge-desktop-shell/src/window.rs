//! The desktop window: a tao event loop driving a wry webview pointed at
//! the in-process workshop server.
//!
//! There is no native menu: tao 0.36 moved menu support out to the `muda`
//! crate, and a one-item `File > Quit` menu does not justify the extra
//! dependency and its event channel. The window's close button is the quit
//! gesture. `run` uses tao's `run_return` so control comes back after the
//! loop exits and the caller can shut the server down cleanly.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Icon, Window, WindowBuilder};
#[cfg(not(target_os = "windows"))]
use wry::DragDropEvent;
use wry::{PermissionKind, PermissionResponse, WebView, WebViewBuilder};

/// The cold medallion program icon, embedded so the installed binary
/// carries no asset files. Frames 2-5 stay on disk for a future activity
/// animation.
const ICON_PNG: &[u8] = include_bytes!("../assets/icons/promptforge-icon-1.png");

/// What the shell does with a URL the webview wants to navigate to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Navigation {
    /// The webview loads the URL itself.
    Allow,
    /// The URL opens in the system browser; the webview stays put.
    OpenExternally,
}

/// Classifies a navigation target against the server's origin: a
/// same-origin http(s) URL (the in-process server) loads in the webview,
/// while any other absolute http(s) URL opens in the system browser. A
/// clicked link never navigates the app away from itself, and no other
/// loopback server - same machine, different port, or a different
/// loopback spelling - gets the shell's IPC bridge, folder picker, or
/// desktop flag. Other schemes (`about:blank`, `data:`) and unparseable
/// values are left to the webview.
#[must_use]
pub(crate) fn classify_navigation(server: &url::Origin, target: &str) -> Navigation {
    let Ok(url) = url::Url::parse(target) else {
        return Navigation::Allow;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Navigation::Allow;
    }
    if url.origin() == *server {
        Navigation::Allow
    } else {
        Navigation::OpenExternally
    }
}

/// A window command the custom title bar sends over the wry IPC bridge.
/// The IPC handler cannot touch the tao `Window`, so commands travel
/// through an `EventLoopProxy` and run on the event loop thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowCommand {
    /// Begin a native window drag (the title bar's empty center).
    Drag,
    /// Minimize the window.
    Minimize,
    /// Maximize the window, or restore it if already maximized.
    ToggleMaximize,
    /// Close the window; the loop exits and the server shuts down.
    Close,
}

/// A user event delivered to the tao event loop. The IPC, drag-drop, and
/// navigation handlers run on webview threads without access to the tao
/// `Window` or the `WebView`, so their payloads travel through an
/// `EventLoopProxy` and run on the event loop thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellEvent {
    /// A command from the custom title bar.
    Command(WindowCommand),
    /// Real OS paths dropped onto the webview from Explorer.
    FileDrop(Vec<PathBuf>),
    /// An external URL a denied navigation opens in the system browser.
    OpenExternal(String),
    /// The page asked for the native folder picker.
    PickFolder,
}

/// The web message the page posts to request the native folder picker.
/// It shares the channel with the title-bar envelopes and the file-drop
/// bridge's `workspace-drop` message (file_drop.rs).
const PICK_FOLDER_MESSAGE: &str = "workspace-pick-folder";

/// The deferred half of a navigation decision: a denied external URL
/// becomes an [`OpenExternal`](ShellEvent::OpenExternal) event for the
/// event loop to execute; an allowed navigation defers nothing.
#[must_use]
fn navigation_effect(classification: Navigation, target: String) -> Option<ShellEvent> {
    match classification {
        Navigation::Allow => None,
        Navigation::OpenExternally => Some(ShellEvent::OpenExternal(target)),
    }
}

/// Parses one IPC envelope (`{"command": "..."}`) into a window command.
/// Malformed JSON, missing or mistyped fields, and unknown command names
/// all return `None`, so unrecognized payloads can never reach a native
/// window operation.
#[must_use]
pub(crate) fn parse_window_command(payload: &str) -> Option<WindowCommand> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let command = value.get("command")?.as_str()?;
    match command {
        "drag" => Some(WindowCommand::Drag),
        "minimize" => Some(WindowCommand::Minimize),
        "toggle-maximize" => Some(WindowCommand::ToggleMaximize),
        "close" => Some(WindowCommand::Close),
        _ => None,
    }
}

/// Parses one message from the page's shared channel into the shell event
/// it requests: the bare [`PICK_FOLDER_MESSAGE`] string asks for the
/// native folder picker, and a `{"command": "..."}` envelope names a
/// title-bar window command. Anything else on the channel (the file-drop
/// bridge's own message included) parses to `None` and is ignored.
#[must_use]
pub(crate) fn parse_web_message(payload: &str) -> Option<ShellEvent> {
    if payload == PICK_FOLDER_MESSAGE {
        return Some(ShellEvent::PickFolder);
    }
    parse_window_command(payload).map(ShellEvent::Command)
}

/// Decodes a PNG into 32bpp RGBA pixels plus its dimensions. Only 8-bit
/// RGBA output is accepted, matching the bundled asset, so the pixel format
/// handed to `Icon::from_rgba` is fixed at compile time by the asset.
///
/// # Errors
/// Returns an error if the PNG cannot be decoded or is not 8-bit RGBA.
fn decode_png_rgba(png_bytes: &[u8]) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
        .read_info()
        .context("read the program icon header")?;
    let capacity = reader
        .output_buffer_size()
        .context("the program icon has no image frame")?;
    let mut pixels = vec![0; capacity];
    let info = reader
        .next_frame(&mut pixels)
        .context("decode the program icon frame")?;
    anyhow::ensure!(
        info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight,
        "the program icon must be 8-bit RGBA"
    );
    pixels.truncate(info.buffer_size());
    Ok((pixels, info.width, info.height))
}

/// Builds the tao window icon from the bundled PNG. On any decode or
/// conversion failure it logs and returns `None`, so a bad asset never
/// blocks startup - the window just keeps the OS default icon.
fn window_icon() -> Option<Icon> {
    let result = decode_png_rgba(ICON_PNG).and_then(|(pixels, width, height)| {
        Icon::from_rgba(pixels, width, height).map_err(Into::into)
    });
    match result {
        Ok(icon) => Some(icon),
        Err(error) => {
            eprintln!("could not load the program icon, using the default: {error}");
            None
        }
    }
}

/// Dispatches the `promptforge:maximized` event the title bar listens for,
/// keeping the maximize/restore glyph in sync. Every maximize path (button,
/// double-click, Windows Snap, restore) surfaces in the loop as a resize.
fn dispatch_maximized(webview: &WebView, maximized: bool) {
    let script = format!(
        "window.dispatchEvent(new CustomEvent(\"promptforge:maximized\", {{detail: {{maximized: {maximized}}}}}));"
    );
    if let Err(error) = webview.evaluate_script(&script) {
        eprintln!("could not dispatch the maximized-state event: {error}");
    }
}

/// Renders a dropped OS path for the browser event: the path's own text
/// with any Windows verbatim (`\\?\`) prefix stripped, so the page hands
/// the workspace API the same spelling Explorer shows. The verbatim UNC
/// form (`\\?\UNC\server\share`) collapses to the plain UNC spelling
/// (`\\server\share`). Separators stay native - the workspace server runs
/// on this same machine and canonicalizes whatever it receives.
#[must_use]
pub(crate) fn normalize_dropped_path(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    match text.strip_prefix(r"\\?\") {
        Some(stripped) => stripped.to_owned(),
        None => text,
    }
}

/// Dispatches the `promptforge:file-drop` event carrying the normalized
/// dropped paths. The page grants each path through the workspace HTTP
/// API; the shell never reads file bytes merely because a file was
/// dragged onto the window.
fn dispatch_file_drop(webview: &WebView, paths: &[PathBuf]) {
    let normalized: Vec<String> = paths
        .iter()
        .map(|path| normalize_dropped_path(path))
        .collect();
    let detail = match serde_json::to_string(&normalized) {
        Ok(detail) => detail,
        Err(error) => {
            eprintln!("could not encode the dropped paths: {error}");
            return;
        }
    };
    let script = format!(
        "window.dispatchEvent(new CustomEvent(\"promptforge:file-drop\", {{detail: {{paths: {detail}}}}}));"
    );
    if let Err(error) = webview.evaluate_script(&script) {
        eprintln!("could not dispatch the file-drop event: {error}");
    }
}

/// Opens the native folder picker, modal to the workshop window, and
/// returns the chosen folder, or `None` when the user cancels.
///
/// The dialog is rfd's synchronous `IFileDialog`, shown on the event loop
/// thread. That blocks this loop iteration, but never the message pump:
/// the modal runs its own native pump for the whole thread, and the
/// WebView2 message handler that requested the pick already returned when
/// the request travelled through the `EventLoopProxy` - the same deferral
/// the file-drop bridge uses - so no webview callback is ever on the
/// stack under the modal.
#[cfg(target_os = "windows")]
fn pick_folder(window: &Window) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Add Folder to Workspace")
        .set_parent(window)
        .pick_folder()
}

/// The folder picker on platforms without the WebView2 web-message
/// channel: nothing asks for it there, so an unexpected request logs and
/// answers as a cancel.
#[cfg(not(target_os = "windows"))]
fn pick_folder(_window: &Window) -> Option<PathBuf> {
    eprintln!("the native folder picker is not wired on this platform");
    None
}

/// The JSON payload for the `promptforge:folder-picked` event: the chosen
/// path, normalized like a dropped path and JSON-encoded so backslashes
/// and quotes survive the trip into the page's event detail. `None` only
/// when the path cannot be encoded, which is logged.
#[must_use]
fn folder_picked_detail(path: &Path) -> Option<String> {
    match serde_json::to_string(&normalize_dropped_path(path)) {
        Ok(detail) => Some(detail),
        Err(error) => {
            eprintln!("could not encode the picked folder path: {error}");
            None
        }
    }
}

/// The `promptforge:folder-picked` dispatch script for a pick outcome, or
/// `None` for a cancelled pick - a cancel dispatches no event, matching
/// the file-drop bridge, which dispatches nothing for an empty drop.
#[must_use]
fn folder_picked_script(picked: Option<&Path>) -> Option<String> {
    let detail = folder_picked_detail(picked?)?;
    Some(format!(
        "window.dispatchEvent(new CustomEvent(\"promptforge:folder-picked\", {{detail: {{path: {detail}}}}}));"
    ))
}

/// Dispatches the `promptforge:folder-picked` event carrying the chosen
/// path. The page grants the path through the workspace HTTP API, exactly
/// as it grants a dropped path.
fn dispatch_folder_picked(webview: &WebView, picked: Option<&Path>) {
    let Some(script) = folder_picked_script(picked) else {
        return;
    };
    if let Err(error) = webview.evaluate_script(&script) {
        eprintln!("could not dispatch the folder-picked event: {error}");
    }
}

/// Executes one [`ShellEvent`] on the event loop thread, where the tao
/// `Window` and the `WebView` live. The match is exhaustive on purpose:
/// a new variant fails to compile here instead of being silently
/// swallowed by the loop's catch-all for foreign tao events.
fn handle_shell_event(
    event: ShellEvent,
    window: &Window,
    webview: &WebView,
    control_flow: &mut ControlFlow,
) {
    match event {
        ShellEvent::Command(WindowCommand::Close) => {
            *control_flow = ControlFlow::Exit;
        }
        ShellEvent::Command(WindowCommand::Drag) => {
            if let Err(error) = window.drag_window() {
                eprintln!("could not start the native window drag: {error}");
            }
        }
        ShellEvent::Command(WindowCommand::Minimize) => {
            window.set_minimized(true);
        }
        ShellEvent::Command(WindowCommand::ToggleMaximize) => {
            window.set_maximized(!window.is_maximized());
        }
        ShellEvent::FileDrop(paths) => {
            dispatch_file_drop(webview, &paths);
        }
        ShellEvent::PickFolder => {
            dispatch_folder_picked(webview, pick_folder(window).as_deref());
        }
        ShellEvent::OpenExternal(url) => {
            if let Err(error) = open::that(&url) {
                eprintln!("could not open {url} in the system browser: {error}");
            }
        }
    }
}

/// Opens the workshop window on `url` and runs the event loop until the
/// user closes the window, then returns.
///
/// This is the crate's single entry point: the caller owns everything
/// before the window opens (configuration, server startup, the health
/// wait) and everything after it closes (shutdown).
///
/// # Errors
/// Returns an error if `url` is not a valid URL, or if the window or the
/// webview cannot be created.
pub fn run(url: &str) -> anyhow::Result<()> {
    // The one origin the webview may navigate to in place; every other
    // http(s) target opens in the system browser (classify_navigation).
    let server_origin = url::Url::parse(url)
        .context("parse the workshop URL")?
        .origin();
    let event_loop = EventLoopBuilder::<ShellEvent>::with_user_event().build();
    let builder = WindowBuilder::new()
        .with_title("PromptForge")
        .with_window_icon(window_icon());
    // The custom HTML title bar replaces the native frame on Windows;
    // macOS and Linux keep their decorated windows.
    #[cfg(target_os = "windows")]
    let builder = builder.with_decorations(false);
    let window = builder
        .build(&event_loop)
        .context("create the workshop window")?;

    // One channel into the loop; each handler gets its own clonable
    // sender. The clones are made up front because the IPC handler moves
    // the original.
    let proxy = event_loop.create_proxy();
    let navigation_proxy = proxy.clone();
    let drop_proxy = proxy.clone();
    let webview_builder = WebViewBuilder::new()
        .with_url(url)
        .with_initialization_script("window.__PROMPTFORGE_DESKTOP__ = true;")
        .with_ipc_handler(move |request: wry::http::Request<String>| {
            let Some(event) = parse_web_message(request.body()) else {
                return;
            };
            if let Err(error) = proxy.send_event(event) {
                eprintln!("could not forward the web message to the event loop: {error}");
            }
        })
        // Both decision callbacks below stay inline: wry demands each
        // answer synchronously as the callback's return value, so the
        // decision cannot defer through the proxy - the proxy can fire
        // events at the loop, never answer from it.
        .with_permission_handler(|kind| match kind {
            PermissionKind::Microphone => PermissionResponse::Allow,
            _ => PermissionResponse::Default,
        })
        .with_navigation_handler(move |target| {
            let classification = classify_navigation(&server_origin, &target);
            if let Some(effect) = navigation_effect(classification, target)
                && let Err(error) = navigation_proxy.send_event(effect)
            {
                eprintln!("could not forward the external-open request to the event loop: {error}");
            }
            classification == Navigation::Allow
        });
    // On Windows the shell must NOT use wry's drag-drop handler: wry
    // implements it by registering its own OLE drop target on the WebView2
    // child windows, which starves Chromium of drag events and disables
    // HTML5 drag-and-drop inside the page (Dockview panel drags included);
    // see https://github.com/tauri-apps/tauri/issues/15138.
    // Explorer path drops arrive over the web-message bridge instead (see
    // file_drop.rs), attached right after the webview is built.
    #[cfg(not(target_os = "windows"))]
    let webview_builder = {
        webview_builder.with_drag_drop_handler(move |event| {
            // Only the drop is consumed. Returning true tells wry to skip
            // the platform's default handling, and on macOS wry suppresses
            // the WKWebView superclass drag methods whenever the handler
            // returns true - consuming Enter/Over/Leave would starve the
            // page of dragover/drop and kill HTML5 drag-and-drop (Dockview
            // panel drags included), so those return false and the default
            // WebKit behavior runs.
            match event {
                DragDropEvent::Drop { paths, .. } => {
                    if let Err(error) = drop_proxy.send_event(ShellEvent::FileDrop(paths)) {
                        eprintln!("could not forward the dropped paths to the event loop: {error}");
                    }
                    // Take over the drop so the webview never navigates to
                    // a dropped file; the page learns the paths through the
                    // promptforge:file-drop event instead.
                    true
                }
                _ => false,
            }
        })
    };
    let webview = webview_builder
        .build(&window)
        .context("create the workshop webview")?;

    // The page posts a drop's File objects over the WebView2 web-message
    // channel; the bridge reads their real OS paths and feeds the same
    // FileDrop event the wry handler produces elsewhere. An attach failure
    // only degrades Explorer path drops, never the app.
    #[cfg(target_os = "windows")]
    {
        if let Err(error) = crate::file_drop::attach(&webview, move |paths| {
            if let Err(error) = drop_proxy.send_event(ShellEvent::FileDrop(paths)) {
                eprintln!("could not forward the dropped paths to the event loop: {error}");
            }
        }) {
            eprintln!(
                "could not attach the file-drop bridge; Explorer drops will not grant workspace roots: {error}"
            );
        }
    }

    let mut event_loop = event_loop;
    // Resize events stream in during a drag-resize while the maximized
    // flag almost never changes, so the title bar hears only about
    // transitions. The first resize dispatches too, giving the page the
    // initial state.
    let mut last_maximized: Option<bool> = None;
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(..),
                ..
            } => {
                let maximized = window.is_maximized();
                if last_maximized != Some(maximized) {
                    last_maximized = Some(maximized);
                    dispatch_maximized(&webview, maximized);
                }
            }
            Event::UserEvent(shell_event) => {
                handle_shell_event(shell_event, &window, &webview, control_flow);
            }
            _ => {}
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        ICON_PNG, Navigation, PICK_FOLDER_MESSAGE, ShellEvent, WindowCommand, classify_navigation,
        decode_png_rgba, folder_picked_detail, folder_picked_script, navigation_effect,
        normalize_dropped_path, parse_web_message, parse_window_command,
    };

    #[test]
    fn the_bundled_icon_decodes_to_128px_rgba() {
        let (pixels, width, height) = match decode_png_rgba(ICON_PNG) {
            Ok(decoded) => decoded,
            Err(error) => panic!("the bundled program icon must decode: {error}"),
        };
        assert_eq!((width, height), (128, 128));
        assert_eq!(pixels.len(), 128 * 128 * 4);
    }

    #[test]
    fn non_png_bytes_fail_to_decode() {
        assert!(decode_png_rgba(b"not a png").is_err());
        assert!(decode_png_rgba(b"").is_err());
    }

    #[test]
    fn each_valid_command_parses_to_its_variant() {
        let cases = [
            (r#"{"command":"drag"}"#, WindowCommand::Drag),
            (r#"{"command":"minimize"}"#, WindowCommand::Minimize),
            (
                r#"{"command":"toggle-maximize"}"#,
                WindowCommand::ToggleMaximize,
            ),
            (r#"{"command":"close"}"#, WindowCommand::Close),
        ];
        for (payload, expected) in cases {
            assert_eq!(parse_window_command(payload), Some(expected), "{payload}");
        }
    }

    #[test]
    fn malformed_json_and_unknown_commands_are_ignored() {
        for payload in [
            "",
            "not json",
            "null",
            r#"["drag"]"#,
            r#"{"other":"drag"}"#,
            r#"{"command":""}"#,
            r#"{"command":"quit"}"#,
            r#"{"command":"Drag"}"#,
            r#"{"command":42}"#,
            r#"{"command":["drag"]}"#,
        ] {
            assert_eq!(parse_window_command(payload), None, "{payload}");
        }
    }

    #[test]
    fn the_pick_folder_message_routes_to_the_picker() {
        assert_eq!(
            parse_web_message("workspace-pick-folder"),
            Some(ShellEvent::PickFolder)
        );
        assert_eq!(PICK_FOLDER_MESSAGE, "workspace-pick-folder");
    }

    #[test]
    fn title_bar_envelopes_still_route_through_the_shared_parser() {
        assert_eq!(
            parse_web_message(r#"{"command":"minimize"}"#),
            Some(ShellEvent::Command(WindowCommand::Minimize))
        );
    }

    #[test]
    fn foreign_channel_messages_parse_to_no_event() {
        for payload in [
            "",
            "workspace-drop",
            "workspace-pick-folder ",
            "Workspace-Pick-Folder",
            r#"{"command":"workspace-pick-folder"}"#,
            r#""workspace-pick-folder""#,
        ] {
            assert_eq!(parse_web_message(payload), None, "{payload}");
        }
    }

    #[test]
    fn a_cancelled_pick_dispatches_no_event() {
        assert_eq!(folder_picked_script(None), None);
    }

    #[test]
    fn a_chosen_path_round_trips_through_the_event_payload() {
        for path in [
            r"C:\Users\Vinnie\Documents\project",
            r"C:\Users\Vinnie\My Documents\a folder",
            "D:\\src\\caf\u{e9} \u{4e2d}\u{6587}",
        ] {
            let Some(detail) = folder_picked_detail(Path::new(path)) else {
                panic!("a picked path must encode: {path}");
            };
            let round_tripped: String = match serde_json::from_str(&detail) {
                Ok(value) => value,
                Err(error) => panic!("the payload must be valid JSON: {error}"),
            };
            assert_eq!(round_tripped, path, "{path}");
        }
    }

    #[test]
    fn the_picked_path_script_targets_the_folder_picked_event() {
        let Some(script) = folder_picked_script(Some(Path::new(r"\\?\C:\Users\Vinnie\proj")))
        else {
            panic!("a chosen path must produce a dispatch script");
        };
        assert!(script.contains("promptforge:folder-picked"), "{script}");
        // The verbatim prefix is stripped and the backslashes arrive
        // JSON-escaped, so the page reads the Explorer spelling back.
        assert!(script.contains(r#""C:\\Users\\Vinnie\\proj""#), "{script}");
    }

    /// The origin of the in-process server the test window was opened on.
    fn server_origin(url: &str) -> url::Origin {
        match url::Url::parse(url) {
            Ok(parsed) => parsed.origin(),
            Err(error) => panic!("the test server URL must parse: {error}"),
        }
    }

    #[test]
    fn same_origin_urls_load_in_the_webview() {
        let server = server_origin("http://127.0.0.1:7910/");
        for url in [
            "http://127.0.0.1:7910/",
            "http://127.0.0.1:7910/ws",
            "http://127.0.0.1:7910/settings?tab=keys#top",
        ] {
            assert_eq!(
                classify_navigation(&server, url),
                Navigation::Allow,
                "{url}"
            );
        }
    }

    #[test]
    fn a_default_port_normalizes_into_the_origin() {
        let server = server_origin("https://127.0.0.1/");
        assert_eq!(
            classify_navigation(&server, "https://127.0.0.1:443/"),
            Navigation::Allow
        );
    }

    #[test]
    fn other_loopback_origins_open_in_the_system_browser() {
        let server = server_origin("http://127.0.0.1:7910/");
        for url in [
            // Another loopback server is not the workshop, whatever the
            // port, scheme, or loopback spelling.
            "http://127.0.0.1:4000/",
            "https://127.0.0.1:7910/",
            "http://localhost:7910/",
            "http://[::1]:7910/",
        ] {
            assert_eq!(
                classify_navigation(&server, url),
                Navigation::OpenExternally,
                "{url}"
            );
        }
    }

    #[test]
    fn external_urls_open_in_the_system_browser() {
        let server = server_origin("http://127.0.0.1:7910/");
        for url in [
            "https://example.com/",
            "http://192.168.1.10/",
            "https://localhost.evil.example/",
        ] {
            assert_eq!(
                classify_navigation(&server, url),
                Navigation::OpenExternally,
                "{url}"
            );
        }
    }

    #[test]
    fn allowed_navigations_defer_no_side_effect() {
        assert_eq!(
            navigation_effect(Navigation::Allow, "http://127.0.0.1:7910/".to_owned()),
            None
        );
    }

    #[test]
    fn denied_navigations_defer_the_browser_open_to_the_event_loop() {
        let target = "https://example.com/docs";
        assert_eq!(
            navigation_effect(Navigation::OpenExternally, target.to_owned()),
            Some(ShellEvent::OpenExternal(target.to_owned()))
        );
    }

    #[test]
    fn non_http_and_unparseable_targets_are_left_to_the_webview() {
        let server = server_origin("http://127.0.0.1:7910/");
        for url in [
            "about:blank",
            "data:text/html,<p>hi</p>",
            "not a url",
            "/relative/path",
        ] {
            assert_eq!(
                classify_navigation(&server, url),
                Navigation::Allow,
                "{url}"
            );
        }
    }

    #[test]
    fn dropped_paths_keep_backslashes_spaces_and_unicode() {
        for path in [
            r"C:\Users\Vinnie\Documents\project",
            r"C:\Users\Vinnie\My Documents\file name.txt",
            "C:\\Users\\Vinnie\\caf\u{e9} \u{4e2d}\u{6587}.txt",
            r"D:\src\promptforge\crates",
        ] {
            assert_eq!(normalize_dropped_path(Path::new(path)), path, "{path}");
        }
    }

    #[test]
    fn dropped_paths_shed_the_verbatim_prefix() {
        assert_eq!(
            normalize_dropped_path(Path::new(r"\\?\C:\Users\Vinnie\file.txt")),
            r"C:\Users\Vinnie\file.txt"
        );
        assert_eq!(
            normalize_dropped_path(Path::new("\\\\?\\D:\\src\\caf\u{e9}.txt")),
            "D:\\src\\caf\u{e9}.txt"
        );
        assert_eq!(
            normalize_dropped_path(Path::new(r"\\?\UNC\server\share\file.txt")),
            r"\\server\share\file.txt"
        );
    }
}
