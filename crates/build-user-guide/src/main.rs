//! Assembles the PromptForge guide: walks `guide/src/<set>/`, regenerates
//! the table of contents, and writes the per-set single-file exports.
//!
//! The four sets, in audience order.
const SETS: &[&str] = &["workshop", "gateway", "language", "agent"];

fn main() {
    let workspace = workspace_root();
    let src = workspace.join("guide").join("src");
    for set in SETS {
        println!("{}", src.join(set).display());
    }
}

/// Walk up from this crate's manifest dir to find the workspace root.
fn workspace_root() -> std::path::PathBuf {
    let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.exists()
            && let Ok(contents) = std::fs::read_to_string(&candidate)
            && contents.contains("[workspace]")
        {
            return dir;
        }
        if !dir.pop() {
            eprintln!("error: could not find workspace root");
            std::process::exit(1);
        }
    }
}
