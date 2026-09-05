//! Embeds the program icon into `promptforge-gateway.exe` on Windows, so
//! Explorer, Task Manager, and the taskbar show the orange-P shield
//! instead of the generic executable glyph. On every other host this
//! script only declares its input and exits.
//!
//! The icon lives in `crates/workshop/icons/icon.ico`, outside this
//! crate, because the workshop's Tauri bundle is the one source of the
//! icon set. That path would break `cargo package`, which only sees the
//! crate's own files, but the gateway is `publish = false`, so the
//! out-of-crate path is accepted.

use std::path::{Path, PathBuf};

/// The icon, relative to this crate's manifest directory.
const ICON: &str = "../workshop/icons/icon.ico";

fn main() -> Result<(), String> {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or("CARGO_MANIFEST_DIR is not set; run through cargo")?,
    );
    let icon = manifest_dir.join(ICON);
    println!("cargo::rerun-if-changed={}", icon.display());
    embed_icon(&icon)
}

/// Writes a one-line resource script into `OUT_DIR` naming `icon` and
/// compiles it with `rc.exe` (MSVC) or `windres` (GNU) through
/// `embed-resource`, which links the result into every binary target.
#[cfg(windows)]
fn embed_icon(icon: &Path) -> Result<(), String> {
    use embed_resource::CompilationResult;

    let out_dir =
        PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is not set; run through cargo")?);
    let icon = icon
        .to_str()
        .ok_or_else(|| format!("the icon path {} is not UTF-8", icon.display()))?;
    // The resource compiler reads the file name as a C string literal, so
    // path separators need doubling. Resource id 1: Explorer shows the
    // first icon group in the resource table, and this exe has one.
    let script = format!("1 ICON \"{}\"\n", icon.replace('\\', "\\\\"));
    let script_path = out_dir.join("promptforge-gateway.rc");
    std::fs::write(&script_path, script)
        .map_err(|error| format!("write {}: {error}", script_path.display()))?;
    match embed_resource::compile(&script_path, embed_resource::NONE) {
        CompilationResult::Ok | CompilationResult::NotWindows => Ok(()),
        // A toolchain without a resource compiler still builds a working
        // gateway; the icon is cosmetic, so surface the gap without
        // failing the build.
        CompilationResult::NotAttempted(reason) => {
            println!("cargo::warning=the exe icon was not embedded: {reason}");
            Ok(())
        }
        CompilationResult::Failed(reason) => {
            Err(format!("compile the exe icon resource: {reason}"))
        }
    }
}

/// Nothing to embed: only Windows executables carry icon resources.
#[cfg(not(windows))]
fn embed_icon(_icon: &Path) -> Result<(), String> {
    Ok(())
}
