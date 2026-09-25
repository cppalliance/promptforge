use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_owned()).collect()
}

/// A temporary site holding an empty file at each `/`-separated path.
fn site_with(files: &[&str]) -> tempfile::TempDir {
    let site = tempfile::tempdir().expect("tempdir");
    for file in files {
        let path = site.path().join(file);
        fs::create_dir_all(path.parent().expect("a file has a parent")).expect("mkdir");
        fs::write(&path, "").expect("write file");
    }
    site
}

/// A page with one anchor per href.
fn page(hrefs: &[&str]) -> String {
    let mut html = String::new();
    for href in hrefs {
        html.push_str("<a href=\"");
        html.push_str(href);
        html.push_str("\">link</a>\n");
    }
    html
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn no_arguments_build_the_full_site() {
    assert_eq!(parse_args(&[]), Ok(Options { books_only: false }));
}

#[test]
fn the_books_only_flag_is_parsed() {
    assert_eq!(
        parse_args(&args(&["--books-only"])),
        Ok(Options { books_only: true })
    );
}

#[test]
fn unknown_or_extra_arguments_are_refused_with_the_usage() {
    for list in [
        &["--bogus"][..],
        &["books-only"],
        &["--books-only", "--books-only"],
    ] {
        assert_eq!(
            parse_args(&args(list)),
            Err(USAGE.to_owned()),
            "accepted {list:?}"
        );
    }
}

#[test]
fn staged_books_are_the_sorted_subfolders() {
    let staged = tempfile::tempdir().expect("tempdir");
    for book in ["workshop", "gateway", "language"] {
        fs::create_dir_all(staged.path().join(book).join("src")).expect("book dir");
    }
    fs::write(staged.path().join("stray.txt"), "not a book").expect("stray file");
    let books = staged_books(staged.path()).expect("discovery");
    assert_eq!(books, ["gateway", "language", "workshop"]);
}

#[test]
fn a_stage_with_no_books_is_an_error_naming_the_folder() {
    let staged = tempfile::tempdir().expect("tempdir");
    let error = staged_books(staged.path()).expect_err("no books");
    assert!(
        error.contains(&staged.path().display().to_string()),
        "{error}"
    );
}

#[test]
fn copy_dir_copies_nested_files_into_an_existing_folder() {
    let temp = tempfile::tempdir().expect("tempdir");
    let from = temp.path().join("landing");
    fs::create_dir_all(from.join("img").join("icons")).expect("landing tree");
    fs::write(from.join("index.html"), "<p>new</p>").expect("index");
    fs::write(from.join("img").join(".gitkeep"), "").expect("gitkeep");
    fs::write(from.join("img").join("icons").join("a.svg"), "<svg/>").expect("icon");
    let to = temp.path().join("site");
    fs::create_dir_all(to.join("gateway")).expect("built book");
    fs::write(to.join("gateway").join("index.html"), "book").expect("book page");
    fs::write(to.join("index.html"), "<p>old</p>").expect("stale index");

    copy_dir(&from, &to).expect("copy");

    assert_eq!(read(&to.join("index.html")), "<p>new</p>");
    assert!(to.join("img").join(".gitkeep").is_file());
    assert_eq!(read(&to.join("img").join("icons").join("a.svg")), "<svg/>");
    assert_eq!(read(&to.join("gateway").join("index.html")), "book");
}

#[test]
fn copy_dir_names_a_missing_source() {
    let temp = tempfile::tempdir().expect("tempdir");
    let from = temp.path().join("no-landing");
    let error = copy_dir(&from, &temp.path().join("site")).expect_err("missing source");
    assert!(error.contains(&from.display().to_string()), "{error}");
}

#[test]
fn a_page_whose_links_all_resolve_passes() {
    let site = site_with(&[
        "style.css",
        "gateway/index.html",
        "language/agent/index.html",
    ]);
    let html = format!(
        "<link rel=\"stylesheet\" href=\"style.css\">\n{}",
        page(&["gateway/index.html#setup", "language/agent/index.html"])
    );
    assert_eq!(
        broken_links(site.path(), &html, false),
        Vec::<String>::new()
    );
}

#[test]
fn a_missing_target_fails_and_is_named() {
    let site = site_with(&["gateway/index.html"]);
    let html = page(&["gateway/index.html", "workshop/index.html"]);
    let broken = broken_links(site.path(), &html, false);
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert!(broken[0].contains("workshop/index.html"), "{broken:?}");
}

#[test]
fn every_broken_link_is_listed() {
    let site = site_with(&[]);
    let html = page(&["a.html", "b/c.html"]);
    let broken = broken_links(site.path(), &html, false);
    assert_eq!(broken.len(), 2, "{broken:?}");
    assert!(broken[0].contains("a.html") && broken[1].contains("b/c.html"));
}

#[test]
fn a_folder_link_fails_even_when_the_folder_exists() {
    let site = site_with(&["gateway/index.html"]);
    let html = page(&["gateway/", "gateway"]);
    let broken = broken_links(site.path(), &html, false);
    assert_eq!(broken.len(), 2, "{broken:?}");
}

#[test]
fn a_link_that_leaves_the_site_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let site = temp.path().join("site");
    fs::create_dir_all(&site).expect("site dir");
    fs::write(site.join("index.html"), "").expect("landing");
    fs::write(temp.path().join("outside.html"), "").expect("outside file");
    let html = page(&["../outside.html", "/index.html"]);
    let broken = broken_links(&site, &html, false);
    assert_eq!(broken.len(), 2, "{broken:?}");
}

#[test]
fn external_mail_and_fragment_links_are_ignored() {
    let site = site_with(&[]);
    let html = page(&[
        "http://example.com/",
        "https://example.com/docs/",
        "mailto:docs@example.com",
        "#top",
    ]);
    assert_eq!(
        broken_links(site.path(), &html, false),
        Vec::<String>::new()
    );
}

#[test]
fn books_only_skips_only_the_rustdoc_folders() {
    let site = site_with(&[]);
    let html = page(&[
        "promptforge/index.html",
        "harness/index.html",
        "gateway/index.html",
        "promptforge-extra/index.html",
    ]);
    assert_eq!(broken_links(site.path(), &html, false).len(), 4);
    let broken = broken_links(site.path(), &html, true);
    assert_eq!(broken.len(), 2, "{broken:?}");
    assert!(broken[0].contains("gateway/index.html"), "{broken:?}");
    assert!(
        broken[1].contains("promptforge-extra/index.html"),
        "{broken:?}"
    );
}

#[test]
fn a_child_that_cannot_start_is_named() {
    let error = run_child(&mut Command::new("promptforge-no-such-program"))
        .expect_err("the program does not exist");
    assert!(error.contains("promptforge-no-such-program"), "{error}");
}

#[test]
fn a_child_that_exits_nonzero_is_named() {
    let mut command = Command::new(env!("CARGO"));
    command.arg("--no-such-flag").stderr(Stdio::null());
    let error = run_child(&mut command).expect_err("cargo rejects the flag");
    assert!(error.contains("--no-such-flag"), "{error}");
}
