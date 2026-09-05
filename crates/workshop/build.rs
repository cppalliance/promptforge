//! Build script for the workshop desktop app: runs tauri-build so
//! `tauri::generate_context!` sees the config-derived environment it
//! requires.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["desktop_update_supported"])),
    )?;
    Ok(())
}
