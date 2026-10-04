//! Tests for the tray's pure rules: phases, labels, the menu, login, icons, and the macOS and Linux backends.

use super::*;

#[test]
fn a_serving_poll_shows_running_and_a_stopped_poll_shows_error() {
    assert_eq!(next_phase(Poll::Serving), TrayPhase::Running);
    assert_eq!(next_phase(Poll::Stopped), TrayPhase::Error);
}

#[test]
fn the_status_label_follows_the_tray_idiom() {
    assert_eq!(status_label(TrayPhase::Starting, None, 0, 0.0), "Starting");
    assert_eq!(
        status_label(TrayPhase::Running, None, 2, 4.1),
        "Running - 2 models, 4.1 GB"
    );
    assert_eq!(
        status_label(TrayPhase::Running, None, 1, 1.0),
        "Running - 1 model, 1.0 GB",
        "a single model is singular"
    );
    assert_eq!(
        status_label(TrayPhase::Running, None, 2, 0.0),
        "Running - 2 models",
        "a gateway serving only remote models declares no VRAM"
    );
    assert_eq!(
        status_label(TrayPhase::Running, None, 0, 0.0),
        "Running - 0 models",
        "an idle queue with nothing loaded says so"
    );
    assert_eq!(
        status_label(TrayPhase::Error, None, 0, 0.0),
        "Error - serving stopped"
    );
}

#[test]
fn a_busy_hub_drives_the_label_with_its_text() {
    assert_eq!(
        status_label(TrayPhase::Running, Some("Downloading qwen 34%"), 0, 0.0),
        "Running - Downloading qwen 34%"
    );
    assert_eq!(
        status_label(TrayPhase::Running, Some("load-profile: main"), 2, 4.1),
        "Running - load-profile: main",
        "the activity text outranks the model count and the percent"
    );
}

#[test]
fn the_starting_and_error_phases_ignore_the_hub() {
    assert_eq!(
        status_label(TrayPhase::Starting, Some("load-profile: main"), 0, 0.0),
        "Starting",
        "no poll has reported yet, so the activity readout waits"
    );
    assert_eq!(
        status_label(TrayPhase::Error, Some("load-profile: main"), 3, 2.0),
        "Error - serving stopped",
        "a stopped gateway reports the error, not a stale activity"
    );
}

#[test]
fn the_menu_layout_puts_status_first_and_quit_last() {
    let spec = menu_spec("Running - 2 models, 4.1 GB", true, true, false);
    assert_eq!(
        spec,
        vec![
            MenuItemSpec::Status("Running - 2 models, 4.1 GB".to_owned()),
            MenuItemSpec::Workshop { enabled: true },
            MenuItemSpec::Settings,
            MenuItemSpec::Separator,
            MenuItemSpec::LaunchAtLogin {
                enabled: true,
                checked: false
            },
            MenuItemSpec::Separator,
            MenuItemSpec::Quit,
        ]
    );
}

#[test]
fn the_menu_spec_sets_the_workshop_and_login_states() {
    let spec = menu_spec("Running", false, true, true);
    assert!(
        matches!(spec[1], MenuItemSpec::Workshop { enabled: false }),
        "a Gateway-only install disables Workshop: {spec:?}"
    );
    assert!(
        matches!(
            spec[4],
            MenuItemSpec::LaunchAtLogin {
                enabled: true,
                checked: true
            }
        ),
        "the check mark follows the OS entry: {spec:?}"
    );
}

#[test]
fn the_menu_spec_disables_login_when_the_os_store_is_unavailable() {
    let spec = menu_spec("Running", true, false, false);
    assert!(
        matches!(
            spec[4],
            MenuItemSpec::LaunchAtLogin {
                enabled: false,
                checked: false
            }
        ),
        "a bare executable inside another app's bundle cannot register itself: {spec:?}"
    );
}

#[test]
fn the_sibling_probe_finds_the_workshop_exe_beside_the_gateway() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let gateway = temp.path().join("promptforge-gateway.exe");
    let workshop = temp.path().join(WORKSHOP_EXE_NAME);
    std::fs::write(&workshop, "").expect("write fixture");

    assert_eq!(
        workshop_sibling(&gateway).as_deref(),
        Some(workshop.as_path())
    );

    std::fs::remove_file(&workshop).expect("remove fixture");
    assert_eq!(
        workshop_sibling(&gateway),
        None,
        "a Gateway-only install disables the item"
    );
}

#[test]
fn the_sibling_probe_tolerates_a_missing_install_directory() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let gateway = temp.path().join("absent").join("promptforge-gateway.exe");
    assert_eq!(workshop_sibling(&gateway), None);
}

/// An in-memory `RunKeyStore` double.
#[derive(Default)]
struct FakeStore {
    value: Option<String>,
    fail_writes: bool,
}

