//! The platform-independent tray rules: the menu layout, the status
//! label, the icon phase machine, the workshop sibling probe, the
//! launch-at-login entry, and the icon tinting. Every backend consumes
//! these so the idiom cannot drift between platforms, and every rule is
//! unit-tested here without a tray - CI is headless.

use std::path::{Path, PathBuf};

/// The tray icon's visual phases: grayed while starting, steady while
/// running, distinct on error. The status label and tooltip carry the
/// matching word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayPhase {
    /// No status poll has reported yet.
    Starting,
    /// The gateway is serving.
    Running,
    /// The serve loop has stopped.
    Error,
}

/// What one status poll observed about the gateway thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Poll {
    /// The gateway thread is alive and serving.
    Serving,
    /// The gateway thread has exited. Shutdown arrives only through the
    /// tray, so a finished thread means the serve loop failed.
    Stopped,
}

/// Maps a poll to the icon phase. The machine is memoryless on purpose:
/// the phase is recomputed from every poll and can never latch a stale
/// state, which is the "starting forever" bug class of event-driven tray
/// icons.
pub(crate) fn next_phase(poll: Poll) -> TrayPhase {
    match poll {
        Poll::Serving => TrayPhase::Running,
        Poll::Stopped => TrayPhase::Error,
    }
}

/// The status label at the top of the menu, also used as the tooltip: the
/// gateway state plus the models it serves, e.g. "Running - 2 models,
/// 4.1 GB". The VRAM total is omitted when no local or STT model declares
/// any.
pub(crate) fn status_label(phase: TrayPhase, models: usize, vram_gb: f64) -> String {
    match phase {
        TrayPhase::Starting => "Starting".to_owned(),
        TrayPhase::Error => "Error - serving stopped".to_owned(),
        TrayPhase::Running => {
            let models = match models {
                1 => "1 model".to_owned(),
                n => format!("{n} models"),
            };
            if vram_gb > 0.0 {
                format!("Running - {models}, {vram_gb:.1} GB")
            } else {
                format!("Running - {models}")
            }
        }
    }
}

/// One tray menu entry, in display order. The backend materializes the
/// spec into native menu items; the layout is data so the idiom is
/// testable without a tray.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuItemSpec {
    /// The disabled status label on top.
    Status(String),
    /// Launches the workshop shell; disabled when no sibling exe exists.
    Workshop {
        /// Whether the sibling probe found the workshop exe.
        enabled: bool,
    },
    /// Opens the config SPA in the browser.
    Settings,
    /// A separator.
    Separator,
    /// The launch-at-login check item; the checked state is read from the
    /// OS, never from local config.
    LaunchAtLogin {
        /// Whether the platform's login store is available to this
        /// process. macOS disables the item when the gateway is a bare
        /// executable inside another app's bundle, where
        /// `SMAppService.mainApp` registration would target the wrong
        /// program.
        enabled: bool,
        /// Whether the OS autostart entry exists.
        checked: bool,
    },
    /// Quits the gateway. Always last.
    Quit,
}

/// The tray menu layout: status label on top, then Workshop and Settings,
/// Launch at Login between separators, Quit last.
pub(crate) fn menu_spec(
    status: &str,
    workshop_enabled: bool,
    login_enabled: bool,
    login_checked: bool,
) -> Vec<MenuItemSpec> {
    vec![
        MenuItemSpec::Status(status.to_owned()),
        MenuItemSpec::Workshop {
            enabled: workshop_enabled,
        },
        MenuItemSpec::Settings,
        MenuItemSpec::Separator,
        MenuItemSpec::LaunchAtLogin {
            enabled: login_enabled,
            checked: login_checked,
        },
        MenuItemSpec::Separator,
        MenuItemSpec::Quit,
    ]
}

/// The workshop executable's file name beside the gateway's.
#[cfg(target_os = "windows")]
const WORKSHOP_EXE_NAME: &str = "promptforge-workshop.exe";

/// The workshop executable's file name beside the gateway's.
#[cfg(not(target_os = "windows"))]
const WORKSHOP_EXE_NAME: &str = "promptforge-workshop";

/// Probes for the workshop executable beside the gateway's own
/// executable. The installer lays both in one directory, so presence
/// means the tray's Workshop item can launch it; absence (a Gateway-only
/// install) means the item stays disabled.
pub(crate) fn workshop_sibling(gateway_exe: &Path) -> Option<PathBuf> {
    let candidate = gateway_exe.parent()?.join(WORKSHOP_EXE_NAME);
    candidate.is_file().then_some(candidate)
}

/// The one-time browser handoff URL for the Settings item and the
/// double-click: `GET /auth` validates the key, sets the session cookie,
/// and redirects to the key-free `/config/`, so the key never sits in
/// browser history. The key is percent-encoded: a generated key is hex
/// and passes through unchanged, but a configured key can carry
/// query-special characters (`/auth` decodes through serde_urlencoded).
pub(crate) fn auth_url(base_url: &str, key: &str) -> String {
    let key: String = url::form_urlencoded::byte_serialize(key.as_bytes()).collect();
    format!("{base_url}/auth?key={key}")
}

