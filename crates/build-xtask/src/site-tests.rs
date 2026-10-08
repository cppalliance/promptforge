use std::ffi::OsStr;
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

/// Writes `content` to the `/`-separated path `file` under `site`.
fn write_page(site: &Path, file: &str, content: &str) {
    let path = site.join(file);
    fs::create_dir_all(path.parent().expect("a file has a parent")).expect("mkdir");
    fs::write(&path, content).expect("write page");
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
    let site = site_with(&["style.css", "gateway/index.html", "language/index.html"]);
    let html = format!(
        "<link rel=\"stylesheet\" href=\"style.css\">\n{}",
        page(&["gateway/index.html#setup", "language/index.html"])
    );
    assert_eq!(
        broken_links(site.path(), "index.html", &html, false),
        Vec::<String>::new()
    );
}

#[test]
fn a_missing_target_fails_and_is_named() {
    let site = site_with(&["gateway/index.html"]);
    let html = page(&["gateway/index.html", "workshop/index.html"]);
    let broken = broken_links(site.path(), "index.html", &html, false);
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert!(broken[0].contains("workshop/index.html"), "{broken:?}");
}

#[test]
fn every_broken_link_is_listed() {
    let site = site_with(&[]);
    let html = page(&["a.html", "b/c.html"]);
    let broken = broken_links(site.path(), "index.html", &html, false);
    assert_eq!(broken.len(), 2, "{broken:?}");
    assert!(broken[0].contains("a.html") && broken[1].contains("b/c.html"));
}

#[test]
fn a_folder_link_fails_even_when_the_folder_exists() {
    let site = site_with(&["gateway/index.html"]);
    let html = page(&["gateway/", "gateway"]);
    let broken = broken_links(site.path(), "index.html", &html, false);
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
    let broken = broken_links(&site, "index.html", &html, false);
    assert_eq!(broken.len(), 2, "{broken:?}");
}

#[test]
fn a_nested_page_resolves_links_from_its_own_folder() {
    let site = site_with(&["gateway/01-install.html", "gateway/css/chrome.css"]);
    let html = page(&["01-install.html", "css/chrome.css", "02-missing.html"]);
    let broken = broken_links(site.path(), "gateway/index.html", &html, false);
    assert_eq!(broken, ["02-missing.html: no such file"]);
}

#[test]
fn a_parent_link_that_stays_inside_the_site_passes() {
    let site = site_with(&["index.html", "gateway/css/chrome.css"]);
    let html = page(&[
        "../css/chrome.css",
        "../../index.html",
        "./../../index.html",
    ]);
    assert_eq!(
        broken_links(site.path(), "gateway/sub/page.html", &html, false),
        Vec::<String>::new()
    );
}

#[test]
fn a_parent_link_that_escapes_the_site_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let site = temp.path().join("site");
    fs::create_dir_all(site.join("gateway")).expect("site dir");
    fs::write(temp.path().join("outside.html"), "").expect("outside file");
    let html = page(&["../../outside.html"]);
    let broken = broken_links(&site, "gateway/index.html", &html, false);
    assert_eq!(broken, ["../../outside.html: leaves the site folder"]);
}

#[test]
fn every_book_page_is_checked_and_named_in_the_error() {
    let site = tempfile::tempdir().expect("tempdir");
    write_page(site.path(), "index.html", &page(&["gateway/index.html"]));
    write_page(
        site.path(),
        "gateway/index.html",
        &page(&["01-missing.html"]),
    );
    write_page(
        site.path(),
        "gateway/nested/page.html",
        &page(&["../gone.html"]),
    );
    let error = check_links(site.path(), false).expect_err("two broken links");
    assert!(error.contains("has 2 broken links"), "{error}");
    assert!(
        error.contains("gateway/index.html: 01-missing.html: no such file"),
        "{error}"
    );
    assert!(
        error.contains("gateway/nested/page.html: ../gone.html: no such file"),
        "{error}"
    );
}

