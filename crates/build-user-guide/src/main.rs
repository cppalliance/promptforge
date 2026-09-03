//! Assembles the PromptForge guide: walks `guide/src/<set>/`, synthesizes a
//! landing page per part, regenerates `guide/src/SUMMARY.md`, writes the
//! per-set single-file exports, and fails on any link that does not resolve.
//!
//! Chapter files carry a numeric prefix (`01-frontmatter.md`) so a name sort
//! is the reading order. The generator owns the chapters and the
//! introduction; this crate owns `SUMMARY.md` and the per-part `index.md`
//! files. Neither owned file is hand-edited.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

/// The four documentation sets, in audience order, with their part titles.
const SETS: &[(&str, &str)] = &[
    ("workshop", "The Workshop"),
    ("gateway", "The Gateway"),
    ("language", "The Prompt Language"),
    ("agent", "Agent Programs"),
];

/// One chapter file inside a set directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Chapter {
    /// The file name, for example `01-frontmatter.md`.
    file_name: String,
    /// The chapter title, read from the file's first H1 heading.
    title: String,
}

/// The error type for assembly failures.
#[derive(Debug)]
struct AssembleError(String);

impl fmt::Display for AssembleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AssembleError {}

fn main() {
    let workspace = workspace_root();
    let guide = workspace.join("guide");
    if let Err(error) = assemble(&guide) {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

/// Run the full assembly over `guide/`: landing pages, SUMMARY.md, exports,
/// and the link check.
fn assemble(guide: &Path) -> Result<(), AssembleError> {
    let src = guide.join("src");
    let intro = src.join("introduction.md");
    if !intro.is_file() {
        return Err(AssembleError(format!(
            "introduction is missing: {}",
            intro.display()
        )));
    }

    let mut parts: Vec<(&str, &str, Vec<Chapter>)> = Vec::new();
    for (set, part_title) in SETS {
        let chapters = read_chapters(&src.join(set))?;
        let index = render_index(part_title, &chapters);
        write_file(&src.join(set).join("index.md"), &index)?;
        parts.push((set, part_title, chapters));
    }

    let summary = render_summary(&parts);
    check_links(&summary, &src)?;
    write_file(&src.join("SUMMARY.md"), &summary)?;

    for (set, part_title, chapters) in &parts {
        let export = render_export(part_title, chapters, &src.join(set))?;
        write_file(&guide.join(format!("promptforge-{set}-guide.md")), &export)?;
    }
    Ok(())
}

/// List a set directory's chapter files in reading order, reading each
/// chapter's title from its first H1 heading.
fn read_chapters(set_dir: &Path) -> Result<Vec<Chapter>, AssembleError> {
    if !set_dir.is_dir() {
        return Err(AssembleError(format!(
            "set directory is missing: {}",
            set_dir.display()
        )));
    }
    let mut names: Vec<String> = Vec::new();
    let entries = fs::read_dir(set_dir)
        .map_err(|e| AssembleError(format!("cannot read {}: {e}", set_dir.display())))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| AssembleError(format!("cannot read {}: {e}", set_dir.display())))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".md") && name != "index.md" {
            names.push(name);
        }
    }
    names.sort();

    let mut chapters = Vec::new();
    for name in names {
        let path = set_dir.join(&name);
        let content = fs::read_to_string(&path)
            .map_err(|e| AssembleError(format!("cannot read {}: {e}", path.display())))?;
        let title = content
            .trim_start_matches('\u{feff}')
            .lines()
            .find_map(|line| line.strip_prefix("# "))
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .ok_or_else(|| {
                AssembleError(format!("no H1 title in {}", path.display()))
            })?
            .to_owned();
        chapters.push(Chapter {
            file_name: name,
            title,
        });
    }
    Ok(chapters)
}

/// Render a part landing page: the part title and its chapter list.
fn render_index(part_title: &str, chapters: &[Chapter]) -> String {
    let mut out = format!("# {part_title}\n");
    for chapter in chapters {
        out.push_str(&format!(
            "\n- [{}]({})",
            chapter.title, chapter.file_name
        ));
    }
    out.push('\n');
    out
}

/// Render SUMMARY.md: the introduction, then the parts in audience order
/// with every chapter linked.
fn render_summary(parts: &[(&str, &str, Vec<Chapter>)]) -> String {
    let mut out = String::from("# Summary\n\n- [Introduction](introduction.md)\n");
    for (set, part_title, chapters) in parts {
        out.push_str(&format!("\n# {part_title}\n\n- [Overview]({set}/index.md)\n"));
        for chapter in chapters {
            out.push_str(&format!(
                "- [{}]({}/{})\n",
                chapter.title, set, chapter.file_name
            ));
        }
    }
    out
}

