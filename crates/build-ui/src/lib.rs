//! Shared build-script helper that bundles a UI package's TypeScript
//! sources with esbuild into the Cargo build output directory.
//!
//! Both UI crates drive their entire UI build through this crate:
//! `gateway-config-ui` keeps its package nested at `<crate>/ui/` and calls
//! [`build`]; `workshop-server` builds its sibling package at
//! `crates/workshop/ui/` through [`build_sibling`]. Either way the bundle
//! and copies of the static files land in `$OUT_DIR/ui-dist/`, which git
//! never tracks, so no build step can dirty the repository. Cargo's own
//! change detection decides when the bundle is rebuilt. Splitting builds
//! content-hash every bundle file and emit a `manifest.json` plus a
//! stamped `index.html`; non-splitting builds keep the unversioned
//! `app.js`. Building requires Node.js 22 and one `npm ci` per install
//! root - the UI package itself, or the npm workspace root it is a member
//! of; there is no fallback.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Static files copied next to the workshop UI bundle, relative to `ui/`.
pub const WORKSHOP_STATIC_FILES: &[&str] = &[
    "index.html",
    "style.css",
    "pcm-worklet.js",
    "icons/promptforge-icon.png",
    "icons/promptforge-icon@2x.png",
];

/// Static files copied next to the config UI bundle, relative to `ui/`.
pub const CONFIG_UI_STATIC_FILES: &[&str] = &[
    "index.html",
    "icons/promptforge-icon.png",
    "icons/promptforge-icon@2x.png",
];

/// One crate's UI build configuration.
#[derive(Clone, Copy, Debug)]
pub struct UiBuild {
    /// Files to copy next to the bundle, relative to the ui folder.
    pub static_files: &'static [&'static str],
    /// Bakes the crate version into the bundle as the `__APP_VERSION__`
    /// define.
    pub define_app_version: bool,
    /// Code-split the bundle: dynamic imports become lazily loaded chunks
    /// under `chunks/`, and every bundle file is content-hashed - the
    /// entry lands at `bundle/app-<hash>.js` (plus its extracted
    /// `bundle/app-<hash>.css`), the chunks at `chunks/<name>-<hash>`.
    /// The build then writes `manifest.json` (the logical-to-hashed name
    /// map the workshop server's asset routes resolve through) and stamps
    /// the dist copy of `index.html` with the hashed URLs. The workshop
    /// UI splits (its panel registry lazy-loads feature directories); the
    /// config UI does not, and keeps its unversioned `app.js`.
    pub splitting: bool,
}

/// Runs the UI build: declares the watched inputs, bundles
/// `ui/src/main.ts` with esbuild into `$OUT_DIR/ui-dist/` (minified
/// in the release profile), and copies the static files next to the
/// bundle. Splitting builds finish with `finalize_hashing`: the
/// manifest and the stamped index page.
///
/// # Errors
/// Returns an error when not run through Cargo, when `<crate>/ui/` is not
/// a directory, when a `package.json` at or above it up to its install
/// root exists but cannot be read or parsed, when no esbuild install is
/// found there or esbuild fails, or when a static file cannot be copied.
pub fn build(config: UiBuild) -> anyhow::Result<()> {
    build_sibling("ui", config)
}

/// The general form of [`build`]: `relative` is joined onto
/// `CARGO_MANIFEST_DIR` to locate the UI package, so it serves both the
/// nested layout (`"ui"`, which [`build`] passes) and a package outside
/// the crate (the workshop server passes `"../ui"` for its sibling at
/// `crates/workshop/ui/`). Output lands in `$OUT_DIR/ui-dist/` either
/// way.
///
/// # Errors
/// Returns an error when not run through Cargo, when the resolved
/// package directory does not exist, when a `package.json` at or above
/// it up to its install root exists but cannot be read or parsed, when
/// no esbuild install is found there or esbuild fails, or when a static
/// file cannot be copied.
pub fn build_sibling(relative: &str, config: UiBuild) -> anyhow::Result<()> {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| anyhow::anyhow!("CARGO_MANIFEST_DIR is not set; run through cargo"))?,
    );
    let out_dir = PathBuf::from(
        std::env::var_os("OUT_DIR")
            .ok_or_else(|| anyhow::anyhow!("OUT_DIR is not set; run through cargo"))?,
    );
    let ui_dir = manifest_dir.join(relative);
    anyhow::ensure!(
        ui_dir.is_dir(),
        "the UI package {relative} resolved to {}, which is not a directory",
        ui_dir.display()
    );
    let dist_dir = out_dir.join("ui-dist");

    watch(&ui_dir, &config)?;
    build_in(&ui_dir, &dist_dir, config)
}