/// The OS's launch-at-login store, behind a seam so the toggle logic is
/// testable without touching the registry. The Windows implementation
/// reads and writes the HKCU Run key through `crate::boot::registry`.
pub(crate) trait RunKeyStore {
    /// Reads the stored login command line, or `None` when absent.
    fn read(&self) -> Option<String>;
    /// Writes the login command line, creating the entry when absent.
    ///
    /// # Errors
    /// Returns the OS error when the entry cannot be written.
    fn write(&mut self, command: &str) -> std::io::Result<()>;
    /// Deletes the login entry. Deleting an absent entry succeeds.
    ///
    /// # Errors
    /// Returns the OS error when the entry cannot be deleted.
    fn delete(&mut self) -> std::io::Result<()>;
}

/// Whether the OS autostart entry exists. The state comes from the OS
/// alone - never local config - because the user can revoke it
/// externally.
pub(crate) fn launch_at_login(store: &dyn RunKeyStore) -> bool {
    store.read().is_some()
}

/// The login command line for the gateway executable: the quoted path
/// (install paths contain spaces) plus `--login`, so a login-triggered
/// start never opens a browser.
pub(crate) fn run_key_command(exe: &Path) -> String {
    format!("\"{}\" --login", exe.display())
}

/// Sets or clears the OS autostart entry, returning the state now in
/// effect.
///
/// # Errors
/// Returns the OS error when the entry cannot be written or deleted; the
/// reported state is then unchanged.
pub(crate) fn set_launch_at_login(
    store: &mut dyn RunKeyStore,
    exe: &Path,
    enable: bool,
) -> std::io::Result<bool> {
    if enable {
        store.write(&run_key_command(exe))?;
        Ok(true)
    } else {
        store.delete()?;
        Ok(false)
    }
}

/// The macOS backend's pure rules: bundle-path derivation, the login-service
/// gate, and the template-glyph preparation. Compiled for macOS and for
/// tests everywhere, so CI exercises them without the objc2 boundary.
#[cfg(any(target_os = "macos", test))]
pub(crate) mod macos {
    use std::path::{Path, PathBuf};

    /// The `.app` bundle containing an executable at
    /// `<name>.app/Contents/MacOS/<exe>`, or `None` for an unbundled
    /// executable (a dev or CI run). Pure path shape matching: the bundle
    /// is never touched on disk.
    pub(crate) fn app_bundle(exe: &Path) -> Option<PathBuf> {
        let macos_dir = exe.parent()?;
        if macos_dir.file_name()? != "MacOS" {
            return None;
        }
        let contents = macos_dir.parent()?;
        if contents.file_name()? != "Contents" {
            return None;
        }
        let bundle = contents.parent()?;
        if bundle.extension()? != "app" {
            return None;
        }
        Some(bundle.to_path_buf())
    }

    /// The bundle's principal executable name, read from
    /// `Contents/Info.plist`. The plist is NeXTSTEP/XML; the only fact the
    /// login gate needs is the `CFBundleExecutable` string, extracted
    /// directly rather than pulling a plist parser for one key.
    pub(crate) fn bundle_principal(bundle: &Path) -> Option<String> {
        let plist = std::fs::read_to_string(bundle.join("Contents/Info.plist")).ok()?;
        let key = plist.find("<key>CFBundleExecutable</key>")?;
        let after = &plist[key + "<key>CFBundleExecutable</key>".len()..];
        let open = after.find("<string>")?;
        let rest = &after[open + "<string>".len()..];
        let close = rest.find("</string>")?;
        Some(rest[..close].to_owned())
    }

    /// Whether `exe` is the bundle's principal executable. SMAppService's
    /// `mainApp` registration launches the principal executable at login,
    /// so the store only exists when that executable is the gateway
    /// itself; inside the workshop's bundle the principal is
    /// `promptforge-workshop` and registration would open the workshop
    /// window at every login.
    pub(crate) fn gateway_is_bundle_principal(bundle: &Path, exe: &Path) -> bool {
        let Some(principal) = bundle_principal(bundle) else {
            return false;
        };
        exe.file_name().is_some_and(|name| *name == *principal)
    }

