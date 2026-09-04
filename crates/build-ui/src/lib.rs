//! Shared build-script helper that bundles a crate's `ui/` TypeScript
//! sources with esbuild into the Cargo build output directory.
//!
//! Both UI crates (`workshop-server` and
//! `gateway-config-ui`) drive their entire UI build through
//! [`build`]: the bundle and copies of the static files land in
//! `$OUT_DIR/ui-dist/`, which git never tracks, so no build step can dirty
//! the repository. Cargo's own change detection decides when the bundle is
//! rebuilt; there is no manifest file and no hash. Building requires
//! Node.js 22 and one `npm ci` per `ui/` folder; there is no fallback.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Static files copied next to the workshop UI bundle, relative to `ui/`.
pub const WORKSHOP_STATIC_FILES: &[&str] = &[
    "index.html",
    "style.css",
    "pcm-worklet.js",
    "icons/promptforge-icon-1.png",
];

/// Static files copied next to the config UI bundle, relative to `ui/`.
pub const CONFIG_UI_STATIC_FILES: &[&str] = &["index.html", "icons/promptforge-icon-1.png"];

/// One crate's UI build configuration.
#[derive(Clone, Copy, Debug)]
pub struct UiBuild {
    /// Files to copy next to the bundle, relative to the ui folder.
    pub static_files: &'static [&'static str],
    /// Run the layer-rule check before bundling (workshop only).
    pub layer_check: bool,
    /// Bake the crate version into the bundle as the `__APP_VERSION__`
    /// define.
    pub define_app_version: bool,
}

/// Runs the UI build: declares the watched inputs, runs the layer check
/// when configured, bundles `ui/src/main.ts` with esbuild into
/// `$OUT_DIR/ui-dist/app.js` (minified in the release profile), and copies
/// the static files next to the bundle.
///
/// # Errors
/// Returns an error string when not run through Cargo, when the layer
/// check fails, when the local esbuild install is missing or fails, or
/// when a static file cannot be copied.
pub fn build(config: UiBuild) -> Result<(), String> {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or("CARGO_MANIFEST_DIR is not set; run through cargo")?,
    );
    let out_dir =
        PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is not set; run through cargo")?);
    let ui_dir = manifest_dir.join("ui");
    let dist_dir = out_dir.join("ui-dist");

    watch(&ui_dir, &config);

    if config.layer_check {
        layer_check(&ui_dir)?;
    }

    // The output tree is rebuilt from scratch so removed assets never
    // linger into what debug builds serve and release builds embed.
    if dist_dir.exists() {
        std::fs::remove_dir_all(&dist_dir)
            .map_err(|error| format!("clear {}: {error}", dist_dir.display()))?;
    }
    bundle(&ui_dir, &dist_dir, config.define_app_version)?;
    copy_static(&ui_dir, &dist_dir, config.static_files)?;
    Ok(())
}

/// Tells Cargo what to watch: the sources, the static files, and the
/// build inputs whose contents change the bundle. Cargo watches a
/// directory recursively, so one line covers every file under `ui/src`.
fn watch(ui_dir: &Path, config: &UiBuild) {
    println!("cargo::rerun-if-changed={}", ui_dir.join("src").display());
    for file in config.static_files {
        println!("cargo::rerun-if-changed={}", ui_dir.join(file).display());
    }
    // esbuild reads tsconfig.json from its working directory, and the
    // lockfile pins the dependency code that lands in the bundle; both can
    // change the output without touching ui/src.
    for file in [
        "build.mjs",
        "package.json",
        "package-lock.json",
        "tsconfig.json",
    ] {
        println!("cargo::rerun-if-changed={}", ui_dir.join(file).display());
    }
    if config.layer_check {
        println!(
            "cargo::rerun-if-changed={}",
            ui_dir.join("check-layers.mjs").display()
        );
    }
    // Both UIs bundle the shared-ui package (a `file:` dependency two
    // directories up); its sources change the bundle without touching
    // ui/src.
    let shared_ui = ui_dir.join("..").join("..").join("shared-ui");
    if shared_ui.is_dir() {
        println!("cargo::rerun-if-changed={}", shared_ui.display());
    }
}

/// Runs the UI layer-rule walk (`ui/check-layers.mjs`) before bundling, so
/// an import that crosses the layer boundaries fails the build. `node` is
/// a real executable on every platform, so no `cmd /c` indirection is
/// needed.
fn layer_check(ui_dir: &Path) -> Result<(), String> {
    let output = Command::new("node")
        .arg("check-layers.mjs")
        .current_dir(ui_dir)
        .output()
        .map_err(|error| {
            format!("node could not be started: {error}; install Node.js 22 so it is on PATH")
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "the UI layer check failed (status {}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

/// Runs the esbuild bundle step from the local `ui/node_modules` install.
/// There is no `npx` fallback: `npx` can download a different esbuild
/// version and produce different output.
fn bundle(ui_dir: &Path, dist_dir: &Path, define_app_version: bool) -> Result<(), String> {
    let mut command = esbuild_command(ui_dir)?;
    command.current_dir(ui_dir).args([
        "src/main.ts",
        "--bundle",
        "--format=esm",
        "--target=es2022",
    ]);
    command.arg(format!("--outfile={}", dist_dir.join("app.js").display()));
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        command.arg("--minify");
    }
    if define_app_version {
        let version = std::env::var("CARGO_PKG_VERSION")
            .map_err(|error| format!("CARGO_PKG_VERSION is not set: {error}; run through cargo"))?;
        // Single quotes: esbuild evaluates the define value as a JS string
        // literal, and unlike double quotes they pass through `cmd /c` on
        // Windows untouched.
        command.arg(format!("--define:__APP_VERSION__='{version}'"));
    }
    let output = command.output().map_err(|error| {
        format!("esbuild could not be started: {error}; install Node.js 22 so it is on PATH")
    })?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "the UI bundle failed (status {}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

/// Builds the command that invokes the local esbuild install, failing with
/// the setup instructions when `ui/node_modules` is absent. On Windows the
/// npm shim is a `.cmd` file, which only runs through `cmd /c`.
fn esbuild_command(ui_dir: &Path) -> Result<Command, String> {
    let bin_dir = ui_dir.join("node_modules").join(".bin");

    #[cfg(windows)]
    {
        let local = bin_dir.join("esbuild.cmd");
        if local.exists() {
            let mut command = Command::new("cmd");
            command.arg("/c").arg(local);
            return Ok(command);
        }
    }

    #[cfg(not(windows))]
    {
        let local = bin_dir.join("esbuild");
        if local.exists() {
            return Ok(Command::new(local));
        }
    }

    Err(format!(
        "ui/node_modules is missing; run `npm ci` in {} first",
        ui_dir.display()
    ))
}

/// Copies the static UI files next to the bundle, keeping the relative
/// paths.
fn copy_static(ui_dir: &Path, dist_dir: &Path, static_files: &[&str]) -> Result<(), String> {
    std::fs::create_dir_all(dist_dir)
        .map_err(|error| format!("create {}: {error}", dist_dir.display()))?;
    for file in static_files {
        let target = dist_dir.join(file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create the parent for {file}: {error}"))?;
        }
        std::fs::copy(ui_dir.join(file), &target)
            .map_err(|error| format!("copy ui/{file} into the bundle output: {error}"))?;
    }
    Ok(())
}
