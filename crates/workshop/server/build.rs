//! Builds the workshop UI bundle before the Rust compile: esbuild on
//! `../ui/src/main.ts` (the sibling `crates/workshop/ui/` package) plus
//! copies of the static assets, all written to `$OUT_DIR/ui-dist/` (never
//! into the repository). The crate version is baked into the bundle as
//! `__APP_VERSION__`. Requires Node.js 22 on `PATH` and one `npm ci` in
//! the `crates/workshop/` npm workspace per checkout (`npm ci --prefix
//! crates/workshop` from the repository root).

fn main() -> std::process::ExitCode {
    if let Err(error) = build_ceiling::check() {
        eprintln!("{error}");
        return std::process::ExitCode::FAILURE;
    }
    match build_ui::build_sibling(
        "../ui",
        build_ui::UiBuild {
            static_files: build_ui::WORKSHOP_STATIC_FILES,
            define_app_version: true,
            splitting: true,
        },
    ) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
