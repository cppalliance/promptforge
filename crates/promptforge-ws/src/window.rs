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
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::WindowBuilder;
use wry::{PermissionKind, PermissionResponse, WebViewBuilder};

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

/// Runs the window's event loop until the user closes the window, then
/// returns.
///
/// # Errors
/// Returns an error if the window or the webview cannot be created.
pub(crate) fn run(url: &str) -> anyhow::Result<()> {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("PromptForge")
        .build(&event_loop)
        .context("create the workshop window")?;
    let _webview = WebViewBuilder::new()
        .with_url(url)
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
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Navigation, classify_navigation};

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
