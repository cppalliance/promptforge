//! The window menu: the quit-everything affordance.
//!
//! The shell's only menu item quits the app; when boot attached to or
//! launched a local sidecar gateway, the item first posts the gateway's
//! `/shutdown` with the connection file's key, so one gesture stops the
//! window, the in-process server, and the gateway. Attached to a LAN
//! gateway through explicit config, the item stops the shell only and
//! says so: a client never stops a shared gateway.

use std::sync::PoisonError;

use shared_sidecar::ConnectionFile;
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Manager as _, Wry};

use crate::GatewaySlot;

/// The quit item's menu id, matched by the event handler.
pub(crate) const QUIT_MENU_ID: &str = "quit-promptforge";

/// Builds and installs the app menu. `sidecar` is the attached or
/// launched gateway's connection file: present, the quit item also stops
/// the gateway and its label says so; absent (a LAN gateway from
/// explicit config), the item stops the shell only.
///
/// # Errors
/// Returns an error when the menu cannot be built or installed.
pub(crate) fn install(app: &tauri::App, sidecar: Option<&ConnectionFile>) -> tauri::Result<()> {
    let label = if sidecar.is_some() {
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

/// Handles the quit item: post the sidecar gateway's `/shutdown` when one
/// is attached, then exit the shell (the `RunEvent::Exit` handler stops
/// the in-process server). A refused or undeliverable shutdown request is
/// reported and the shell exits anyway - quit always works, even when
/// the gateway is wedged.
pub(crate) fn handle_event(app: &AppHandle<Wry>, event: tauri::menu::MenuEvent) {
    let tauri::menu::MenuEvent { id } = event;
    if id != QUIT_MENU_ID {
        return;
    }
    let file = app
        .try_state::<GatewaySlot>()
        .map(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).clone());
    if let Some(Some(file)) = file
        && let Err(error) = shared_sidecar::request_shutdown(&file)
    {
        eprintln!(
            "the gateway did not accept the shutdown request; quitting the shell anyway: {error}"
        );
    }
    app.exit(0);
}
