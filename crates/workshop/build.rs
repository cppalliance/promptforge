//! Build script for the workshop desktop app: runs tauri-build so
//! `tauri::generate_context!` sees the config-derived environment it
//! requires.

fn main() {
    tauri_build::build();
}