/// Path-explicit variant of [`build`]: bundles `ui_dir/src/main.ts` into
/// `dist_dir` without reading Cargo's environment. Tests use it to run
/// the Rust implementer against a scratch output directory (setting
/// process environment would require `unsafe`), and to compare the
/// result against the Node build script's `--out` output.
///
/// # Errors
/// Returns an error when no esbuild install is found at or above
/// `ui_dir` up to its install root or esbuild fails, or when a static
/// file cannot be copied.
pub fn build_in(ui_dir: &Path, dist_dir: &Path, config: UiBuild) -> anyhow::Result<()> {
    // The output tree is rebuilt from scratch so removed assets never
    // linger into what debug builds serve and release builds embed.
    if dist_dir.exists() {
        std::fs::remove_dir_all(dist_dir)
            .map_err(|error| anyhow::anyhow!("clear {}: {error}", dist_dir.display()))?;
    }
    bundle(ui_dir, dist_dir, &config)?;
    copy_static(ui_dir, dist_dir, config.static_files)?;
    if config.splitting {
        finalize_hashing(dist_dir)?;
    }
    Ok(())
}

/// Tells Cargo what to watch: the sources, the static files, and the
/// build inputs whose contents change the bundle. Cargo watches a
/// directory recursively, so one line covers every file under `ui/src`.
fn watch(ui_dir: &Path, config: &UiBuild) -> anyhow::Result<()> {
    for path in watched_paths(ui_dir, config)? {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    Ok(())
}

/// The paths [`watch`] declares, keeping only those that exist: Cargo
/// reruns a build script on every build while a watched path is missing.
fn watched_paths(ui_dir: &Path, config: &UiBuild) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = vec![ui_dir.join("src")];
    paths.extend(config.static_files.iter().map(|file| ui_dir.join(file)));
    // esbuild reads tsconfig.json from its working directory, and the
    // lockfile pins the dependency code that lands in the bundle; both can
    // change the output without touching ui/src.
    paths.extend(["build.mjs", "package.json", "tsconfig.json"].map(|file| ui_dir.join(file)));
    paths.extend(nearest_lockfile(ui_dir));
    // In an npm workspace the root manifest and the sibling members the UI
    // imports through workspace links change the bundle too. The UI's own
    // directory stays out: its inputs are listed above, and the jsdom
    // tests write its dist/.
    if let Some(workspace) = workspace_members(ui_dir)? {
        paths.push(workspace.root.join("package.json"));
        paths.extend(workspace.members);
    }
    // Both UIs bundle the shared-ui package (a `file:` dependency at
    // crates/shared-ui); its sources change the bundle without touching
    // ui/src. The UIs sit at different depths under crates/, so search
    // upward for the crates/ directory instead of counting parents.
    paths.extend(
        ui_dir
            .ancestors()
            .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "crates"))
            .map(|crates| crates.join("shared-ui")),
    );
    paths.retain(|path| path.exists());
    Ok(paths)
}

/// An npm workspace root and its member directories other than the UI's.
#[derive(Debug)]
struct Workspace {
    root: PathBuf,
    members: Vec<PathBuf>,
}

