//! Assembles the PromptForge guide: checks every set's chapters under
//! `guide/src/<set>/` and writes the per-set single-file exports,
//! `guide/promptforge-<set>-guide.md`. With `stage <out>`, it instead
//! stages one mdBook source tree per book under the absolute folder `<out>`
//! (see `stage.rs`).
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

/// The books in audience order, each with its sets and their part titles.
/// This is the only list of books; nothing else names them.
const BOOKS: &[(&str, &[(&str, &str)])] = &[
    ("gateway", &[("gateway", "The Gateway")]),
    ("workshop", &[("workshop", "The Workshop")]),
    (
        "language",
        &[
            ("language", "The Prompt Language"),
            ("agent", "Agent Programs"),
        ],
    ),
];

/// Every set in `BOOKS`, in audience order, with its part title.
fn sets() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    BOOKS.iter().flat_map(|(_, sets)| sets.iter())
}

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
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let result = match args.as_slice() {
        [] => assemble(&guide),
        [mode, out] if mode == "stage" => stage::stage(&guide, Path::new(out)),
        _ => Err(AssembleError(format!(
            "usage: build-user-guide [stage <absolute-out>], got {args:?}"
        ))),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

/// Runs the default mode over `guide/`: the `[workshop.stt]` and H1 checks
/// on every set, then the per-set exports. Nothing is written until every
/// set passes.
fn assemble(guide: &Path) -> Result<(), AssembleError> {
    let src = guide.join("src");
    check_removed_workshop_stt_claims(&src)?;

    let mut exports = Vec::new();
    for (set, part_title) in sets() {
        let chapters = read_chapters(&src.join(set))?;
        exports.push((set, render_export(part_title, &chapters, &src.join(set))?));
    }
    for (set, export) in exports {
        write_file(&guide.join(format!("promptforge-{set}-guide.md")), &export)?;
    }
    Ok(())
}

/// Rejects guide text that presents the removed legacy STT section as usable.
fn check_removed_workshop_stt_claims(src: &Path) -> Result<(), AssembleError> {
    for (set, _) in sets() {
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

/// Renders a book's SUMMARY.md: its parts in audience order, each opening on
/// the set's overview, with every chapter linked. There is no introduction
/// entry, so the book opens on its first set's `index.md`.
fn render_summary(parts: &[(&str, &str, Vec<Chapter>)]) -> String {
    let mut out = String::from("# Summary\n");
    for (set, part_title, chapters) in parts {
        let _ = write!(out, "\n# {part_title}\n\n- [Overview]({set}/index.md)\n");
        for chapter in chapters {
            let _ = writeln!(out, "- [{}]({}/{})", chapter.title, set, chapter.file_name);
        }
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

    /// Builds a fake guide tree with every book's `book.toml`, the mdBook
    /// back-link script, and every set in `BOOKS`, and returns its root.
    pub(crate) fn fake_guide() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (book, _) in BOOKS {
            let book_dir = dir.path().join("books").join(book);
            fs::create_dir_all(&book_dir).expect("mkdir book");
            fs::write(
                book_dir.join("book.toml"),
                format!("[book]\ntitle = \"{book}\"\n"),
            )
            .expect("book.toml");
        }
        let chrome = dir.path().join("chrome");
        fs::create_dir_all(&chrome).expect("mkdir chrome");
        fs::write(chrome.join("back-link.js"), "// All docs link.\n").expect("back-link.js");
        let src = dir.path().join("src");
        for (set, _) in sets() {
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
        for (set, _) in sets().filter(|(set, _)| *set != "workshop") {
            fs::write(src.join(set).join("01-start.md"), "# Start\n\nBody.\n").expect("chapter");
        }
        dir
    }

    /// Reads every set's export from `guide`, in `BOOKS` order.
    fn read_exports(guide: &Path) -> Vec<String> {
        sets()
            .map(|(set, _)| {
                fs::read_to_string(guide.join(format!("promptforge-{set}-guide.md")))
                    .expect("export")
            })
            .collect()
    }

    #[test]
    fn chapters_sort_in_reading_order_and_read_titles() {
        let dir = fake_guide();
        let chapters = read_chapters(&dir.path().join("src").join("workshop")).expect("chapters");
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
        let chapters = read_chapters(&dir.path().join("src").join("workshop")).expect("chapters");
        let index = render_index("The Workshop", &chapters);
        assert!(index.starts_with("# The Workshop\n"));
        assert!(index.contains("- [The Window](01-the-window.md)"));
        assert!(index.contains("- [The Editor](02-the-editor.md)"));
    }

    #[test]
    fn summary_has_parts_in_audience_order() {
        let dir = fake_guide();
        let src = dir.path().join("src");
        let parts: Vec<(&str, &str, Vec<Chapter>)> = sets()
            .map(|(set, title)| {
                (
                    *set,
                    *title,
                    read_chapters(&src.join(set)).expect("chapters"),
                )
            })
            .collect();
        let summary = render_summary(&parts);
        let gateway = summary.find("# The Gateway").expect("gateway part");
        let language = summary
            .find("# The Prompt Language")
            .expect("language part");
        let agent = summary.find("# Agent Programs").expect("agent part");
        assert!(gateway < language && language < agent);
        assert!(!summary.contains("introduction.md"), "{summary}");
        assert!(summary.contains("- [Start](gateway/01-start.md)"));
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
    fn assembly_rejects_legacy_workshop_stt_acceptance_claims() {
        let dir = fake_guide();
        let chapter = dir.path().join("src").join("gateway").join("01-start.md");
        fs::write(
            chapter,
            "# Start\n\nLegacy `[workshop.stt]` input is accepted.\n",
        )
        .expect("stale chapter");
        let error = assemble(dir.path()).expect_err("must reject stale claim");
        assert!(
            error
                .to_string()
                .contains("removed [workshop.stt] section is not described as rejected")
        );
    }

    #[test]
    fn stt_check_covers_every_set() {
        for (set, _) in sets() {
            let dir = fake_guide();
            fs::write(
                dir.path().join("src").join(set).join("09-stale.md"),
                "# Stale\n\nLegacy `[workshop.stt]` input is accepted.\n",
            )
            .expect("stale chapter");
            let error = assemble(dir.path()).expect_err(set);
            assert!(error.to_string().contains("09-stale.md"), "{set}: {error}");
        }
    }

    #[test]
    fn default_mode_writes_an_export_for_every_set() {
        let dir = fake_guide();
        assemble(dir.path()).expect("assemble");
        let workshop =
            fs::read_to_string(dir.path().join("promptforge-workshop-guide.md")).expect("export");
        assert!(workshop.starts_with("# The Workshop\n"));
        assert!(workshop.contains("# The Window"));
        for ((set, title), export) in sets().zip(read_exports(dir.path())) {
            assert!(
                export.starts_with(&format!("# {title}\n")),
                "{set}: {export}"
            );
        }
    }

    #[test]
    fn default_mode_writes_no_summary_or_index() {
        let dir = fake_guide();
        let src = dir.path().join("src");
        assemble(dir.path()).expect("assemble");
        assert!(!src.join("SUMMARY.md").exists());
        for (set, _) in sets() {
            assert!(!src.join(set).join("index.md").exists(), "{set}/index.md");
        }
    }

    #[test]
    fn default_mode_runs_without_an_introduction() {
        let dir = fake_guide();
        fs::remove_file(dir.path().join("src").join("introduction.md")).expect("remove intro");
        assemble(dir.path()).expect("assemble without introduction");
    }

    #[test]
    fn assembly_is_deterministic() {
        let dir = fake_guide();
        assemble(dir.path()).expect("first run");
        let first = read_exports(dir.path());
        assemble(dir.path()).expect("second run");
        assert_eq!(first, read_exports(dir.path()));
        assert!(first[0].contains("# The Gateway"));
        assert!(first[0].contains("# Start"));
    }
}
