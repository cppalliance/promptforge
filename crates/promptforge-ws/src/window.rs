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
