//! The macOS backend's pure rules: bundle-path derivation, the login-service
//! gate, and the template-glyph preparation. Compiled for macOS and for
//! tests everywhere, so CI exercises them without the objc2 boundary.

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
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| [0, 0, 0, px[3]])
        .collect()
}