impl RunKeyStore for FakeStore {
    fn read(&self) -> Option<String> {
        self.value.clone()
    }

    fn write(&mut self, command: &str) -> std::io::Result<()> {
        if self.fail_writes {
            return Err(std::io::Error::other("access denied"));
        }
        self.value = Some(command.to_owned());
        Ok(())
    }

    fn delete(&mut self) -> std::io::Result<()> {
        self.value = None;
        Ok(())
    }
}

#[test]
fn the_login_command_quotes_the_exe_and_appends_the_login_flag() {
    assert_eq!(
        run_key_command(Path::new(
            "C:\\Program Files\\PromptForge\\promptforge-gateway.exe"
        )),
        "\"C:\\Program Files\\PromptForge\\promptforge-gateway.exe\" --login"
    );
}

#[test]
fn enabling_login_writes_the_command_and_disabling_deletes_it() {
    let mut store = FakeStore::default();
    let exe = Path::new("C:\\PromptForge\\promptforge-gateway.exe");

    let enabled =
        set_launch_at_login(&mut store, &run_key_command(exe), true).expect("write succeeds");
    assert!(enabled);
    assert_eq!(
        store.value.as_deref(),
        Some("\"C:\\PromptForge\\promptforge-gateway.exe\" --login")
    );
    assert!(launch_at_login(&store), "the state reads from the store");

    let enabled =
        set_launch_at_login(&mut store, &run_key_command(exe), false).expect("delete succeeds");
    assert!(!enabled);
    assert!(!launch_at_login(&store));
}

#[test]
fn a_failed_write_leaves_the_state_unchanged() {
    let mut store = FakeStore {
        value: None,
        fail_writes: true,
    };
    let exe = Path::new("C:\\PromptForge\\promptforge-gateway.exe");
    let error = set_launch_at_login(&mut store, &run_key_command(exe), true)
        .expect_err("the failure propagates");
    assert_eq!(error.to_string(), "access denied");
    assert!(
        !launch_at_login(&store),
        "a failed write does not read as enabled"
    );
}

#[test]
fn the_grayed_icon_is_luma_with_alpha_preserved() {
    let rgba = [200u8, 100, 50, 255];
    let gray = grayed(&rgba);
    assert_eq!(gray.len(), 4);
    assert_eq!(gray[0], gray[1]);
    assert_eq!(gray[1], gray[2]);
    // Rec. 601: (299*200 + 587*100 + 114*50) / 1000 = 124.
    assert_eq!(gray[0], 124);
    assert_eq!(gray[3], 255, "alpha survives");
}

#[test]
fn the_error_icon_is_red_dominant_with_alpha_preserved() {
    let rgba = [40u8, 200, 220, 128];
    let error = error_tint(&rgba);
    assert!(
        error[0] > error[1] && error[0] > error[2],
        "red dominates: {error:?}"
    );
    assert_eq!(error[3], 128, "alpha survives");
}

mod macos {
    use super::super::macos::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_bundle_walks_up_from_contents_macos_to_the_app() {
        let exe = Path::new("/Applications/PromptForge.app/Contents/MacOS/promptforge-gateway");
        assert_eq!(
            app_bundle(exe).as_deref(),
            Some(Path::new("/Applications/PromptForge.app"))
        );
    }

    #[test]
    fn the_bundle_probe_rejects_unbundled_and_partial_paths() {
        assert_eq!(
            app_bundle(Path::new("/usr/local/bin/promptforge-gateway")),
            None
        );
        assert_eq!(
            app_bundle(Path::new("/Applications/PromptForge.app/Contents/MacOS")),
            None,
            "the exe itself must sit below MacOS"
        );
        assert_eq!(
            app_bundle(Path::new("/opt/x/Contents/MacOS/promptforge-gateway")),
            None,
            "the parent two levels up must be a .app"
        );
        assert_eq!(app_bundle(Path::new("promptforge-gateway")), None);
    }

