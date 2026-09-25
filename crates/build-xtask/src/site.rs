//! `cargo xtask site [--books-only]`: builds the documentation site into
//! `target/site/`, in this order:
//!
//! 1. Clears `target/site/` and `target/site-books/`.
//! 2. Stages one mdBook tree per book into `target/site-books/` by running
//!    `cargo run -p build-user-guide -- stage` as a subprocess, so this
//!    crate still depends on no workspace crates.
//! 3. Runs `$MDBOOK build` (`MDBOOK` defaults to `mdbook`) on every staged
//!    folder, into `target/site/<book>/`. The folders are read from the
//!    stage output; no book is named here.
//! 4. Copies `guide/landing/` into `target/site/`.
//! 5. Checks the landing links: every relative `href` in
//!    `target/site/index.html` must name a file under `target/site/`.
//!
//! Every path passed to a child process is absolute, built from the
//! workspace root. This command does not build the rustdoc folders
//! (`promptforge/` and `harness/`), so both modes produce the books and the
//! link check skips links into those folders.

use std::borrow::Cow;
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask site [--books-only]";

/// The site folders that hold rustdoc output rather than a book.
const RUSTDOC_DIRS: [&str; 2] = ["promptforge", "harness"];

/// Link targets the landing check never resolves.
const IGNORED_PREFIXES: [&str; 4] = ["http:", "https:", "mailto:", "#"];

/// What `cargo xtask site` was asked to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// Skip the rustdoc stage, for PR runs and chapter previews.
    pub(crate) books_only: bool,
}

/// Runs `cargo xtask site` with the arguments after `site`.
pub(crate) fn run(root: &Path, args: &[String]) -> ExitCode {
    match parse_args(args).and_then(|options| build(root, options)) {
        Ok(site) => {
            println!("site: built {}", site.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    match args {
        [] => Ok(Options { books_only: false }),
        [flag] if flag == "--books-only" => Ok(Options { books_only: true }),
        _ => Err(USAGE.to_owned()),
    }
}

/// Builds the site for the workspace at `root`, returning its folder.
fn build(root: &Path, options: Options) -> Result<PathBuf, String> {
    let target = root.join("target");
    let site = target.join("site");
    let staged = target.join("site-books");
    clear(&site)?;
    clear(&staged)?;

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    run_child(
        Command::new(cargo)
            .current_dir(root)
            .args(["run", "-p", "build-user-guide", "--", "stage"])
            .arg(&staged),
    )?;

    let mdbook = std::env::var_os("MDBOOK").unwrap_or_else(|| OsString::from("mdbook"));
    for book in staged_books(&staged)? {
        run_child(
            Command::new(&mdbook)
                .arg("build")
                .arg(staged.join(&book))
                .arg("-d")
                .arg(site.join(&book)),
        )?;
    }

    if !options.books_only {
        println!("site: the rustdoc stage is not built yet, so this site holds the books only");
    }

    copy_dir(&root.join("guide").join("landing"), &site)?;
    check_landing(&site, true)?;
    Ok(site)
}

/// Removes `dir` and everything in it; a folder that does not exist is
/// already clear.
fn clear(dir: &Path) -> Result<(), String> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("site: cannot clear {}: {error}", dir.display())),
    }
}

/// The names of the book folders in `staged`, sorted. Files beside them
/// are not books and are skipped.
fn staged_books(staged: &Path) -> Result<Vec<OsString>, String> {
    let read_error =
        |error: std::io::Error| format!("site: cannot read {}: {error}", staged.display());
    let mut books = Vec::new();
    for entry in fs::read_dir(staged).map_err(read_error)? {
        let entry = entry.map_err(read_error)?;
        if entry.file_type().map_err(read_error)?.is_dir() {
            books.push(entry.file_name());
        }
    }
    if books.is_empty() {
        return Err(format!(
            "site: required at least one staged book folder in {}, found none",
            staged.display()
        ));
    }
    books.sort();
    Ok(books)
}

/// Copies the folder `from` into `to`, recursing into subfolders. Files
/// already in `to` are overwritten; nothing in `to` is removed.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    let read_error =
        |error: std::io::Error| format!("site: cannot read {}: {error}", from.display());
    let entries = fs::read_dir(from).map_err(read_error)?;
    fs::create_dir_all(to)
        .map_err(|error| format!("site: cannot create {}: {error}", to.display()))?;
    for entry in entries {
        let entry = entry.map_err(read_error)?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if entry.file_type().map_err(read_error)?.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            fs::copy(&source, &target).map_err(|error| {
                format!(
                    "site: cannot copy {} to {}: {error}",
                    source.display(),
                    target.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Fails with every broken link on the landing page at `site/index.html`.
fn check_landing(site: &Path, skip_rustdoc: bool) -> Result<(), String> {
    let page = site.join("index.html");
    let html = fs::read_to_string(&page)
        .map_err(|error| format!("site: cannot read {}: {error}", page.display()))?;
    let broken = broken_links(site, &html, skip_rustdoc);
    if broken.is_empty() {
        return Ok(());
    }
    Err(format!(
        "site: {} has {} broken links:\n{}",
        page.display(),
        broken.len(),
        broken.join("\n")
    ))
}

/// Every `href="..."` value in `html` that does not name a file under
/// `site`, one message each, in page order.
///
/// `http:`, `https:`, `mailto:`, and `#` targets are ignored, and so are
/// links into the rustdoc folders when `skip_rustdoc` is set. A `#`
/// fragment or `?` query after the file name is not part of the file.
#[must_use]
fn broken_links(site: &Path, html: &str, skip_rustdoc: bool) -> Vec<String> {
    hrefs(html)
        .filter(|href| {
            !IGNORED_PREFIXES
                .iter()
                .any(|prefix| href.starts_with(prefix))
        })
        .filter(|href| !(skip_rustdoc && RUSTDOC_DIRS.contains(&first_segment(href))))
        .filter_map(|href| unresolved(site, href).map(|reason| format!("{href}: {reason}")))
        .collect()
}

/// The `href="..."` values in `html`, by plain string scan.
fn hrefs(html: &str) -> impl Iterator<Item = &str> {
    html.split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(href, _)| href))
}

fn first_segment(href: &str) -> &str {
    href.split_once('/').map_or(href, |(first, _)| first)
}

/// Why `href` does not name a file under `site`, or `None` when it does.
fn unresolved(site: &Path, href: &str) -> Option<&'static str> {
    let path = href.split(['#', '?']).next().unwrap_or(href);
    if path.starts_with('/') || path.contains([':', '\\']) {
        return Some("is not a relative file path");
    }
    if path.split('/').any(|segment| segment == "..") {
        return Some("leaves the site folder");
    }
    let target = path
        .split('/')
        .fold(site.to_path_buf(), |dir, segment| dir.join(segment));
    if target.is_file() {
        None
    } else if path.is_empty() || path.ends_with('/') || target.is_dir() {
        Some("names a folder, not a file")
    } else {
        Some("no such file")
    }
}

/// Runs `command` to completion. When it cannot start or exits
/// unsuccessfully, the error names the command line.
fn run_child(command: &mut Command) -> Result<(), String> {
    let shown = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy())
        .collect::<Vec<Cow<'_, str>>>()
        .join(" ");
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("site: `{shown}` failed ({status})")),
        Err(error) => Err(format!("site: cannot start `{shown}`: {error}")),
    }
}

#[cfg(test)]
#[path = "site-tests.rs"]
mod tests;
