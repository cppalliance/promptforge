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
//! 4. Unless `--books-only` is set, builds the rustdoc sites
//!    `target/site/promptforge/` and `target/site/harness/` through the
//!    separate `target/site-doc` folder, so the developer's `target/doc`
//!    is untouched. Each page carries the `guide/chrome/banner.html` bar,
//!    and each site folder gets an `index.html` redirect to its crate.
//! 5. Copies `guide/landing/` into `target/site/`.
//! 6. Checks the links on every built page: each relative `href` in an
//!    `.html` file under `target/site/`, resolved from the page's own
//!    folder, must name a file under `target/site/`. The rustdoc folders,
//!    which rustdoc checks itself, and the books' `404.html` pages are not
//!    read. With `--books-only`, links into the rustdoc folders are
//!    skipped.
//!
//! Every path passed to a child process is absolute, built from the
//! workspace root.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask site [--books-only]";

/// The rustdoc sites: the folder under `target/site/` and the crate it
/// documents, with default features, the facade as hosts read it.
const RUSTDOC_SITES: [(&str, &str); 2] =
    [("promptforge", "promptforge"), ("harness", "harness-api")];

/// Link targets the link check never resolves.
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
        Command::new(&cargo)
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
        build_rustdoc(root, &cargo, &site)?;
    }

    copy_dir(&root.join("guide").join("landing"), &site)?;
    check_links(&site, options.books_only)?;
    Ok(site)
}

/// Builds every rustdoc site into `site/<dir>/`.
fn build_rustdoc(root: &Path, cargo: &OsStr, site: &Path) -> Result<(), String> {
    let target_dir = root.join("target").join("site-doc");
    let flags = encoded_rustdoc_flags(&root.join("guide").join("chrome").join("banner.html"));
    for (dir, krate) in RUSTDOC_SITES {
        // Rustdoc merges every crate in a doc folder into one crate list and
        // search index, so each crate starts from an empty folder.
        run_child(
            Command::new(cargo)
                .current_dir(root)
                .args(["clean", "--doc", "--target-dir"])
                .arg(&target_dir),
        )?;
        run_child(
            Command::new(cargo)
                .current_dir(root)
                .args(["doc", "-p", krate, "--no-deps", "--target-dir"])
                .arg(&target_dir)
                .env("CARGO_ENCODED_RUSTDOCFLAGS", &flags),
        )?;
        let out = site.join(dir);
        copy_dir(&target_dir.join("doc"), &out)?;
        let redirect = out.join("index.html");
        fs::write(&redirect, redirect_page(krate))
            .map_err(|error| format!("site: cannot write {}: {error}", redirect.display()))?;
    }
    Ok(())
}

/// The `CARGO_ENCODED_RUSTDOCFLAGS` value that injects `banner` before
/// every rustdoc page's content. Cargo splits this variable only on the
/// `0x1f` separator, so a checkout path with spaces stays one argument;
/// `RUSTDOCFLAGS` splits on spaces and would break it.
fn encoded_rustdoc_flags(banner: &Path) -> OsString {
    let mut flags = OsString::from("--html-before-content\u{1f}");
    flags.push(banner);
    flags
}

/// The page of `krate` relative to its doc folder. Rustdoc names the
/// folder after the crate with `-` replaced by `_`.
fn crate_page(krate: &str) -> String {
    format!("{}/index.html", krate.replace('-', "_"))
}

/// The `index.html` that sends a rustdoc site folder to its crate's page.
fn redirect_page(krate: &str) -> String {
    let page = crate_page(krate);
    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta http-equiv=\"refresh\" content=\"0; url={page}\">\n\
         <title>{krate}</title>\n\
         </head>\n\
         <body>\n\
         <p><a href=\"{page}\">{krate}</a></p>\n\
         </body>\n\
         </html>\n"
    )
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

