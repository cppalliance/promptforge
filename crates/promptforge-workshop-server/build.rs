//! Builds the workshop UI bundle before the Rust compile: esbuild on
//! `ui/src/main.ts` plus copies of the static assets, all written to
//! `$OUT_DIR/ui-dist/` (never into the repository). The layer-rule check
//! runs before bundling, and the crate version is baked into the bundle
//! as `__APP_VERSION__`. Requires Node.js 22 and one `npm ci` in `ui/`
//! per checkout; see the crate README.

fn main() -> std::process::ExitCode {
    match ui_build::build(ui_build::UiBuild {
        static_files: ui_build::WORKSHOP_STATIC_FILES,
        layer_check: true,
        define_app_version: true,
    }) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
