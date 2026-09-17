//! Integration tests for the `build-ui` helper.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Differential behavior test over real build outputs: the Rust
/// implementer (`build_ui::build_in`, what `cargo build` runs through
/// the crate build scripts) and the Node implementer
/// (`ui/build.mjs --out`, the fast-iteration path) must emit the same
/// layout - the same hash-normalized file set, the same manifest, and
/// the same stamped index page. Skips with a message when Node.js or
/// the UI's `node_modules` install is absent.
#[test]
fn both_implementers_emit_the_same_layout() -> anyhow::Result<()> {
    let ui_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("workshop")
        .join("server")
        .join("ui");
    if !node_available() {
        eprintln!("skipping: node is not on PATH; install Node.js 22 to run this test");
        return Ok(());
    }
    if !ui_dir.join("node_modules").is_dir() {
        eprintln!(
            "skipping: {} is missing; run `npm ci` in {} first",
            ui_dir.join("node_modules").display(),
            ui_dir.display()
        );
        return Ok(());
    }

    let rust_dist = tempfile::TempDir::new()?;
    let node_dist = tempfile::TempDir::new()?;

    build_ui::build_in(
        &ui_dir,
        rust_dist.path(),
        build_ui::UiBuild {
            static_files: build_ui::WORKSHOP_STATIC_FILES,
            define_app_version: true,
            splitting: true,
        },
    )?;

    let output = Command::new("node")
        .arg("build.mjs")
        .arg("--out")
        .arg(node_dist.path())
        .current_dir(&ui_dir)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "node build.mjs --out failed (status {}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let rust_files = file_set(rust_dist.path())?;
    let node_files = file_set(node_dist.path())?;
    anyhow::ensure!(
        rust_files == node_files,
        "the emitted file sets differ:\nonly in the Rust build: {:?}\nonly in the Node build: {:?}",
        rust_files.difference(&node_files).collect::<Vec<_>>(),
        node_files.difference(&rust_files).collect::<Vec<_>>(),
    );

    // The manifest maps the logical names to the hashed files; the
    // stamped index page references them. Both are byte-identical once
    // the content hashes are normalized.
    let manifest = normalized_file(rust_dist.path(), "manifest.json")?;
    anyhow::ensure!(
        manifest.contains("\"app.js\"") && manifest.contains("\"app.css\""),
        "manifest.json must map the app.js and app.css logical names, got:\n{manifest}"
    );
    for name in ["manifest.json", "index.html"] {
        let rust_text = normalized_file(rust_dist.path(), name)?;
        let node_text = normalized_file(node_dist.path(), name)?;
        anyhow::ensure!(
            rust_text == node_text,
            "{name} differs between the implementers:\nRust:\n{rust_text}\nNode:\n{node_text}"
        );
    }
    Ok(())
}

/// True when `node` runs on PATH.
fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// The set of files under `root`, relative with `/` separators and with
/// esbuild content hashes normalized so the two implementers' different
/// bytes (minification, defines) do not break the comparison.
fn file_set(root: &Path) -> anyhow::Result<BTreeSet<String>> {
    let mut files = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(normalize_hashes(&relative));
            }
        }
    }
    Ok(files)
}

/// Reads one file under `root` with its content hashes normalized.
fn normalized_file(root: &Path, name: &str) -> anyhow::Result<String> {
    Ok(normalize_hashes(&std::fs::read_to_string(root.join(name))?))
}

/// Replaces esbuild content hashes (`-XXXXXXXX` in base32, A-Z and 2-7)
/// with a fixed token.
fn normalize_hashes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '-' {
            let candidate = &rest[1..];
            let hash: String = candidate.chars().take(8).collect();
            let bounded = candidate
                .chars()
                .nth(8)
                .is_none_or(|next| !next.is_ascii_alphanumeric());
            if hash.len() == 8
                && hash.chars().all(|h| matches!(h, 'A'..='Z' | '2'..='7'))
                && bounded
            {
                out.push_str("-HASH");
                rest = &candidate[8..];
                continue;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}