/// Render a set's single-file export: the chapters concatenated in reading
/// order.
fn render_export(
    part_title: &str,
    chapters: &[Chapter],
    set_dir: &Path,
) -> Result<String, AssembleError> {
    let mut out = format!("# {part_title}\n");
    for chapter in chapters {
        let path = set_dir.join(&chapter.file_name);
        let content = fs::read_to_string(&path)
            .map_err(|e| AssembleError(format!("cannot read {}: {e}", path.display())))?;
        out.push_str("\n---\n\n");
        out.push_str(content.trim_end());
        out.push('\n');
    }
    Ok(out)
}

/// Verify that every relative link target in SUMMARY.md resolves to a file
/// under `src/`.
fn check_links(summary: &str, src: &Path) -> Result<(), AssembleError> {
    for line in summary.lines() {
        let Some(start) = line.find("](") else { continue };
        let Some(end) = line[start + 2..].find(')') else { continue };
        let target = &line[start + 2..start + 2 + end];
        let path = src.join(target);
        if !path.is_file() {
            return Err(AssembleError(format!(
                "SUMMARY link does not resolve: {target}"
            )));
        }
    }
    Ok(())
}

/// Write a file, creating no directories and failing loudly on error.
fn write_file(path: &Path, content: &str) -> Result<(), AssembleError> {
    fs::write(path, content)
        .map_err(|e| AssembleError(format!("cannot write {}: {e}", path.display())))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake guide tree with two sets and return its root.
    fn fake_guide() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("workshop")).expect("mkdir workshop");
        fs::create_dir_all(src.join("gateway")).expect("mkdir gateway");
        fs::create_dir_all(src.join("language")).expect("mkdir language");
        fs::create_dir_all(src.join("agent")).expect("mkdir agent");
        fs::write(src.join("introduction.md"), "# PromptForge\n").expect("intro");
        fs::write(
            src.join("workshop").join("01-the-window.md"),
            "# The Window\n\nBody.\n",
        )
        .expect("chapter 1");
        fs::write(
            src.join("workshop").join("02-the-editor.md"),
            "# The Editor\n\nBody.\n",
        )
        .expect("chapter 2");
        for set in ["gateway", "language", "agent"] {
            fs::write(
                src.join(set).join("01-start.md"),
                "# Start\n\nBody.\n",
            )
            .expect("chapter");
        }
        dir
    }

    #[test]
    fn chapters_sort_in_reading_order_and_read_titles() {
        let dir = fake_guide();
        let chapters =
            read_chapters(&dir.path().join("src").join("workshop")).expect("chapters");
        let names: Vec<&str> = chapters
            .iter()
            .map(|chapter| chapter.file_name.as_str())
            .collect();
        assert_eq!(names, ["01-the-window.md", "02-the-editor.md"]);
        assert_eq!(chapters[0].title, "The Window");
        assert_eq!(chapters[1].title, "The Editor");
    }

    #[test]
    fn index_lists_every_chapter() {
        let dir = fake_guide();
        let chapters =
            read_chapters(&dir.path().join("src").join("workshop")).expect("chapters");
        let index = render_index("The Workshop", &chapters);
        assert!(index.starts_with("# The Workshop\n"));
        assert!(index.contains("- [The Window](01-the-window.md)"));
        assert!(index.contains("- [The Editor](02-the-editor.md)"));
    }

    #[test]
    fn summary_has_parts_in_audience_order() {
        let dir = fake_guide();
        let src = dir.path().join("src");
        let parts: Vec<(&str, &str, Vec<Chapter>)> = SETS
            .iter()
            .map(|(set, title)| {
                (*set, *title, read_chapters(&src.join(set)).expect("chapters"))
            })
            .collect();
        let summary = render_summary(&parts);
        let workshop = summary.find("# The Workshop").expect("workshop part");
        let gateway = summary.find("# The Gateway").expect("gateway part");
        let language = summary.find("# The Prompt Language").expect("language part");
        let agent = summary.find("# Agent Programs").expect("agent part");
        assert!(workshop < gateway && gateway < language && language < agent);
        assert!(summary.contains("- [Introduction](introduction.md)"));
        assert!(summary.contains("- [The Window](workshop/01-the-window.md)"));
    }

    #[test]
    fn link_check_rejects_a_missing_target() {
        let dir = fake_guide();
        let src = dir.path().join("src");
        let summary = "# Summary\n\n- [Gone](workshop/99-gone.md)\n";
        let error = check_links(summary, &src).expect_err("must fail");
        assert!(error.to_string().contains("workshop/99-gone.md"));
    }

    #[test]
    fn assembly_is_deterministic() {
        let dir = fake_guide();
        assemble(dir.path()).expect("first run");
        let first = fs::read_to_string(dir.path().join("src").join("SUMMARY.md"))
            .expect("summary");
        assemble(dir.path()).expect("second run");
        let second = fs::read_to_string(dir.path().join("src").join("SUMMARY.md"))
            .expect("summary");
        assert_eq!(first, second);
        let export =
            fs::read_to_string(dir.path().join("promptforge-workshop-guide.md"))
                .expect("export");
        assert!(export.contains("# The Workshop"));
        assert!(export.contains("# The Window"));
        assert!(export.contains("# The Editor"));
    }
}
