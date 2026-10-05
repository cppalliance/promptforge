//! The Linux backend's pure rules: the watcher bus name, the XDG autostart
//! entry's path and contents, the first-run notification, and the icon
//! pixel conversion. Compiled for Linux and for tests everywhere, so CI
//! exercises them without a session bus.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The StatusNotifierWatcher's well-known bus name. Its owner on the
/// session bus is what makes a tray visible; stock GNOME runs no
/// watcher without the AppIndicator extension.
pub(crate) const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";

/// The freedesktop notification service's well-known bus name.
pub(crate) const NOTIFICATIONS_NAME: &str = "org.freedesktop.Notifications";

/// The notification service's object path.
pub(crate) const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

/// The autostart entry's file name inside the XDG autostart directory.
const AUTOSTART_FILE_NAME: &str = "promptforge-gateway.desktop";

/// The autostart entry's path: `$XDG_CONFIG_HOME/autostart/<name>`,
/// defaulting to `~/.config/autostart/<name>`. An empty or relative
/// `XDG_CONFIG_HOME` is ignored, per the basedir spec. The spec's
/// "absolute" is Linux path semantics, a leading `/`; checked as such
/// (`Path::is_absolute` would ask the operating system, which on a
/// Windows test machine rejects a drive-less path).
pub(crate) fn autostart_path(xdg_config_home: Option<&OsStr>, home: &Path) -> PathBuf {
    let config = xdg_config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.starts_with("/"))
        .unwrap_or_else(|| home.join(".config"));
    config.join("autostart").join(AUTOSTART_FILE_NAME)
}

/// The Exec line's command: the exe path double-quoted with the
/// desktop-entry spec's reserved characters (`"`, `` ` ``, `$`, `\`)
/// backslash-escaped, plus `--login` - the bare invocation serves.
/// The shared `run_key_command`
/// quotes for the Windows Run key, whose parser has no escape layer;
/// the desktop-entry parser does, so an install path containing a
/// reserved character would misparse without the escaping.
pub(crate) fn exec_command(exe: &Path) -> String {
    let raw = exe.to_string_lossy();
    let mut quoted = String::with_capacity(raw.len() + 3);
    quoted.push('"');
    for ch in raw.chars() {
        if matches!(ch, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    format!("{quoted} --login")
}

/// The autostart entry's contents: `Terminal=false` (a daemon, not a
/// terminal program), and the Exec line is the login command - the
/// quoted exe plus `--login`, so a login-triggered start never
/// opens a browser. The app-grid launcher is packaging's file; this
/// writer serves the autostart toggle.
pub(crate) fn desktop_entry(exec_command: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=PromptForge Gateway\n\
         Comment=PromptForge inference gateway\n\
         Exec={exec_command}\n\
         Terminal=false\n"
    )
}

/// The first-run notification's sentinel: written after the no-watcher
/// notification is posted, so the once-per-install message never
/// repeats. Sits beside the profile config rather than in the run
/// directory, because it records user-facing state rather than a
/// runtime fact.
pub(crate) fn notification_marker(home: &Path) -> PathBuf {
    home.join(".promptforge").join("tray-notification-sent")
}

/// The no-watcher notification's body, naming the Settings handoff URL
/// so a tray-less desktop user still has a path to the config SPA.
pub(crate) fn notification_body(settings_url: &str) -> String {
    format!(
        "This desktop shows no system tray, so no tray icon appears. \
         The gateway is still running; its Settings page is {settings_url}"
    )
}

/// RGBA to ksni's ARGB32: the alpha byte leads each pixel.
pub(crate) fn to_argb(rgba: &[u8]) -> Vec<u8> {
    debug_assert!(
        rgba.len().is_multiple_of(4),
        "an RGBA buffer is whole pixels"
    );
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| [px[3], px[0], px[1], px[2]])
        .collect()
}