    /// Writes a minimal bundle fixture: `Contents/Info.plist` with the
    /// given principal executable.
    fn bundle_fixture(dir: &Path, principal: &str) -> PathBuf {
        let bundle = dir.join("PromptForge.app");
        let contents = bundle.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).expect("mkdir");
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                "<?xml version=\"1.0\"?>\n<plist><dict>\n\
                 <key>CFBundleExecutable</key>\n<string>{principal}</string>\n\
                 </dict></plist>\n"
            ),
        )
        .expect("write plist");
        bundle
    }

    #[test]
    fn the_principal_reads_the_bundle_executable() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let bundle = bundle_fixture(temp.path(), "promptforge-gateway");
        assert_eq!(
            bundle_principal(&bundle).as_deref(),
            Some("promptforge-gateway")
        );
    }

    #[test]
    fn the_principal_is_none_without_a_plist() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(bundle_principal(temp.path()), None);
    }

    #[test]
    fn the_gateway_is_principal_only_in_its_own_bundle() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let bundle = bundle_fixture(temp.path(), "promptforge-workshop");
        let gateway = bundle.join("Contents/MacOS/promptforge-gateway");
        assert!(
            !gateway_is_bundle_principal(&bundle, &gateway),
            "inside the workshop's bundle, mainApp registration would launch the workshop"
        );
        let bundle = bundle_fixture(temp.path(), "promptforge-gateway");
        assert!(
            gateway_is_bundle_principal(&bundle, &gateway),
            "a standalone gateway bundle registers itself"
        );
    }

    #[test]
    fn registration_reads_enabled_and_requires_approval_as_present() {
        assert!(login_registered(LoginServiceStatus::Enabled));
        assert!(
            login_registered(LoginServiceStatus::RequiresApproval),
            "a revoked-but-registered entry still exists in System Settings"
        );
        assert!(!login_registered(LoginServiceStatus::NotRegistered));
        assert!(!login_registered(LoginServiceStatus::NotFound));
    }

    #[test]
    fn the_login_service_requires_macos_13() {
        assert!(!login_service_supported(12));
        assert!(login_service_supported(13));
        assert!(login_service_supported(26));
    }

    #[test]
    fn the_template_glyph_is_black_with_alpha_preserved() {
        let rgba = [200u8, 100, 50, 255, 1, 2, 3, 64];
        let glyph = template_glyph(&rgba);
        assert_eq!(glyph, [0, 0, 0, 255, 0, 0, 0, 64]);
    }
}

mod linux {
    use super::super::linux::*;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_watcher_and_notification_names_are_the_freedesktop_names() {
        // A typo here silently strands the tray on every desktop.
        assert_eq!(WATCHER_NAME, "org.kde.StatusNotifierWatcher");
        assert_eq!(NOTIFICATIONS_NAME, "org.freedesktop.Notifications");
        assert_eq!(NOTIFICATIONS_PATH, "/org/freedesktop/Notifications");
    }

    #[test]
    fn the_autostart_path_honors_xdg_config_home() {
        let home = Path::new("/home/user");
        let xdg = OsStr::new("/xdg/config");
        assert_eq!(
            autostart_path(Some(xdg), home),
            PathBuf::from("/xdg/config/autostart/promptforge-gateway.desktop")
        );
    }

    #[test]
    fn the_autostart_path_defaults_to_home_config() {
        let home = Path::new("/home/user");
        let expected = PathBuf::from("/home/user/.config/autostart/promptforge-gateway.desktop");
        assert_eq!(autostart_path(None, home), expected);
        assert_eq!(
            autostart_path(Some(OsStr::new("")), home),
            expected,
            "an empty XDG_CONFIG_HOME is ignored, per the basedir spec"
        );
        assert_eq!(
            autostart_path(Some(OsStr::new("relative/dir")), home),
            expected,
            "a relative XDG_CONFIG_HOME is invalid and ignored"
        );
    }

    #[test]
    fn the_exec_command_quotes_spaces_and_escapes_reserved_characters() {
        assert_eq!(
            exec_command(Path::new("/opt/Prompt Forge/promptforge-gateway")),
            "\"/opt/Prompt Forge/promptforge-gateway\" --login"
        );
        assert_eq!(
            exec_command(Path::new("/opt/weird$`\\\"dir/promptforge-gateway")),
            "\"/opt/weird\\$\\`\\\\\\\"dir/promptforge-gateway\" --login",
            "the desktop-entry parser's reserved characters are backslash-escaped"
        );
    }

    #[test]
    fn the_desktop_entry_is_a_daemon_autostart_file() {
        let entry = desktop_entry("\"/opt/Prompt Forge/promptforge-gateway\" --login");
        assert_eq!(
            entry,
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=PromptForge Gateway\n\
             Comment=PromptForge inference gateway\n\
             Exec=\"/opt/Prompt Forge/promptforge-gateway\" --login\n\
             Terminal=false\n",
            "the Exec line is the quoted exe plus --login; Terminal=false"
        );
    }

    #[test]
    fn the_notification_marker_lives_beside_the_profile_config() {
        assert_eq!(
            notification_marker(Path::new("/home/user")),
            PathBuf::from("/home/user/.promptforge/tray-notification-sent")
        );
    }

    #[test]
    fn the_notification_body_names_the_settings_url() {
        let body = notification_body("http://127.0.0.1:8081/auth?key=abc123");
        assert!(
            body.contains("http://127.0.0.1:8081/auth?key=abc123"),
            "the body hands the tray-less user the Settings URL: {body}"
        );
    }

    #[test]
    fn the_argb_conversion_leads_with_alpha() {
        let rgba = [1u8, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(to_argb(&rgba), [4, 1, 2, 3, 8, 5, 6, 7]);
    }
}
