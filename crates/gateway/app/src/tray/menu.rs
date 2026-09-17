//! The native menu materialization shared by the tray backends: builds the
//! platform-independent [`MenuItemSpec`] into muda menu items and retains
//! the handles the backend mutates in place. A displayed menu is never
//! rebuilt: rebuilding is both a stale-menu UX bug
//! (<https://github.com/tauri-apps/muda/issues/129>) and, on macOS, a
//! use-after-free class
//! (<https://github.com/tauri-apps/muda/issues/328>, fixed by
//! <https://github.com/tauri-apps/muda/pull/361> but unreleased as of muda
//! 0.19.3), so every state change goes through the retained item handles
//! and `set_menu` is never called after construction.

use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};

use crate::tray::logic::MenuItemSpec;

/// Why menu materialization failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MenuBuildError {
    /// A native menu operation failed.
    #[error("build the tray menu")]
    Menu(#[source] tray_icon::menu::Error),
    /// The menu spec did not yield every retained item (a `menu_spec`
    /// bug).
    #[error("the menu spec is incomplete")]
    Spec,
}

/// The materialized menu plus the retained item handles the backend
/// mutates in place.
pub(crate) struct BuiltMenu {
    /// The native menu, handed to the tray icon at construction.
    pub(crate) menu: Menu,
    /// The disabled status line at the top of the menu.
    pub(crate) status: MenuItem,
    /// Launches the workshop shell.
    pub(crate) workshop: MenuItem,
    /// Opens the config SPA.
    pub(crate) settings: MenuItem,
    /// The launch-at-login check item.
    pub(crate) login: CheckMenuItem,
    /// Quits the gateway.
    pub(crate) quit: MenuItem,
}

impl BuiltMenu {
    /// Builds the native menu from the platform-independent spec.
    pub(crate) fn from_spec(spec: &[MenuItemSpec]) -> Result<BuiltMenu, MenuBuildError> {
        let menu = Menu::new();
        let mut status = None;
        let mut workshop = None;
        let mut settings = None;
        let mut login = None;
        let mut quit = None;
        for item in spec {
            match item {
                MenuItemSpec::Status(text) => {
                    let item = MenuItem::new(text, false, None);
                    menu.append(&item).map_err(MenuBuildError::Menu)?;
                    status = Some(item);
                }
                MenuItemSpec::Workshop { enabled } => {
                    let item = MenuItem::new("Workshop", *enabled, None);
                    menu.append(&item).map_err(MenuBuildError::Menu)?;
                    workshop = Some(item);
                }
                MenuItemSpec::Settings => {
                    let item = MenuItem::new("Settings", true, None);
                    menu.append(&item).map_err(MenuBuildError::Menu)?;
                    settings = Some(item);
                }
                MenuItemSpec::Separator => {
                    menu.append(&PredefinedMenuItem::separator())
                        .map_err(MenuBuildError::Menu)?;
                }
                MenuItemSpec::LaunchAtLogin { enabled, checked } => {
                    let item = CheckMenuItem::new("Launch at Login", *enabled, *checked, None);
                    menu.append(&item).map_err(MenuBuildError::Menu)?;
                    login = Some(item);
                }
                MenuItemSpec::Quit => {
                    let item = MenuItem::new("Quit", true, None);
                    menu.append(&item).map_err(MenuBuildError::Menu)?;
                    quit = Some(item);
                }
            }
        }
        Ok(BuiltMenu {
            menu,
            status: status.ok_or(MenuBuildError::Spec)?,
            workshop: workshop.ok_or(MenuBuildError::Spec)?,
            settings: settings.ok_or(MenuBuildError::Spec)?,
            login: login.ok_or(MenuBuildError::Spec)?,
            quit: quit.ok_or(MenuBuildError::Spec)?,
        })
    }
}