#[test]
fn rustdoc_pages_are_not_checked() {
    let site = tempfile::tempdir().expect("tempdir");
    write_page(site.path(), "index.html", "");
    write_page(site.path(), "promptforge/index.html", &page(&["gone.html"]));
    write_page(
        site.path(),
        "harness/harness/index.html",
        &page(&["gone.html"]),
    );
    write_page(
        site.path(),
        "gateway/promptforge/page.html",
        &page(&["gone.html"]),
    );
    let error = check_links(site.path(), false).expect_err("only the book page is read");
    assert!(error.contains("has 1 broken links"), "{error}");
    assert!(error.contains("gateway/promptforge/page.html"), "{error}");
}

#[test]
fn a_books_404_page_is_not_checked() {
    let site = tempfile::tempdir().expect("tempdir");
    write_page(site.path(), "index.html", "");
    write_page(
        site.path(),
        "gateway/404.html",
        &format!(
            "<base href=\"/promptforge/gateway/\">\n{}",
            page(&["print.html"])
        ),
    );
    assert_eq!(check_links(site.path(), false), Ok(()));
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
        broken_links(site.path(), "index.html", &html, false),
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
    assert_eq!(
        broken_links(site.path(), "index.html", &html, false).len(),
        4
    );
    let broken = broken_links(site.path(), "index.html", &html, true);
    assert_eq!(broken.len(), 2, "{broken:?}");
    assert!(broken[0].contains("gateway/index.html"), "{broken:?}");
    assert!(
        broken[1].contains("promptforge-extra/index.html"),
        "{broken:?}"
    );
}

#[test]
fn books_only_skips_a_book_pages_link_into_the_rustdoc_folders() {
    let site = site_with(&[]);
    let html = page(&["../promptforge/index.html", "../harness/index.html"]);
    assert_eq!(
        broken_links(site.path(), "gateway/index.html", &html, false).len(),
        2
    );
    assert_eq!(
        broken_links(site.path(), "gateway/index.html", &html, true),
        Vec::<String>::new()
    );
}

#[test]
fn the_encoded_flags_join_on_the_unit_separator_and_keep_a_spaced_path_whole() {
    let banner = Path::new("C:/Program Files/checkout dir/guide/chrome/banner.html");
    let flags = encoded_rustdoc_flags(banner);
    let flags = flags.to_str().expect("the flags are UTF-8");
    assert_eq!(
        flags.split('\u{1f}').collect::<Vec<_>>(),
        [
            "--html-before-content",
            "C:/Program Files/checkout dir/guide/chrome/banner.html"
        ]
    );
}

#[test]
fn the_crate_page_uses_the_underscored_crate_name() {
    assert_eq!(
        crate_page("gateway-api-types"),
        "gateway_api_types/index.html"
    );
    assert_eq!(crate_page("harness"), "harness/index.html");
    assert_eq!(crate_page("promptforge"), "promptforge/index.html");
}

#[test]
fn the_rustdoc_sites_document_each_public_harness_crate_and_the_web_plugin() {
    for krate in [
        "harness",
        "harness-gateway-client",
        "plugin-web",
        "plugin-mcp",
    ] {
        assert!(
            RUSTDOC_SITES
                .iter()
                .any(|(dir, documented, _)| *dir == krate && *documented == krate),
            "the {krate}/ folder documents the crate {krate}: {RUSTDOC_SITES:?}"
        );
    }
}

/// A stage output holding, per `(book, title, description)`, the book's
/// folder with a `book.toml` giving that title and description.
fn staged_with(books: &[(&str, &str, &str)]) -> tempfile::TempDir {
    let staged = tempfile::tempdir().expect("tempdir");
    for (book, title, description) in books {
        let config = format!("[book]\ntitle = \"{title}\"\ndescription = \"{description}\"\n");
        write_page(staged.path(), &format!("{book}/book.toml"), &config);
    }
    staged
}

const STAGED: [(&str, &str, &str); 3] = [
    ("gateway", "Gateway Guide", "Running the Gateway"),
    ("language", "Language Guide", "Writing prompts"),
    ("workshop", "Workshop Guide", "The desktop app"),
];

