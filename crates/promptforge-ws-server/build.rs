//! Builds the workshop UI bundle before the Rust compile.
//!
//! Runs esbuild on `ui/src/main.ts` into `ui/dist/app.js` and copies the
//! static assets (`ui/index.html`, `ui/style.css`, ...) into `ui/dist/`,
//! which `rust-embed` then serves from disk (debug) or embeds (release).
//!
//! Requires Node.js on `PATH` and one `npm install` in `ui/` per checkout
//! (see the crate README). The local `ui/node_modules/.bin/esbuild` is
//! preferred; without it the build falls back to `npx esbuild`, which may
//! download esbuild on first use.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Static UI files copied verbatim into `ui/dist/`. Mirrored in
/// `ui/build.mjs`.
const STATIC_FILES: &[&str] = &[
    "index.html",
    "style.css",
    "pcm-worklet.js",
    "icons/promptforge-icon-1.png",
];

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
        ui_dir.join("check-layers.mjs").display()
    );
    println!(
        "cargo::rerun-if-changed={}",
        ui_dir.join("package.json").display()
    );
    // esbuild reads tsconfig.json from its working directory, and the
    // lockfile pins the dependency code that lands in the bundle; both can
    // change dist/ output without touching ui/src.
    for file in ["tsconfig.json", "package-lock.json"] {
        println!("cargo::rerun-if-changed={}", ui_dir.join(file).display());
    }

    // dist/ is rebuilt from scratch so removed assets never linger into the
    // release embed.
    if dist_dir.exists() {
        std::fs::remove_dir_all(&dist_dir).map_err(|error| format!("clear ui/dist: {error}"))?;
    }
    check_layers(&ui_dir)?;
    bundle(&ui_dir)?;
    copy_static(&ui_dir, &dist_dir)?;
    Ok(())
}

/// Runs the UI layer-rule walk (`ui/check-layers.mjs`) before bundling, so
/// an import that crosses the layer boundaries fails `cargo build`. The
/// esbuild CLI invocation in `bundle` cannot load plugins, hence this
/// spawned check; `build.mjs` enforces the same rule through an esbuild
/// plugin. Unlike the npm shims, `node` is a real executable on every
/// platform, so no `cmd /c` indirection is needed.
fn check_layers(ui_dir: &Path) -> Result<(), String> {
    let output = Command::new("node")
        .arg("check-layers.mjs")
        .current_dir(ui_dir)
        .output()
        .map_err(|error| {
            format!("node could not be started: {error}; install Node.js so it is on PATH")
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
    // Release builds embed the bundle in the binary; minify what we embed.
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        command.arg("--minify");
    }
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
