//! The window menu: the quit-everything affordance.
//!
//! The shell's only menu item quits the app; when boot attached to or
//! launched a local sidecar gateway, the item first posts the gateway's
//! `/shutdown` through the server's current validated Gateway snapshot, so
//! one gesture stops the window, the in-process server, and the Gateway.
//! Attached to a LAN Gateway through explicit config, the snapshot grants
//! no shutdown authority, so the item stops the shell only and says so.

use std::sync::PoisonError;

use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Manager as _, Wry};

use crate::ServerSlot;

/// The quit item's menu id, matched by the event handler.
pub(crate) const QUIT_MENU_ID: &str = "quit-promptforge";

/// Builds and installs the app menu. A local sidecar makes the quit item
/// stop both products; a configured LAN Gateway makes it stop only the shell.
///
/// # Errors
/// Returns an error when the menu cannot be built or installed.
pub(crate) fn install(app: &tauri::App, has_sidecar: bool) -> tauri::Result<()> {
    let label = if has_sidecar {
        "Quit PromptForge and Gateway"
    } else {
        "Quit PromptForge"
    };
    let quit = MenuItemBuilder::with_id(QUIT_MENU_ID, label)
        .accelerator("CmdOrCtrl+Q")
        .build(app)?;
    // Setting a menu replaces the stock one wholesale, so macOS
    // re-declares the default layout (app, File, Edit, View, Window) with
    // the stock Quit swapped for the quit-everything item. Windows and
    // Linux get the single File submenu; the Windows window is
    // undecorated, so the menu bar is hidden there and the accelerator
    // is the gesture.
    #[cfg(target_os = "macos")]
    let menu = {
        let app_menu = SubmenuBuilder::new(app, "PromptForge")
            .about(None)
            .separator()
            .services()
            .separator()
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .item(&quit)
            .build()?;
        let file = SubmenuBuilder::new(app, "File").close_window().build()?;
        let edit = SubmenuBuilder::new(app, "Edit")
            .undo()
            .redo()
            .separator()
            .cut()
            .copy()
            .paste()
            .select_all()
            .build()?;
        let view = SubmenuBuilder::new(app, "View").fullscreen().build()?;
        let window = SubmenuBuilder::new(app, "Window")
            .minimize()
            .maximize()
            .separator()
            .close_window()
            .build()?;
        MenuBuilder::new(app)
            .items(&[&app_menu, &file, &edit, &view, &window])
            .build()?
    };
    #[cfg(not(target_os = "macos"))]
    let menu = {
        let file = SubmenuBuilder::new(app, "File").item(&quit).build()?;
        MenuBuilder::new(app).item(&file).build()?
    };
    let _previous = app.set_menu(menu)?;
    Ok(())
}

/// Handles the quit item: ask the server's current validated local Gateway
/// snapshot to post `/shutdown`, then exit the shell (the `RunEvent::Exit`
/// handler stops the in-process server). A configured LAN Gateway grants no
/// shutdown authority. A refused or undeliverable request is reported and
/// the shell exits anyway - quit always works, even when the Gateway is
/// wedged.
pub(crate) fn handle_event(app: &AppHandle<Wry>, event: tauri::menu::MenuEvent) {
    let tauri::menu::MenuEvent { id } = event;
    if id != QUIT_MENU_ID {
        return;
    }
    let gateway = app.try_state::<ServerSlot>().and_then(|slot| {
        slot.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(workshop_server::ServerHandle::gateway_updater)
    });
    if let Some(gateway) = gateway
        && let Err(error) = gateway.request_shutdown()
    {
        eprintln!(
            "the gateway did not accept the shutdown request; quitting the shell anyway: {error}"
        );
    }
    app.exit(0);
}
