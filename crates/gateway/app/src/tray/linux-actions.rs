//! What the tray does: the menu items' actions and the [`ksni::Tray`]
//! surface the StatusNotifierItem service reads.

use ksni::menu::{CheckmarkItem, StandardItem};

use super::{SniTray, probe_workshop};
use crate::tray::logic::{self, MenuItemSpec};

impl SniTray {
    /// Opens the config SPA in the default browser through the one-time
    /// handoff URL, so the bearer key never sits in browser history.
    fn open_settings(&self) {
        if let Err(error) = open::that(&self.auth_url) {
            tracing::warn!("could not open the settings page: {error}");
        }
    }

    /// Launches the workshop shell, detached: its own process group, so a
    /// terminal Ctrl-C on the gateway does not SIGINT the workshop. It
    /// attaches to this gateway through the gateway discovery file and outlives
    /// it.
    fn launch_workshop(&self) {
        use std::os::unix::process::CommandExt as _;

        let Some(exe) = self.workshop_exe.as_ref() else {
            return;
        };
        let mut command = std::process::Command::new(exe);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        if let Err(error) = command.spawn() {
            tracing::warn!("could not launch {}: {error}", exe.display());
        }
    }

    /// Toggles the XDG autostart entry and reflects the result in the
    /// check item; a failed write leaves the state reading from the file.
    fn toggle_login(&mut self) {
        let Some(store) = self.login.as_mut() else {
            // The item is disabled without a store, so no click arrives.
            return;
        };
        let enable = !logic::launch_at_login(store);
        let exe = match std::env::current_exe() {
            Ok(exe) => {
                // The Exec line names the canonicalized exe: current_exe is
                // the resolved /proc/self/exe on Linux, and canonicalize
                // covers symlinked launchers.
                std::fs::canonicalize(&exe).unwrap_or(exe)
            }
            Err(error) => {
                tracing::warn!("could not locate the gateway executable: {error}");
                return;
            }
        };
        match logic::set_launch_at_login(store, &logic::linux::exec_command(&exe), enable) {
            Ok(enabled) => self.login_checked = enabled,
            Err(error) => {
                tracing::warn!("could not update the autostart entry: {error}");
                self.login_checked = logic::launch_at_login(store);
            }
        }
    }

    /// Signals the main loop to quit.
    fn request_quit(&self) {
        // A failed send means the main loop already ended, which is the
        // quit state being requested.
        let _ = self.quit.send(());
    }
}

impl ksni::Tray for SniTray {
    // The menu is the only path: SNI delivers icon click events at the
    // visualization's discretion, so activation opens the menu instead of
    // an action.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "promptforge-gateway".to_owned()
    }

    fn title(&self) -> String {
        self.label.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icons.for_phase(self.phase).clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.label.clone(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        logic::menu_spec(
            &self.label,
            self.workshop_exe.is_some(),
            self.login.is_some(),
            self.login_checked,
        )
        .into_iter()
        .map(|item| match item {
            MenuItemSpec::Status(text) => StandardItem {
                label: text,
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItemSpec::Workshop { enabled } => StandardItem {
                label: "Workshop".to_owned(),
                enabled,
                activate: Box::new(|tray: &mut Self| tray.launch_workshop()),
                ..Default::default()
            }
            .into(),
            MenuItemSpec::Settings => StandardItem {
                label: "Settings".to_owned(),
                activate: Box::new(|tray: &mut Self| tray.open_settings()),
                ..Default::default()
            }
            .into(),
            MenuItemSpec::Separator => ksni::MenuItem::Separator,
            MenuItemSpec::LaunchAtLogin { enabled, checked } => CheckmarkItem {
                label: "Launch at Login".to_owned(),
                enabled,
                checked,
                activate: Box::new(|tray: &mut Self| tray.toggle_login()),
                ..Default::default()
            }
            .into(),
            MenuItemSpec::Quit => StandardItem {
                label: "Quit".to_owned(),
                activate: Box::new(|tray: &mut Self| tray.request_quit()),
                ..Default::default()
            }
            .into(),
        })
        .collect()
    }

    fn menu_about_to_show(&mut self) {
        // The pre-display refresh: re-probe the states the menu is about
        // to show, so it never displays a stale enabled bit or check mark.
        self.workshop_exe = probe_workshop();
        if let Some(store) = self.login.as_ref() {
            self.login_checked = logic::launch_at_login(store);
        }
    }

    fn watcher_online(&self) {
        tracing::info!("a StatusNotifierWatcher appeared; the tray is live");
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        tracing::info!(
            "the StatusNotifierWatcher is offline ({reason:?}); the tray re-registers when one appears"
        );
        // Stay alive: returning false would shut the service down and lose
        // the automatic re-registration.
        true
    }
}