/// Fails with every broken link on every page [`checked_pages`] names,
/// each message led by its page.
fn check_links(site: &Path, skip_rustdoc: bool) -> Result<(), String> {
    let mut broken = Vec::new();
    for page in checked_pages(site)? {
        let path = site.join(&page);
        let html = fs::read_to_string(&path)
            .map_err(|error| format!("site: cannot read {}: {error}", path.display()))?;
        broken.extend(
            broken_links(site, &page, &html, skip_rustdoc)
                .into_iter()
                .map(|link| format!("{page}: {link}")),
        );
    }
    if broken.is_empty() {
        return Ok(());
    }
    Err(format!(
        "site: {} has {} broken links:\n{}",
        site.display(),
        broken.len(),
        broken.join("\n")
    ))
}

/// The `/`-separated paths, relative to `site`, of every page the link
/// check reads, sorted: each `.html` file except those under the rustdoc
/// folders, which rustdoc checks itself, and every `404.html`, whose
/// `<base href>` resolves its links against the published URL rather than
/// its own folder.
fn checked_pages(site: &Path) -> Result<Vec<String>, String> {
    fn walk(dir: &Path, prefix: &str, pages: &mut Vec<String>) -> Result<(), String> {
        let read_error =
            |error: std::io::Error| format!("site: cannot read {}: {error}", dir.display());
        for entry in fs::read_dir(dir).map_err(read_error)? {
            let entry = entry.map_err(read_error)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if entry.file_type().map_err(read_error)?.is_dir() {
                if !(prefix.is_empty() && is_rustdoc_folder(&name)) {
                    walk(&entry.path(), &relative, pages)?;
                }
            } else if Path::new(&name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
                && !name.eq_ignore_ascii_case("404.html")
            {
                pages.push(relative);
            }
        }
        Ok(())
    }
    let mut pages = Vec::new();
    walk(site, "", &mut pages)?;
    pages.sort();
    Ok(pages)
}

fn is_rustdoc_folder(name: &str) -> bool {
    RUSTDOC_SITES.iter().any(|(dir, _)| *dir == name)
}

/// Every `href="..."` value in `html`, the page at the `/`-separated path
/// `page` under `site`, that does not name a file under `site`, one
/// message each, in page order.
///
/// `http:`, `https:`, `mailto:`, and `#` targets are ignored, and so are
/// links into the rustdoc folders when `skip_rustdoc` is set.
#[must_use]
fn broken_links(site: &Path, page: &str, html: &str, skip_rustdoc: bool) -> Vec<String> {
    hrefs(html)
        .filter(|href| {
            !IGNORED_PREFIXES
                .iter()
                .any(|prefix| href.starts_with(prefix))
        })
        .filter_map(|href| {
            let reason = match resolve(page, href) {
                Err(reason) => reason,
                Ok(segments) => {
                    if skip_rustdoc && segments.first().is_some_and(|dir| is_rustdoc_folder(dir)) {
                        return None;
                    }
                    unresolved(site, &segments)?
                }
            };
            Some(format!("{href}: {reason}"))
        })
        .collect()
}

/// The `href="..."` values in `html`, by plain string scan.
fn hrefs(html: &str) -> impl Iterator<Item = &str> {
    html.split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(href, _)| href))
}

/// Resolves `href` from the folder of `page` into the target's path
/// segments under the site root, or says why it names nothing there. A
/// `#` fragment or `?` query after the file name is not part of the file.
fn resolve<'a>(page: &'a str, href: &'a str) -> Result<Vec<&'a str>, &'static str> {
    let path = href.split(['#', '?']).next().unwrap_or(href);
    if path.starts_with('/') || path.contains([':', '\\']) {
        return Err("is not a relative file path");
    }
    let mut segments: Vec<&str> = page.split('/').collect();
    segments.pop();
    for segment in path.split('/') {
        match segment {
            "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err("leaves the site folder");
                }
            }
            _ => segments.push(segment),
        }
    }
    Ok(segments)
}

/// Why the resolved `segments` do not name a file under `site`, or `None`
/// when they do.
fn unresolved(site: &Path, segments: &[&str]) -> Option<&'static str> {
    let target = segments
        .iter()
        .fold(site.to_path_buf(), |dir, segment| dir.join(segment));
    if target.is_file() {
        None
    } else if target.is_dir() || segments.last().is_none_or(|last| last.is_empty()) {
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