/// Finds the nearest `package.json` at or above `ui_dir`, up to and
/// including its [`install_root`], whose `workspaces` field is an array,
/// returning its directory and every listed member except `ui_dir`.
/// Entries are taken literally; npm's glob patterns are not expanded. A
/// manifest that exists but cannot be read or parsed is an error rather
/// than a skip: skipping a broken workspace root would drop it and its
/// members from the watch list.
fn workspace_members(ui_dir: &Path) -> anyhow::Result<Option<Workspace>> {
    let install_root = install_root(ui_dir);
    let ui_dir = normalize(ui_dir);
    for dir in ui_dir
        .ancestors()
        .take_while(|dir| dir.starts_with(&install_root))
    {
        let manifest_path = dir.join("package.json");
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => anyhow::bail!(
                "read {} to find the npm workspace root: {error}",
                manifest_path.display()
            ),
        };
        let manifest: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
            anyhow::anyhow!(
                "parse {} to find the npm workspace root: {error}",
                manifest_path.display()
            )
        })?;
        let Some(entries) = manifest
            .get("workspaces")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        let members = entries
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(|entry| normalize(&dir.join(entry)))
            .filter(|member| *member != ui_dir)
            .collect();
        return Ok(Some(Workspace {
            root: dir.to_path_buf(),
            members,
        }));
    }
    Ok(None)
}

/// The nearest `package-lock.json` at or above `ui_dir`: the UI's own for
/// a standalone package, the root's for an npm workspace member.
fn nearest_lockfile(ui_dir: &Path) -> Option<PathBuf> {
    normalize(ui_dir)
        .ancestors()
        .map(|dir| dir.join("package-lock.json"))
        .find(|lockfile| lockfile.is_file())
}

/// The directory the upward searches stop at: the one holding the
/// nearest `package-lock.json`, where `npm ci` installs, or `ui_dir`
/// itself when there is no lockfile.
fn install_root(ui_dir: &Path) -> PathBuf {
    nearest_lockfile(ui_dir)
        .and_then(|lockfile| lockfile.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| normalize(ui_dir))
}

/// Finds the npm esbuild shim (`node_modules/.bin/esbuild`, or
/// `esbuild.cmd` on Windows) in `ui_dir` or the nearest ancestor that has
/// one, searching up to and including the install root and never above
/// it: the directory holding the nearest `package-lock.json`, or `ui_dir`
/// itself when there is none. A standalone UI finds only its own install,
/// and an npm workspace member finds the install hoisted to its root.
#[must_use]
pub fn find_esbuild(ui_dir: &Path) -> Option<PathBuf> {
    let shim = if cfg!(windows) {
        "esbuild.cmd"
    } else {
        "esbuild"
    };
    let install_root = install_root(ui_dir);
    normalize(ui_dir)
        .ancestors()
        .take_while(|dir| dir.starts_with(&install_root))
        .map(|dir| dir.join("node_modules").join(".bin").join(shim))
        .find(|path| path.is_file())
}

/// Folds `.` and `..` lexically so the ancestor searches walk the real
/// parent chain: the workshop server passes `crates/workshop/server/../ui`,
/// whose lexical ancestors include `server`.
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if normal.file_name().is_some() => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}

/// Runs the esbuild bundle step from the npm install [`find_esbuild`]
/// locates. There is no `npx` fallback: `npx` can download a different
/// esbuild version and produce different output.
fn bundle(ui_dir: &Path, dist_dir: &Path, config: &UiBuild) -> anyhow::Result<()> {
    let mut command = esbuild_command(ui_dir)?;
    command.current_dir(ui_dir).args([
        "src/main.ts",
        "--bundle",
        "--format=esm",
        "--target=es2022",
    ]);
    if config.splitting {
        // Every bundle file is content-hashed: the entry lands under
        // bundle/ (index.html is stamped with the hashed URLs by
        // finalize_hashing, and the server's asset routes resolve the
        // logical names through manifest.json); chunks are content-hashed
        // under chunks/, served by the workshop server's chunk route.
        command.arg("--splitting");
        command.arg(format!("--outdir={}", dist_dir.display()));
        command.arg("--entry-names=bundle/app-[hash]");
        command.arg("--chunk-names=chunks/[name]-[hash]");
    } else {
        command.arg(format!("--outfile={}", dist_dir.join("app.js").display()));
    }
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        command.arg("--minify");
    }
    if config.define_app_version {
        let version = std::env::var("CARGO_PKG_VERSION").map_err(|error| {
            anyhow::anyhow!("CARGO_PKG_VERSION is not set: {error}; run through cargo")
        })?;
        // Single quotes: esbuild evaluates the define value as a JS string
        // literal, and unlike double quotes they pass through `cmd /c` on
        // Windows untouched.
        command.arg(format!("--define:__APP_VERSION__='{version}'"));
    }
    let output = command.output().map_err(|error| {
        anyhow::anyhow!(
            "esbuild could not be started: {error}; install Node.js 22 so it is on PATH"
        )
    })?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "the UI bundle failed (status {}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