fn staged_landing() -> String {
    let staged = staged_with(&STAGED);
    let books = staged_books(staged.path()).expect("books");
    landing::page(staged.path(), &books).expect("landing page")
}

#[test]
fn the_landing_page_has_one_row_per_rustdoc_site_and_per_staged_book() {
    let html = staged_landing();
    assert_eq!(
        html.matches("<tr>").count(),
        1 + RUSTDOC_SITES.len() + STAGED.len(),
        "a header row, then one row per site and book: {html}"
    );
    for (dir, krate, covers) in RUSTDOC_SITES {
        let link = format!("<a href=\"{dir}/index.html\">{krate}</a>");
        assert_eq!(html.matches(&link).count(), 1, "{dir}: {html}");
        assert!(!covers.is_empty() && html.contains(covers), "{dir}: {html}");
    }
    for (book, title, description) in STAGED {
        let link = format!("<a href=\"{book}/index.html\">{title}</a>");
        assert_eq!(html.matches(&link).count(), 1, "{book}: {html}");
        assert!(html.contains(description), "{book}: {html}");
    }
}

#[test]
fn the_landing_page_has_no_images() {
    let html = staged_landing();
    assert!(!html.contains("<img"), "{html}");
}

#[test]
fn a_staged_book_without_a_description_is_named() {
    let staged = tempfile::tempdir().expect("tempdir");
    write_page(
        staged.path(),
        "gateway/book.toml",
        "[book]\ntitle = \"Gateway Guide\"\n",
    );
    let books = staged_books(staged.path()).expect("books");
    let error = landing::page(staged.path(), &books).expect_err("no description");
    let config = staged.path().join("gateway").join("book.toml");
    assert!(error.contains(&config.display().to_string()), "{error}");
    assert!(error.contains("description"), "{error}");
}

#[test]
fn every_guide_book_config_gives_its_landing_title_and_description() {
    let books_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../guide/books");
    let books = staged_books(&books_dir).expect("guide/books holds the book configs");
    landing::page(&books_dir, &books).expect("each book.toml has a title and a description");
}

#[test]
fn the_site_without_promptforge_docs_fails_naming_it_before_building() {
    for unset in [None, Some(OsStr::new(""))] {
        let root = tempfile::tempdir().expect("tempdir");
        let error = site(root.path(), unset, &[]).expect_err("PROMPTFORGE_DOCS is unset");
        assert!(error.contains("PROMPTFORGE_DOCS"), "{unset:?}: {error}");
        assert!(!root.path().join("target").exists(), "{unset:?}");
    }
}

#[test]
fn the_site_with_a_missing_docs_root_fails_naming_promptforge_docs_and_the_path() {
    let root = tempfile::tempdir().expect("tempdir");
    let missing = root.path().join("no-such-docs");
    let error = site(root.path(), Some(missing.as_os_str()), &[]).expect_err("missing docs root");
    assert!(error.contains("PROMPTFORGE_DOCS"), "{error}");
    assert!(error.contains(&missing.display().to_string()), "{error}");
    assert!(!root.path().join("target").exists());
}

#[test]
fn the_stage_outputs_files_land_in_the_site_root_without_the_book_trees() {
    let staged = site_with(&["gateway/src/01-start.md", "gateway/book.toml"]);
    write_page(
        staged.path(),
        "promptforge-gateway-guide.md",
        "# The Gateway\n",
    );
    let site = tempfile::tempdir().expect("tempdir");
    copy_files(staged.path(), site.path()).expect("copy");
    assert_eq!(
        read(&site.path().join("promptforge-gateway-guide.md")),
        "# The Gateway\n"
    );
    assert!(!site.path().join("gateway").exists());
}

#[test]
fn the_redirect_sends_only_to_the_crate_page() {
    let html = redirect_page("harness");
    assert!(
        html.contains("<meta http-equiv=\"refresh\" content=\"0; url=harness/index.html\">"),
        "{html}"
    );
    assert_eq!(hrefs(&html).collect::<Vec<_>>(), ["harness/index.html"]);
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
