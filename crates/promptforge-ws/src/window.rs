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
use tao::window::{Icon, WindowBuilder};
#[cfg(not(target_os = "windows"))]
use wry::DragDropEvent;
use wry::{PermissionKind, PermissionResponse, WebView, WebViewBuilder};

/// How often the delegating drop target retries its installation while
/// Chromium's own target has not appeared yet.
#[cfg(target_os = "windows")]
const DROP_DELEGATION_RETRY: std::time::Duration = std::time::Duration::from_millis(200);

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

/// Classifies a navigation target: loopback http(s) URLs (the in-process
/// server) load in the webview, while any other absolute http(s) URL opens
/// in the system browser so a clicked link never navigates the app away
/// from itself. Other schemes (`about:blank`, `data:`) and unparseable
/// values are left to the webview.
#[must_use]
pub(crate) fn classify_navigation(target: &str) -> Navigation {
    let Ok(url) = url::Url::parse(target) else {
        return Navigation::Allow;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Navigation::Allow;
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) if address.is_loopback() => Navigation::Allow,
        Some(url::Host::Ipv6(address)) if address.is_loopback() => Navigation::Allow,
        Some(url::Host::Domain(domain)) if domain.eq_ignore_ascii_case("localhost") => {
            Navigation::Allow
        }
        _ => Navigation::OpenExternally,
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

/// A user event delivered to the tao event loop. Both the IPC handler and
/// the drag-drop handler run on webview threads without access to the tao
/// `Window` or the `WebView`, so their payloads travel through an
/// `EventLoopProxy` and run on the event loop thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellEvent {
    /// A command from the custom title bar.
    Command(WindowCommand),
    /// Real OS paths dropped onto the webview from Explorer.
    FileDrop(Vec<PathBuf>),
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

/// Runs the window's event loop until the user closes the window, then
/// returns.
///
/// # Errors
/// Returns an error if the window or the webview cannot be created.
pub(crate) fn run(url: &str) -> anyhow::Result<()> {
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

    let proxy = event_loop.create_proxy();
    let webview_builder = WebViewBuilder::new()
        .with_url(url)
        .with_initialization_script("window.__PROMPTFORGE_DESKTOP__ = true;")
        .with_ipc_handler(move |request: wry::http::Request<String>| {
            let Some(command) = parse_window_command(request.body()) else {
                return;
            };
            if let Err(error) = proxy.send_event(ShellEvent::Command(command)) {
                eprintln!("could not forward the window command to the event loop: {error}");
            }
        })
        .with_permission_handler(|kind| match kind {
            PermissionKind::Microphone => PermissionResponse::Allow,
            _ => PermissionResponse::Default,
        })
        .with_navigation_handler(|target| match classify_navigation(&target) {
            Navigation::Allow => true,
            Navigation::OpenExternally => {
                if let Err(error) = open::that(&target) {
                    eprintln!("could not open {target} in the system browser: {error}");
                }
                false
            }
        });
    // On Windows the shell must NOT use wry's drag-drop handler: wry
    // implements it by revoking WebView2's own OLE drop target, which
    // disables HTML5 drag-and-drop inside the page (Dockview panel drags
    // included). The delegating drop target installed below observes
    // Explorer drops without taking Chromium's target away.
    #[cfg(not(target_os = "windows"))]
    let webview_builder = {
        let drop_proxy = event_loop.create_proxy();
        webview_builder.with_drag_drop_handler(move |event| {
            // Only the drop carries paths worth granting; enter, over, and
            // leave are cursor feedback the shell does not need.
            if let DragDropEvent::Drop { paths, .. } = event
                && let Err(error) = drop_proxy.send_event(ShellEvent::FileDrop(paths))
            {
                eprintln!("could not forward the dropped paths to the event loop: {error}");
            }
            // Take over the drop so the webview never navigates to a
            // dropped file; the page learns the paths through the
            // promptforge:file-drop event instead.
            true
        })
    };
    let webview = webview_builder
        .build(&window)
        .context("create the workshop webview")?;

    // The delegating drop target wraps the target Chromium registers on
    // its child windows. Chromium registers it asynchronously some time
    // after the webview exists, so installation retries on a short timer
    // until it lands (or gives up, costing only Explorer path drops).
    #[cfg(target_os = "windows")]
    let mut drop_installer = {
        use tao::platform::windows::WindowExtWindows as _;
        let drop_proxy = event_loop.create_proxy();
        crate::drop_target::Installer::new(
            window.hwnd(),
            std::rc::Rc::new(move |paths: Vec<PathBuf>| {
                if let Err(error) = drop_proxy.send_event(ShellEvent::FileDrop(paths)) {
                    eprintln!("could not forward the dropped paths to the event loop: {error}");
                }
            }),
        )
    };

    let mut event_loop = event_loop;
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        #[cfg(target_os = "windows")]
        if drop_installer.attempt() {
            *control_flow =
                ControlFlow::WaitUntil(std::time::Instant::now() + DROP_DELEGATION_RETRY);
        }
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::UserEvent(ShellEvent::Command(WindowCommand::Close)) => {
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(..),
                ..
            } => dispatch_maximized(&webview, window.is_maximized()),
            Event::UserEvent(ShellEvent::Command(WindowCommand::Drag)) => {
                if let Err(error) = window.drag_window() {
                    eprintln!("could not start the native window drag: {error}");
                }
            }
            Event::UserEvent(ShellEvent::Command(WindowCommand::Minimize)) => {
                window.set_minimized(true);
            }
            Event::UserEvent(ShellEvent::Command(WindowCommand::ToggleMaximize)) => {
                window.set_maximized(!window.is_maximized());
            }
            Event::UserEvent(ShellEvent::FileDrop(paths)) => {
                dispatch_file_drop(&webview, &paths);
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
        ICON_PNG, Navigation, WindowCommand, classify_navigation, decode_png_rgba,
        normalize_dropped_path, parse_window_command,
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
    fn loopback_urls_load_in_the_webview() {
        for url in [
            "http://127.0.0.1:7910/",
            "http://127.0.0.1:7910/ws",
            "https://127.0.0.1/",
            "http://localhost:7910/",
            "http://LOCALHOST/",
            "http://[::1]:7910/",
        ] {
            assert_eq!(classify_navigation(url), Navigation::Allow, "{url}");
        }
    }

    #[test]
    fn external_urls_open_in_the_system_browser() {
        for url in [
            "https://example.com/",
            "http://192.168.1.10/",
            "https://localhost.evil.example/",
        ] {
            assert_eq!(
                classify_navigation(url),
                Navigation::OpenExternally,
                "{url}"
            );
        }
    }

    #[test]
    fn non_http_and_unparseable_targets_are_left_to_the_webview() {
        for url in [
            "about:blank",
            "data:text/html,<p>hi</p>",
            "not a url",
            "/relative/path",
        ] {
            assert_eq!(classify_navigation(url), Navigation::Allow, "{url}");
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
