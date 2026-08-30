//! Builds the config UI bundle before the Rust compile.
//!
//! Debug builds run the UI build in place: esbuild on `ui/src/main.ts`
//! into `ui/dist/app.js`, plus copies of the static assets
//! (`ui/index.html`, the program icon), which `rust-embed` serves from
//! disk. Release builds embed the versioned, minified artifact in
//! `ui/dist/` (bundle plus `manifest.json`); when the artifact is absent
//! or stale against the current sources, the build produces it first with
//! `node build.mjs --package` and verifies the result, so a single
//! `cargo build --release` is sufficient. See `build/manifest.rs` for the
//! artifact contract.
//!
//! Both paths require Node.js on `PATH` and one `npm ci` in `ui/` per
//! checkout (see the crate README). The debug bundle prefers the local
//! `ui/node_modules/.bin/esbuild`; without it the build falls back to
//! `npx esbuild`, which may download esbuild on first use.

#[path = "build/manifest.rs"]
mod manifest;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use manifest::STATIC_FILES;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or("CARGO_MANIFEST_DIR is not set; run through cargo")?,
    );
    let ui_dir = manifest_dir.join("ui");
    let dist_dir = ui_dir.join("dist");

    println!("cargo::rerun-if-changed={}", ui_dir.join("src").display());
    for file in STATIC_FILES {
        println!("cargo::rerun-if-changed={}", ui_dir.join(file).display());
    }
    println!(
        "cargo::rerun-if-changed={}",
        ui_dir.join("build.mjs").display()
    );
    println!(
        "cargo::rerun-if-changed={}",
        ui_dir.join("manifest.mjs").display()
    );
    // esbuild reads tsconfig.json from its working directory, and the
    // lockfile pins the dependency code that lands in the bundle; both can
    // change dist/ output without touching ui/src.
    for file in ["package.json", "tsconfig.json", "package-lock.json"] {
        println!("cargo::rerun-if-changed={}", ui_dir.join(file).display());
    }
    // A fresh `npm run package` rewrites the manifest; watching it is what
    // re-triggers this script so a release build embeds the new artifact.
    println!(
        "cargo::rerun-if-changed={}",
        dist_dir.join("manifest.json").display()
    );

    if std::env::var("PROFILE").as_deref() == Ok("release") {
        return release_artifact(&ui_dir);
    }

    // dist/ is rebuilt from scratch so removed assets never linger in what
    // debug builds serve from disk.
    if dist_dir.exists() {
        std::fs::remove_dir_all(&dist_dir).map_err(|error| format!("clear ui/dist: {error}"))?;
    }
    bundle(&ui_dir)?;
    copy_static(&ui_dir, &dist_dir)?;
    Ok(())
}

/// Release builds embed the verified artifact from `ui/dist/`. When the
/// artifact is absent or stale against the current sources, the build
/// produces it first and verifies the result, so one
/// `cargo build --release` is enough. The build fails only when the
/// artifact cannot be produced or still does not verify.
fn release_artifact(ui_dir: &Path) -> Result<(), String> {
    if manifest::verify(ui_dir).is_ok() {
        return Ok(());
    }
    package(ui_dir)?;
    manifest::verify(ui_dir)
}

/// Runs the packaging step (`node build.mjs --package`) in `ui/`, which
/// rebuilds `dist/` from scratch: the bundle is minified, the static files
/// are copied, and the manifest is written. `node` is a real executable on
/// every platform, so no `cmd /c` indirection is needed.
fn package(ui_dir: &Path) -> Result<(), String> {
    let output = Command::new("node")
        .arg("build.mjs")
        .arg("--package")
        .current_dir(ui_dir)
        .output()
        .map_err(|error| {
            format!("node could not be started: {error}; install Node.js so it is on PATH")
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "the UI packaging step failed (status {}):\n{}\n{}\n\
         If ui/node_modules is missing, run `npm ci` in {} first.",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        ui_dir.display(),
    ))
}

/// Runs the esbuild bundle step, preferring the local install in
/// `ui/node_modules` and falling back to `npx esbuild`.
fn bundle(ui_dir: &Path) -> Result<(), String> {
    let mut command = esbuild_command(ui_dir);
    command.current_dir(ui_dir).args([
        "src/main.ts",
        "--bundle",
        "--format=esm",
        "--target=es2022",
        "--outfile=dist/app.js",
    ]);
    let output = command.output().map_err(|error| {
        format!("esbuild could not be started: {error}; install Node.js so it is on PATH")
    })?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "the UI bundle failed (status {}):\n{}\n{}\n\
         If ui/node_modules is missing, run `npm install` in {} first.",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        ui_dir.display(),
    ))
}

/// Builds the command that invokes esbuild. On Windows the npm shims are
/// `.cmd` files, which only run through `cmd /c`.
fn esbuild_command(ui_dir: &Path) -> Command {
    let bin_dir = ui_dir.join("node_modules").join(".bin");

    #[cfg(windows)]
    {
        let local = bin_dir.join("esbuild.cmd");
        if local.exists() {
            let mut command = Command::new("cmd");
            command.arg("/c").arg(&local);
            return command;
        }
        warn_no_local_install(ui_dir);
        let mut command = Command::new("cmd");
        command.arg("/c").arg("npx").arg("--yes").arg("esbuild");
        command
    }

    #[cfg(not(windows))]
    {
        let local = bin_dir.join("esbuild");
        if local.exists() {
            return Command::new(local);
        }
        warn_no_local_install(ui_dir);
        let mut command = Command::new("npx");
        command.arg("--yes").arg("esbuild");
        command
    }
}

fn warn_no_local_install(ui_dir: &Path) {
    println!(
        "cargo::warning=ui/node_modules is missing; falling back to `npx esbuild`. \
         Run `npm install` in {} once for a fast, offline-capable build.",
        ui_dir.display()
    );
}

/// Copies the static UI files into `ui/dist/` next to the bundle.
fn copy_static(ui_dir: &Path, dist_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dist_dir).map_err(|error| format!("create ui/dist: {error}"))?;
    for file in STATIC_FILES {
        let target = dist_dir.join(file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create ui/dist parent for {file}: {error}"))?;
        }
        std::fs::copy(ui_dir.join(file), &target)
            .map_err(|error| format!("copy ui/{file} into ui/dist: {error}"))?;
    }
    Ok(())
}
