//! The one toolchain `cargo xtask api` runs on. Rustdoc JSON is nightly
//! only and changes format roughly every release, so the nightly and the
//! `rustdoc-types` release that reads its `format_version` move together,
//! in [`PINNED`], and nowhere else. CI reads the nightly from here.

/// A nightly toolchain paired with the `rustdoc-types` release matching
/// its rustdoc JSON `format_version`.
#[derive(Debug)]
pub(crate) struct Pinned {
    /// The rustup toolchain name, as in `cargo +<nightly> xtask api`.
    pub(crate) nightly: &'static str,
    /// The exact `rustdoc-types` version the workspace manifest pins.
    pub(crate) rustdoc_types: &'static str,
}

/// The pinned pair. `nightly-2026-09-05` emits `format_version` 61, which
/// `rustdoc-types` 0.61.0 reads.
pub(crate) const PINNED: Pinned = Pinned {
    nightly: "nightly-2026-09-05",
    rustdoc_types: "0.61.0",
};

/// The toolchain rustup resolved for this process. Rustup sets
/// `RUSTUP_TOOLCHAIN` for every tool it proxies, to the full name with the
/// host triple (`nightly-2026-09-05-x86_64-pc-windows-msvc`).
pub(crate) fn active() -> Option<String> {
    std::env::var("RUSTUP_TOOLCHAIN").ok()
}

/// Accepts `active` only when it names the pinned nightly, with or without
/// a host triple; anything else is refused before any build starts, with
/// the command that runs on the right toolchain.
pub(crate) fn require_pinned(active: Option<&str>) -> Result<(), String> {
    let nightly = PINNED.nightly;
    let found = match active {
        Some(name) if name == nightly => return Ok(()),
        Some(name) => {
            let triple = name
                .strip_prefix(nightly)
                .and_then(|rest| rest.strip_prefix('-'));
            if triple.is_some_and(|triple| !triple.is_empty()) {
                return Ok(());
            }
            format!("toolchain `{name}`")
        }
        None => "no rustup toolchain (RUSTUP_TOOLCHAIN is unset)".to_owned(),
    };
    Err(format!(
        "cargo xtask api: required the pinned nightly `{nightly}`, found {found}; \
         run `cargo +{nightly} xtask api`"
    ))
}

#[cfg(test)]
#[path = "toolchain-tests.rs"]
mod tests;
