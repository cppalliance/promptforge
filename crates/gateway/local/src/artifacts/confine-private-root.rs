//! Owner-only cache root enforcement: Unix mode bits and the Windows DACL.

use std::path::Path;

#[cfg(unix)]
use std::fs;

use super::Result;
#[cfg(windows)]
use super::{parse_whoami_user_sid, windows_sid_grant};
use crate::error::LocalError;

/// Enforces the private-cache ownership precondition on the cache `root`.
///
/// This is a real, verified restriction on every platform (ART-006), never a
/// silent no-op:
/// - Unix: `chmod 0700`, then verify no group/world mode bits remain.
/// - Windows: strip inherited ACEs and grant the current process SID full control
///   (`icacls /inheritance:r /grant:r`), then verify no broad principal
///   (Everyone / Authenticated Users / Users) still appears in the DACL.
///
/// Returns [`LocalError::CacheNotPrivate`] when the root cannot be made private.
#[cfg(unix)]
pub(crate) fn enforce_private_cache_root(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(|source| {
        LocalError::Io {
            operation: "restrict cache root to owner-only",
            path: root.to_owned(),
            source,
        }
    })?;
    let mode = fs::symlink_metadata(root)
        .map_err(|source| LocalError::Io {
            operation: "inspect cache root permissions",
            path: root.to_owned(),
            source,
        })?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(LocalError::CacheNotPrivate {
            path: root.to_owned(),
            reason: format!("filesystem mode {mode:o} still allows group/world access"),
        });
    }
    Ok(())
}

/// Broad, multi-user principals that must never retain access to the cache.
///
/// Matched by the well-known English `icacls` names plus a couple of common
/// localizations; the primary guarantee is the `/inheritance:r` strip, which is
/// SID-based and locale-independent, so this listing is a defense-in-depth
/// verification rather than the sole enforcement.
/// Each entry is matched with its ACE `:` suffix (icacls renders an access
/// entry as `principal:(perms)`), so a cache path that merely *contains* one of
/// these words (for example `C:\Users\...`) is not a false positive.
#[cfg(windows)]
const BROAD_WINDOWS_PRINCIPALS: [&str; 5] = [
    "Everyone:",
    "Authenticated Users:",
    "\\Users:",
    "Todos:", // es-* localization of "Everyone"
    "Jeder:", // de-* localization of "Everyone"
];

#[cfg(windows)]
pub(crate) fn enforce_private_cache_root(root: &Path) -> Result<()> {
    let sid = current_windows_sid(root)?;
    set_owner_only_windows_dacl(root, &sid)?;
    verify_private_windows_dacl(root)
}

/// Resolves the current process token's SID through the standard Windows CLI.
#[cfg(windows)]
fn current_windows_sid(root: &Path) -> Result<String> {
    let mut cmd = std::process::Command::new("whoami");
    cmd.args(["/user", "/fo", "csv", "/nh"]);
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::CREATE_NO_WINDOW);
    }
    let output = cmd.output().map_err(|source| LocalError::Io {
        operation: "run whoami to resolve cache owner SID",
        path: root.to_owned(),
        source,
    })?;
    parse_whoami_user_sid(
        root,
        output.status.success(),
        &output.stdout,
        &output.stderr,
    )
}

/// Removes inherited ACEs and grants the current process SID sole full control.
#[cfg(windows)]
fn set_owner_only_windows_dacl(root: &Path, sid: &str) -> Result<()> {
    let mut cmd = std::process::Command::new("icacls");
    cmd.arg(root)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(windows_sid_grant(sid));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::CREATE_NO_WINDOW);
    }
    let output = cmd.output().map_err(|source| LocalError::Io {
        operation: "run icacls to restrict cache DACL",
        path: root.to_owned(),
        source,
    })?;
    if !output.status.success() {
        return Err(LocalError::CacheNotPrivate {
            path: root.to_owned(),
            reason: format!(
                "icacls restriction failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(())
}

/// Verifies no broad multi-user principal retains access after the restriction.
#[cfg(windows)]
fn verify_private_windows_dacl(root: &Path) -> Result<()> {
    let mut cmd = std::process::Command::new("icacls");
    cmd.arg(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::CREATE_NO_WINDOW);
    }
    let output = cmd.output().map_err(|source| LocalError::Io {
        operation: "run icacls to verify cache DACL",
        path: root.to_owned(),
        source,
    })?;
    if !output.status.success() {
        return Err(LocalError::CacheNotPrivate {
            path: root.to_owned(),
            reason: "could not read back the cache DACL to verify it".to_owned(),
        });
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    for principal in BROAD_WINDOWS_PRINCIPALS {
        if listing.contains(principal) {
            return Err(LocalError::CacheNotPrivate {
                path: root.to_owned(),
                reason: format!(
                    "a broad principal ({}) still has DACL access",
                    principal.trim_end_matches(':')
                ),
            });
        }
    }
    Ok(())
}
