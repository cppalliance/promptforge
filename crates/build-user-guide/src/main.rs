//! Stages the PromptForge guide for the docs site: `stage <out>` checks
//! every set's chapters under `src/<set>/` in the `promptforge-docs`
//! checkout that `PROMPTFORGE_DOCS` names, then writes under the absolute
//! folder `<out>` one mdBook source tree per book and, beside them, the
//! per-set single-file exports `promptforge-<set>-guide.md` (see
//! `stage.rs`). Each book's `book.toml` and the mdBook chrome come from
//! this repository's `guide/`.
//!
//! Chapter files have a numeric prefix (`01-frontmatter.md`) so a name sort
//! is the reading order. The generator owns the chapters; this crate owns the
//! exports, which are never hand-edited.

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

mod stage;

/// The books in audience order, each with its part title. Every book holds
/// one set, named like the book, so `src/<book>/` in the docs checkout is
/// its source. This is the only list of books; nothing else names them.
const BOOKS: &[(&str, &str)] = &[
    ("gateway", "The Gateway"),
    ("workshop", "The Workshop"),
    ("language", "The Prompt Language"),
];

/// The environment variable naming the root of the `promptforge-docs`
/// checkout.
const DOCS_VAR: &str = "PROMPTFORGE_DOCS";

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
    let guide = workspace_root().join("guide");
    let docs = env::var_os(DOCS_VAR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let result = match args.as_slice() {
        [mode, out] if mode == "stage" => stage::stage(&guide, docs.as_deref(), Path::new(out)),
        _ => Err(AssembleError(format!(
            "usage: build-user-guide stage <absolute-out>, got {args:?}"
        ))),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

/// Rejects guide text that presents the removed legacy STT section as usable.
fn check_removed_workshop_stt_claims(src: &Path) -> Result<(), AssembleError> {
    for (set, _) in BOOKS {
        let set_dir = src.join(set);
        for chapter in read_chapters(&set_dir)? {
            let path = set_dir.join(chapter.file_name);
            let content = fs::read_to_string(&path)
                .map_err(|e| AssembleError(format!("cannot read {}: {e}", path.display())))?;
            for (index, line) in content.lines().enumerate() {
                if line.contains("[workshop.stt]") && !line.to_ascii_lowercase().contains("reject")
                {
                    return Err(AssembleError(format!(
                        "removed [workshop.stt] section is not described as rejected in {}:{}",
                        path.display(),
                        index + 1
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Lists a set directory's chapter files in reading order, reading each
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
        let entry =
            entry.map_err(|e| AssembleError(format!("cannot read {}: {e}", set_dir.display())))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            && name != "index.md"
        {
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
            .ok_or_else(|| AssembleError(format!("no H1 title in {}", path.display())))?
            .to_owned();
        chapters.push(Chapter {
            file_name: name,
            title,
        });
    }
    Ok(chapters)
}

/// Renders a part landing page: the part title and its chapter list.
fn render_index(part_title: &str, chapters: &[Chapter]) -> String {
    let mut out = format!("# {part_title}\n");
    for chapter in chapters {
        let _ = write!(out, "\n- [{}]({})", chapter.title, chapter.file_name);
    }
    out.push('\n');
    out
}

/// Renders a book's SUMMARY.md: its one part, opening on the overview, with
/// every chapter linked as a sibling of SUMMARY.md. There is no
/// introduction entry, so the book opens on its `index.md`.
fn render_summary(part_title: &str, chapters: &[Chapter]) -> String {
    let mut out = format!("# Summary\n\n# {part_title}\n\n- [Overview](index.md)\n");
    for chapter in chapters {
        let _ = writeln!(out, "- [{}]({})", chapter.title, chapter.file_name);
    }
    out
}

/// Renders a set's single-file export: the chapters concatenated in reading
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

/// Verifies that every relative link target in SUMMARY.md resolves to a file
/// under `src/`.
fn check_links(summary: &str, src: &Path) -> Result<(), AssembleError> {
    for line in summary.lines() {
        let Some(start) = line.find("](") else {
            continue;
        };
        let Some(end) = line[start + 2..].find(')') else {
            continue;
        };
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

/// Writes a file, creating no directories and failing loudly on error.
fn write_file(path: &Path, content: &str) -> Result<(), AssembleError> {
    fs::write(path, content)
        .map_err(|e| AssembleError(format!("cannot write {}: {e}", path.display())))
}

/// Walks up from this crate's manifest dir to find the workspace root.
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

    /// A fake of the split layout, both trees under one temporary folder.
    pub(crate) struct FakeGuide {
        /// Stands in for promptforge's `guide/`: every book's `book.toml`
        /// and the mdBook back-link script.
        pub(crate) guide: PathBuf,
        /// Stands in for a `promptforge-docs` checkout: `src/introduction.md`
        /// and every set in `BOOKS` under `src/`.
        pub(crate) docs: PathBuf,
        _root: tempfile::TempDir,
    }

    /// Builds a [`FakeGuide`] whose workshop set holds two chapters and
    /// every other set one.
    pub(crate) fn fake_guide() -> FakeGuide {
        let root = tempfile::tempdir().expect("tempdir");
        let guide = root.path().join("guide");
        let docs = root.path().join("docs");
        for (book, _) in BOOKS {
            let book_dir = guide.join("books").join(book);
            fs::create_dir_all(&book_dir).expect("mkdir book");
            fs::write(
                book_dir.join("book.toml"),
                format!("[book]\ntitle = \"{book}\"\n"),
            )
            .expect("book.toml");
        }
        let chrome = guide.join("chrome");
        fs::create_dir_all(&chrome).expect("mkdir chrome");
        fs::write(chrome.join("back-link.js"), "// All docs link.\n").expect("back-link.js");
        let src = docs.join("src");
        for (set, _) in BOOKS {
            fs::create_dir_all(src.join(set)).expect("mkdir set");
        }
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
        for (set, _) in BOOKS.iter().filter(|(set, _)| *set != "workshop") {
            fs::write(src.join(set).join("01-start.md"), "# Start\n\nBody.\n").expect("chapter");
        }
        FakeGuide {
            guide,
            docs,
            _root: root,
        }
    }

    /// The fake's workshop set directory.
    fn workshop_set(fake: &FakeGuide) -> PathBuf {
        fake.docs.join("src").join("workshop")
    }

    #[test]
    fn chapters_sort_in_reading_order_and_read_titles() {
        let fake = fake_guide();
        let chapters = read_chapters(&workshop_set(&fake)).expect("chapters");
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
        let fake = fake_guide();
        let chapters = read_chapters(&workshop_set(&fake)).expect("chapters");
        let index = render_index("The Workshop", &chapters);
        assert!(index.starts_with("# The Workshop\n"));
        assert!(index.contains("- [The Window](01-the-window.md)"));
        assert!(index.contains("- [The Editor](02-the-editor.md)"));
    }

    #[test]
    fn summary_opens_on_the_overview_and_links_chapters_as_siblings() {
        let fake = fake_guide();
        let chapters = read_chapters(&workshop_set(&fake)).expect("chapters");
        let summary = render_summary("The Workshop", &chapters);
        assert_eq!(
            summary,
            "# Summary\n\n# The Workshop\n\n- [Overview](index.md)\n\
             - [The Window](01-the-window.md)\n- [The Editor](02-the-editor.md)\n"
        );
    }

    #[test]
    fn link_check_rejects_a_missing_target() {
        let fake = fake_guide();
        let summary = "# Summary\n\n- [Gone](99-gone.md)\n";
        let error = check_links(summary, &workshop_set(&fake)).expect_err("must fail");
        assert!(error.to_string().contains("99-gone.md"));
    }
}
