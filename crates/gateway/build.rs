//! Embeds the program icon and an application manifest into
//! `promptforge-gateway.exe` on Windows, so Explorer, Task Manager, and
//! the taskbar show the orange-P shield instead of the generic executable
//! glyph. On every other host this script only declares its input and
//! exits.
//!
//! The icon lives in `crates/workshop/icons/icon.ico`, outside this
//! crate, because the workshop's Tauri bundle is the one source of the
//! icon set. That path would break `cargo package`, which only sees the
//! crate's own files, but the gateway is `publish = false`, so the
//! out-of-crate path is accepted.
//!
//! The manifest declares the common-controls v6 dependency. Workspace
//! builds unify `muda`'s `common-controls-v6` feature on (the workshop's
//! Tauri dependency enables it), which makes the tray's predefined About
//! dialog call `TaskDialogIndirect`. Without the manifest that import
//! binds to the v5 `comctl32.dll`, which does not export it, and the exe
//! fails to start with STATUS_ENTRYPOINT_NOT_FOUND.

use std::path::{Path, PathBuf};

/// The icon, relative to this crate's manifest directory.
const ICON: &str = "../workshop/icons/icon.ico";

/// The application manifest: the common-controls v6 dependency that
/// `muda`'s `common-controls-v6` feature requires. The resource script
/// references it as `CREATEPROCESS_MANIFEST_RESOURCE_ID` (1) of type
/// `RT_MANIFEST` (24).
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn main() -> Result<(), String> {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or("CARGO_MANIFEST_DIR is not set; run through cargo")?,
    );
    let icon = manifest_dir.join(ICON);
    println!("cargo::rerun-if-changed={}", icon.display());
    embed_resources(&icon)
}

/// Writes a resource script into `OUT_DIR` naming `icon` and the manifest
/// and compiles it with `rc.exe` (MSVC) or `windres` (GNU) through
/// `embed-resource`, which links the result into every binary target.
#[cfg(windows)]
fn embed_resources(icon: &Path) -> Result<(), String> {
    use embed_resource::CompilationResult;

    let out_dir =
        PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is not set; run through cargo")?);
    let icon = icon
        .to_str()
        .ok_or_else(|| format!("the icon path {} is not UTF-8", icon.display()))?;
    let manifest_path = out_dir.join("promptforge-gateway.manifest.xml");
    std::fs::write(&manifest_path, MANIFEST)
        .map_err(|error| format!("write {}: {error}", manifest_path.display()))?;
    let manifest = manifest_path
        .to_str()
        .ok_or_else(|| format!("the manifest path {} is not UTF-8", manifest_path.display()))?;
    // The resource compiler reads the file names as C string literals, so
    // path separators need doubling. Icon resource id 1: Explorer shows
    // the first icon group in the resource table, and this exe has one.
    // Manifest resource id 1 of type 24 (RT_MANIFEST) is the process
    // manifest the loader reads before resolving imports.
    let script = format!(
        "1 ICON \"{}\"\n1 24 \"{}\"\n",
        icon.replace('\\', "\\\\"),
        manifest.replace('\\', "\\\\")
    );
    let script_path = out_dir.join("promptforge-gateway.rc");
    std::fs::write(&script_path, script)
        .map_err(|error| format!("write {}: {error}", script_path.display()))?;
    match embed_resource::compile(&script_path, embed_resource::NONE) {
        CompilationResult::Ok | CompilationResult::NotWindows => Ok(()),
        // A toolchain without a resource compiler still builds; the icon
        // is cosmetic, so the gap stays a warning. The manifest is not
        // cosmetic when feature unification turns on `common-controls-v6`,
        // but that unification only happens in workspace builds, and a
        // missing rc.exe is visible in the build output either way.
        CompilationResult::NotAttempted(reason) => {
            println!("cargo::warning=the exe resources were not embedded: {reason}");
            Ok(())
        }
        CompilationResult::Failed(reason) => Err(format!("compile the exe resources: {reason}")),
    }
}

/// Nothing to embed: only Windows executables carry icon and manifest
/// resources.
#[cfg(not(windows))]
fn embed_resources(_icon: &Path) -> Result<(), String> {
    Ok(())
}