    /// The `SMAppServiceStatus` values the tray distinguishes, mirrored so
    /// the mapping is testable off macOS.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum LoginServiceStatus {
        /// Never registered, or unregistered after registration.
        NotRegistered,
        /// Registered and eligible to run at login.
        Enabled,
        /// Registered, but the user must approve it in System Settings.
        RequiresApproval,
        /// An error occurred; no such service exists.
        NotFound,
    }

    /// Whether the check item shows the login entry as present.
    /// `RequiresApproval` reads as present: the registration exists and
    /// surfaces in System Settings, with the OS - not the tray - gating
    /// the actual launch.
    pub(crate) fn login_registered(status: LoginServiceStatus) -> bool {
        matches!(
            status,
            LoginServiceStatus::Enabled | LoginServiceStatus::RequiresApproval
        )
    }

    /// Whether `SMAppService` exists on this OS: the class arrived in
    /// macOS 13, and messaging an absent class panics inside the objc2
    /// class lookup, so the store is gated on the major version.
    pub(crate) fn login_service_supported(os_major: u64) -> bool {
        os_major >= 13
    }

    /// The canonical template form of an RGBA glyph: black pixels with the
    /// alpha untouched. AppKit ignores the color channels of a template
    /// image and renders the alpha shape in the system-appropriate tint;
    /// normalizing keeps the asset honest if the template flag is ever
    /// dropped, and documents that the grayed/error tints cannot apply to
    /// a macOS status item (the phase travels in the label and tooltip).
    pub(crate) fn template_glyph(rgba: &[u8]) -> Vec<u8> {
        debug_assert!(
            rgba.len().is_multiple_of(4),
            "an RGBA buffer is whole pixels"
        );
        rgba.chunks_exact(4)
            .flat_map(|px| [0, 0, 0, px[3]])
            .collect()
    }
}

/// The grayed variant of an RGBA icon: each pixel's luma with the alpha
/// untouched, for the Starting phase.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the fixed-point luma sums to at most 255"
)]
pub(crate) fn grayed(rgba: &[u8]) -> Vec<u8> {
    tint(rgba, |r, g, b| {
        // Rec. 601 luma in fixed point.
        let luma = ((299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000) as u8;
        (luma, luma, luma)
    })
}

/// The error variant of an RGBA icon: red-dominant with the alpha
/// untouched, for the Error phase. `r / 2 + 128` cannot overflow.
pub(crate) fn error_tint(rgba: &[u8]) -> Vec<u8> {
    tint(rgba, |r, g, b| (r / 2 + 128, g / 3, b / 3))
}

/// Maps every pixel's RGB channels through `f`, preserving alpha.
fn tint(rgba: &[u8], f: impl Fn(u8, u8, u8) -> (u8, u8, u8)) -> Vec<u8> {
    debug_assert!(
        rgba.len().is_multiple_of(4),
        "an RGBA buffer is whole pixels"
    );
    rgba.chunks_exact(4)
        .flat_map(|px| {
            let (r, g, b) = f(px[0], px[1], px[2]);
            [r, g, b, px[3]]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_serving_poll_shows_running_and_a_stopped_poll_shows_error() {
        assert_eq!(next_phase(Poll::Serving), TrayPhase::Running);
        assert_eq!(next_phase(Poll::Stopped), TrayPhase::Error);
    }

    #[test]
    fn the_status_label_follows_the_tray_idiom() {
        assert_eq!(status_label(TrayPhase::Starting, 0, 0.0), "Starting");
        assert_eq!(
            status_label(TrayPhase::Running, 2, 4.1),
            "Running - 2 models, 4.1 GB"
        );
        assert_eq!(
            status_label(TrayPhase::Running, 1, 1.0),
            "Running - 1 model, 1.0 GB",
            "a single model is singular"
        );
        assert_eq!(
            status_label(TrayPhase::Running, 2, 0.0),
            "Running - 2 models",
            "a gateway serving only remote models declares no VRAM"
        );
        assert_eq!(
            status_label(TrayPhase::Error, 0, 0.0),
            "Error - serving stopped"
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
    fn the_menu_spec_carries_the_workshop_and_login_states() {
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

    #[test]
    fn the_auth_url_targets_the_one_time_handoff() {
        assert_eq!(
            auth_url("http://127.0.0.1:8081", "abc123"),
            "http://127.0.0.1:8081/auth?key=abc123"
        );
    }

    #[test]
    fn the_auth_url_percent_encodes_a_configured_key() {
        // The WHATWG urlencoded byte serializer encodes space as `+`;
        // serde_urlencoded decodes it back.
        assert_eq!(
            auth_url("http://127.0.0.1:8081", "a&b=c d"),
            "http://127.0.0.1:8081/auth?key=a%26b%3Dc+d",
            "a configured key with query-special characters survives the handoff"
        );
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
    fn the_login_command_quotes_the_exe_and_carries_the_login_flag() {
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

        let enabled = set_launch_at_login(&mut store, exe, true).expect("write succeeds");
        assert!(enabled);
        assert_eq!(
            store.value.as_deref(),
            Some("\"C:\\PromptForge\\promptforge-gateway.exe\" --login")
        );
        assert!(launch_at_login(&store), "the state reads from the store");

        let enabled = set_launch_at_login(&mut store, exe, false).expect("delete succeeds");
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
        let error = set_launch_at_login(&mut store, exe, true).expect_err("the failure propagates");
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
}
