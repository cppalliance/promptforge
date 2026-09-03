//! Assembles the per-crate user guides into a single `promptforge-user-guide.md`
//! under `guide/`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, process};

const GUIDES: &[(&str, &str)] = &[
    ("promptforge-cli", "user-guide-promptforge-cli.md"),
    (
        "gateway-config",
        "user-guide-promptforge-gateway-config.md",
    ),
    (
        "gateway-local",
        "user-guide-promptforge-gateway-local.md",
    ),
    ("gateway-stt", "user-guide-promptforge-stt.md"),
    ("gateway", "user-guide-promptforge-gateway.md"),
    ("promptforge-core", "user-guide-promptforge-core.md"),
    (
        "promptforge-tool-picker",
        "user-guide-promptforge-tool-picker.md",
    ),
    ("promptforge-webfetch", "user-guide-promptforge-webfetch.md"),
];

fn main() {
    let workspace = workspace_root();
    let crates_dir = workspace.join("crates");
    let own_dir = crates_dir.join("make-user-guide");

    let mut out = String::new();

    // Top bookend
    let top = read_or_exit(&own_dir.join("user-guide-1.md"));
    out.push_str(&top);
    if !out.ends_with('\n') {
        out.push('\n');
    }

    // Per-crate guides, heading-demoted
    for (crate_name, guide_file) in GUIDES {
        let path = crates_dir.join(crate_name).join(guide_file);
        let content = read_or_exit(&path);
        let demoted = demote_headings(&content);

        out.push_str("\n---\n\n");
        out.push_str(&demoted);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }

    // Bottom bookend
    let bottom = read_or_exit(&own_dir.join("user-guide-2.md"));
    out.push('\n');
    out.push_str(&bottom);
    if !out.ends_with('\n') {
        out.push('\n');
    }

    let dest = workspace.join("guide").join("promptforge-user-guide.md");
    fs::write(&dest, &out).unwrap_or_else(|e| {
        eprintln!("error: cannot write {}: {e}", dest.display());
        process::exit(1);
    });

    println!("{}", dest.display());
}

/// Demote every markdown heading by one level (`#` -> `##`, etc.).
/// Tracks fence state so headings inside code blocks are left alone.
/// A fence opened with N backticks/tildes is only closed by N+ of the
/// same character, so inner fences of shorter length are ignored.
fn demote_headings(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 256);
    let mut fence: Option<(char, usize)> = None;

    for line in src.lines() {
        let trimmed = line.trim_start();
        match fence {
            None => {
                if let Some(marker) = fence_marker(trimmed) {
                    fence = Some(marker);
                } else if line.starts_with('#') {
                    out.push('#');
                }
            }
            Some((ch, len)) => {
                if let Some((close_ch, close_len)) = fence_marker(trimmed)
                    && close_ch == ch
                    && close_len >= len
                {
                    fence = None;
                }
            }
        }
        let _ = writeln!(out, "{line}");
    }
    out
}

/// If the (whitespace-trimmed) line opens or closes a fence, return
/// the fence character and its run length.
fn fence_marker(trimmed: &str) -> Option<(char, usize)> {
    for ch in ['`', '~'] {
        let count = trimmed.chars().take_while(|&c| c == ch).count();
        if count >= 3 {
            return Some((ch, count));
        }
    }
    None
}

/// Walk up from this crate's manifest dir to find the workspace root.
fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.exists()
            && let Ok(contents) = fs::read_to_string(&candidate)
            && contents.contains("[workspace]")
        {
            return dir;
        }
        if !dir.pop() {
            eprintln!("error: could not find workspace root");
            process::exit(1);
        }
    }
}

fn read_or_exit(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {e}", path.display());
        process::exit(1);
    })
}