/// Builds the command that invokes the esbuild install [`find_esbuild`]
/// locates, failing with the setup instructions when there is none: `npm
/// ci` belongs where the nearest lockfile is. On Windows the npm shim is a
/// `.cmd` file, which only runs through `cmd /c`.
fn esbuild_command(ui_dir: &Path) -> anyhow::Result<Command> {
    let Some(esbuild) = find_esbuild(ui_dir) else {
        return Err(anyhow::anyhow!(
            "no node_modules/.bin/esbuild at or above {}; run `npm ci` in {} first",
            ui_dir.display(),
            install_root(ui_dir).display()
        ));
    };
    if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.arg("/c").arg(esbuild);
        Ok(command)
    } else {
        Ok(Command::new(esbuild))
    }
}

/// Copies the static UI files next to the bundle, keeping the relative
/// paths.
fn copy_static(ui_dir: &Path, dist_dir: &Path, static_files: &[&str]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dist_dir)
        .map_err(|error| anyhow::anyhow!("create {}: {error}", dist_dir.display()))?;
    for file in static_files {
        let target = dist_dir.join(file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| anyhow::anyhow!("create the parent for {file}: {error}"))?;
        }
        std::fs::copy(ui_dir.join(file), &target)
            .map_err(|error| anyhow::anyhow!("copy ui/{file} into the bundle output: {error}"))?;
    }
    Ok(())
}

/// Finishes a content-hashed build: writes `manifest.json` mapping the
/// logical names (`app.js`, `app.css`) to the hashed files under
/// `bundle/`, and stamps the copied `index.html` with the hashed URLs so
/// the page loads the immutable assets directly. The workshop server's
/// asset routes resolve the logical names through the manifest and mark
/// the hashed files `Cache-Control: immutable`. Mirrored in the workshop
/// UI's `build.mjs` stamp step.
fn finalize_hashing(dist_dir: &Path) -> anyhow::Result<()> {
    let script = hashed_entry(dist_dir, ".js")?;
    let styles = hashed_entry(dist_dir, ".css")?;
    let manifest = format!("{{\n  \"app.js\": \"{script}\",\n  \"app.css\": \"{styles}\"\n}}\n");
    std::fs::write(dist_dir.join("manifest.json"), manifest)
        .map_err(|error| anyhow::anyhow!("write the asset manifest: {error}"))?;
    let index_path = dist_dir.join("index.html");
    let html = std::fs::read_to_string(&index_path)
        .map_err(|error| anyhow::anyhow!("read the copied index.html: {error}"))?;
    let stamped = html
        .replace("href=\"/app.css\"", &format!("href=\"/{styles}\""))
        .replace("src=\"/app.js\"", &format!("src=\"/{script}\""));
    if stamped == html {
        return Err(anyhow::anyhow!(
            "index.html did not reference /app.js and /app.css; the stamp found nothing"
        ));
    }
    std::fs::write(&index_path, stamped)
        .map_err(|error| anyhow::anyhow!("write the stamped index.html: {error}"))?;
    Ok(())
}

/// Finds the single hashed entry output of one kind under `bundle/`,
/// returning its dist-relative path. The output tree is rebuilt from
/// scratch on every build, so exactly one match must exist.
fn hashed_entry(dist_dir: &Path, extension: &str) -> anyhow::Result<String> {
    let bundle_dir = dist_dir.join("bundle");
    let mut matches: Vec<String> = std::fs::read_dir(&bundle_dir)
        .map_err(|error| anyhow::anyhow!("list {}: {error}", bundle_dir.display()))?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("app-") && name.ends_with(extension))
        .collect();
    matches.sort_unstable();
    match matches.as_slice() {
        [name] => Ok(format!("bundle/{name}")),
        _ => Err(anyhow::anyhow!(
            "expected exactly one bundle/app-*{extension} output, found {}",
            matches.len()
        )),
    }
}

#[cfg(test)]
#[path = "lib-tests.rs"]
mod tests;
