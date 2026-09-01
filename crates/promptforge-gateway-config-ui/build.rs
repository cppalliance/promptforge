//! Builds the config UI bundle before the Rust compile: esbuild on
//! `ui/src/main.ts` plus copies of the static assets, all written to
//! `$OUT_DIR/ui-dist/` (never into the repository). The crate version is
//! baked into the bundle as `__APP_VERSION__`. Requires Node.js 22 and one
//! `npm ci` in `ui/` per checkout; see the crate README.

fn main() -> std::process::ExitCode {
    match ui_build::build(ui_build::UiBuild {
        static_files: ui_build::CONFIG_UI_STATIC_FILES,
        layer_check: false,
        define_app_version: true,
    }) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
