//! Builds the workshop UI bundle before the Rust compile: esbuild on
//! `ui/src/main.ts` plus copies of the static assets, all written to
//! `$OUT_DIR/ui-dist/` (never into the repository). The crate version is
//! baked into the bundle as `__APP_VERSION__`. Requires Node.js 22 and
//! one `npm ci` in `ui/` per checkout; see the crate README. Under the
//! `headless` feature the UI build is skipped and the asset directory is
//! left empty: the asset routes serve through the no-op implementation,
//! so server-only integration tests need neither Node.js nor the bundle.

fn main() -> std::process::ExitCode {
    if std::env::var_os("CARGO_FEATURE_HEADLESS").is_some() {
        return empty_asset_dir();
    }
    match build_ui::build(build_ui::UiBuild {
        static_files: build_ui::WORKSHOP_STATIC_FILES,
        define_app_version: true,
        splitting: true,
    }) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Creates the empty `$OUT_DIR/ui-dist/` the headless build embeds; the
/// folder must exist for the rust-embed derive.
fn empty_asset_dir() -> std::process::ExitCode {
    let Some(out_dir) = std::env::var_os("OUT_DIR") else {
        eprintln!("OUT_DIR is not set; run through cargo");
        return std::process::ExitCode::FAILURE;
    };
    let dist_dir = std::path::Path::new(&out_dir).join("ui-dist");
    match std::fs::create_dir_all(&dist_dir) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("create {}: {error}", dist_dir.display());
            std::process::ExitCode::FAILURE
        }
    }
}
