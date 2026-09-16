//! Build script for the workshop desktop app: refreshes the gateway
//! sidecar from the gateway cargo most recently built, then runs
//! tauri-build so `tauri::generate_context!` sees the config-derived
//! environment it requires.
//!
//! Why the refresh exists: `tauri.conf.json` declares the gateway as an
//! `externalBin`, so tauri-build copies `binaries/promptforge-gateway-
//! <target-triple>` over `target/<profile>/promptforge-gateway` on every
//! workshop build. That is the same path cargo writes the gateway crate's
//! own binary to, so without this step a `cargo build -p gateway` followed
//! by `cargo build -p workshop` ends with the stale sidecar copy in place
//! of the gateway just built. Copying the built gateway forward into
//! `binaries/` first makes that build order correct.

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    refresh_gateway_sidecar()?;
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["desktop_update_supported", "quit"]),
    ))?;
    Ok(())
}

/// Copies `target/<profile>/promptforge-gateway` into the sidecar slot
/// when it is newer than the copy there, so tauri-build ships the gateway
/// most recently built rather than whatever was placed by hand.
fn refresh_gateway_sidecar() -> Result<(), Box<dyn Error>> {
    let target = env::var("TARGET")?;
    let exe_suffix = if env::var("CARGO_CFG_TARGET_OS")? == "windows" {
        ".exe"
    } else {
        ""
    };
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let sidecar = manifest_dir
        .join("binaries")
        .join(format!("promptforge-gateway-{target}{exe_suffix}"));

    let Some(profile_dir) = profile_dir_from_out_dir(&PathBuf::from(env::var("OUT_DIR")?)) else {
        return Ok(());
    };
    let built = profile_dir.join(format!("promptforge-gateway{exe_suffix}"));
    // A rebuilt gateway must re-run this script; a missing one is not an
    // error here because tauri-build reports the absent sidecar itself.
    println!("cargo:rerun-if-changed={}", built.display());
    if !built.is_file() {
        return Ok(());
    }
    if let Some(parent) = sidecar.parent() {
        fs::create_dir_all(parent)?;
    }
    if sidecar_is_current(&built, &sidecar) {
        return Ok(());
    }
    fs::copy(&built, &sidecar)?;
    println!(
        "cargo:warning=refreshed gateway sidecar {} from {}",
        sidecar.display(),
        built.display()
    );
    Ok(())
}

/// `OUT_DIR` is `<target>/<profile>/build/<pkg-hash>/out`; the profile
/// directory holding the built binaries is three levels up.
fn profile_dir_from_out_dir(out_dir: &Path) -> Option<PathBuf> {
    out_dir.ancestors().nth(3).map(Path::to_path_buf)
}

/// The sidecar is current when it exists with the same length and a
/// modification time no older than the built gateway's.
fn sidecar_is_current(built: &Path, sidecar: &Path) -> bool {
    let (Ok(built_meta), Ok(sidecar_meta)) = (fs::metadata(built), fs::metadata(sidecar)) else {
        return false;
    };
    if built_meta.len() != sidecar_meta.len() {
        return false;
    }
    match (built_meta.modified(), sidecar_meta.modified()) {
        (Ok(built_time), Ok(sidecar_time)) => sidecar_time >= built_time,
        _ => false,
    }
}
