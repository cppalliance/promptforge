//! Operator-facing default filesystem paths (`~/.promptforge`).

use std::path::PathBuf;

/// Returns the operator home directory used for defaults.
///
/// Infallible: falls back to the working directory when unset. Used only by
/// the infallible profiles-directory default; artifact provisioning resolves
/// the home with a typed error instead, so a missing home cannot silently
/// redirect downloads into the working directory (ART-009).
#[must_use]
pub(crate) fn default_home() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map_or_else(|| PathBuf::from("."), PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
    }
}

/// Default root for gateway local artifacts (`~/.promptforge`), infallible.
#[must_use]
pub(crate) fn default_promptforge_root() -> PathBuf {
    default_home().join(".promptforge")
}
