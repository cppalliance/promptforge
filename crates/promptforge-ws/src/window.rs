//! The desktop window: a tao event loop driving a wry webview pointed at
//! the in-process workshop server.
//!
//! There is no native menu: tao 0.36 moved menu support out to the `muda`
//! crate, and a one-item `File > Quit` menu does not justify the extra
//! dependency and its event channel. The window's close button is the quit
//! gesture. `run` uses tao's `run_return` so control comes back after the
//! loop exits and the caller can shut the server down cleanly.

use anyhow::Context as _;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::WindowBuilder;
use wry::{PermissionKind, PermissionResponse, WebView, WebViewBuilder};

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

/// Runs the window's event loop until the user closes the window, then
/// returns.
///
/// # Errors
/// Returns an error if the window or the webview cannot be created.
pub(crate) fn run(url: &str) -> anyhow::Result<()> {
    let event_loop = EventLoopBuilder::<WindowCommand>::with_user_event().build();
    let builder = WindowBuilder::new().with_title("PromptForge");
    // The custom HTML title bar replaces the native frame on Windows;
    // macOS and Linux keep their decorated windows.
    #[cfg(target_os = "windows")]
    let builder = builder.with_decorations(false);
    let window = builder
        .build(&event_loop)
        .context("create the workshop window")?;

    let proxy = event_loop.create_proxy();
    let webview = WebViewBuilder::new()
        .with_url(url)
        .with_initialization_script("window.__PROMPTFORGE_DESKTOP__ = true;")
        .with_ipc_handler(move |request: wry::http::Request<String>| {
            let Some(command) = parse_window_command(request.body()) else {
                return;
            };
            if let Err(error) = proxy.send_event(command) {
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
        })
        .build(&window)
        .context("create the workshop webview")?;

    let mut event_loop = event_loop;
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::UserEvent(WindowCommand::Close) => *control_flow = ControlFlow::Exit,
            Event::WindowEvent {
                event: WindowEvent::Resized(..),
                ..
            } => dispatch_maximized(&webview, window.is_maximized()),
            Event::UserEvent(WindowCommand::Drag) => {
                if let Err(error) = window.drag_window() {
                    eprintln!("could not start the native window drag: {error}");
                }
            }
            Event::UserEvent(WindowCommand::Minimize) => window.set_minimized(true),
            Event::UserEvent(WindowCommand::ToggleMaximize) => {
                window.set_maximized(!window.is_maximized());
            }
            _ => {}
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Navigation, WindowCommand, classify_navigation, parse_window_command};

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
}
