//! Tests for stage mode: one tree per book with its config, back-link
//! script, set folders, rendered overviews, and SUMMARY; a relative output
//! path and a broken SUMMARY link are rejected; output is deterministic and
//! the guide tree is only read.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::*;
use crate::BOOKS;
use crate::tests::fake_guide;

/// Every file under `root`, keyed by its `/`-separated relative path.
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let relative = path.strip_prefix(root).expect("under root");
                let key = relative.to_string_lossy().replace('\\', "/");
                files.insert(key, fs::read(&path).expect("read file"));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

/// Reads a staged text file.
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn stage_writes_one_tree_per_book() {
    let guide = fake_guide();
    let src = guide.path().join("src");
    fs::write(src.join("gateway").join("index.md"), "# Stale\n").expect("stale index");
    fs::create_dir_all(src.join("gateway").join("img")).expect("mkdir img");
    fs::write(src.join("gateway").join("img").join("flow.svg"), "<svg/>").expect("asset");
    let out = tempfile::tempdir().expect("tempdir");
    stage(guide.path(), out.path()).expect("stage");

    let mut staged: Vec<String> = fs::read_dir(out.path())
        .expect("read out")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    staged.sort_unstable();
    let mut books: Vec<&str> = BOOKS.iter().map(|(book, _)| *book).collect();
    books.sort_unstable();
    assert_eq!(staged, books);

    for (book, sets) in BOOKS {
        let root = out.path().join(book);
        let config = guide.path().join("books").join(book).join("book.toml");
        assert_eq!(read(&root.join("book.toml")), read(&config), "{book}");
        assert_eq!(read(&root.join("back-link.js")), "// All docs link.\n");
        let summary = read(&root.join("src").join("SUMMARY.md"));
        let mut entries: Vec<String> = vec!["SUMMARY.md".to_owned()];
        for (set, title) in *sets {
            entries.push((*set).to_owned());
            let index = read(&root.join("src").join(set).join("index.md"));
            assert!(
                index.starts_with(&format!("# {title}\n")),
                "{book}/{set}: {index}"
            );
            assert!(
                summary.contains(&format!("({set}/index.md)")),
                "{book}: {summary}"
            );
        }
        entries.sort_unstable();
        let mut listed: Vec<String> = fs::read_dir(root.join("src"))
            .expect("read book src")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        listed.sort_unstable();
        assert_eq!(listed, entries, "{book} stages only its own sets");
    }

    let workshop = out.path().join("workshop").join("src").join("workshop");
    assert!(workshop.join("01-the-window.md").is_file());
    assert!(workshop.join("02-the-editor.md").is_file());
    let gateway = out.path().join("gateway").join("src").join("gateway");
    assert_eq!(read(&gateway.join("img").join("flow.svg")), "<svg/>");
}

#[test]
fn each_book_opens_on_its_first_set_overview() {
    let guide = fake_guide();
    let out = tempfile::tempdir().expect("tempdir");
    stage(guide.path(), out.path()).expect("stage");
    for (book, sets) in BOOKS {
        let summary = read(&out.path().join(book).join("src").join("SUMMARY.md"));
        assert!(!summary.contains("introduction.md"), "{book}: {summary}");
        let first = summary
            .lines()
            .find_map(|line| line.split_once("](").map(|(_, rest)| rest))
            .expect("a link");
        assert_eq!(first, format!("{}/index.md)", sets[0].0), "{book}");
    }
}

#[test]
fn stage_rejects_a_relative_output_path() {
    let guide = fake_guide();
    let error = stage(guide.path(), Path::new("relative-stage-out")).expect_err("must reject");
    let message = error.to_string();
    assert!(message.contains("relative-stage-out"), "{message}");
    assert!(message.contains("absolute"), "{message}");
    assert!(!Path::new("relative-stage-out").exists());
}

#[test]
fn stage_rejects_a_summary_link_that_does_not_resolve() {
    let guide = fake_guide();
    let chapter = guide
        .path()
        .join("src")
        .join("gateway")
        .join("02-proxy (draft).md");
    fs::write(chapter, "# Proxy\n").expect("chapter");
    let out = tempfile::tempdir().expect("tempdir");
    let error = stage(guide.path(), out.path()).expect_err("must reject");
    let message = error.to_string();
    assert!(message.contains("gateway/02-proxy (draft"), "{message}");
    assert!(message.contains("gateway book"), "{message}");
}

#[test]
fn stage_rejects_a_stale_workshop_stt_claim_before_writing() {
    let guide = fake_guide();
    let chapter = guide
        .path()
        .join("src")
        .join("language")
        .join("09-stale.md");
    fs::write(
        chapter,
        "# Stale\n\nLegacy `[workshop.stt]` input is accepted.\n",
    )
    .expect("stale");
    let out = tempfile::tempdir().expect("tempdir");
    let error = stage(guide.path(), out.path()).expect_err("must reject");
    assert!(error.to_string().contains("09-stale.md"), "{error}");
    assert!(snapshot(out.path()).is_empty());
}

#[test]
fn staging_is_deterministic() {
    let guide = fake_guide();
    let first = tempfile::tempdir().expect("tempdir");
    let second = tempfile::tempdir().expect("tempdir");
    stage(guide.path(), first.path()).expect("first");
    stage(guide.path(), second.path()).expect("second");
    let staged = snapshot(first.path());
    assert!(
        staged.contains_key("language/src/SUMMARY.md"),
        "{:?}",
        staged.keys()
    );
    assert_eq!(staged, snapshot(second.path()));
    stage(guide.path(), first.path()).expect("restage");
    assert_eq!(staged, snapshot(first.path()));
}

#[test]
fn stage_leaves_the_guide_tree_untouched() {
    let guide = fake_guide();
    let before = snapshot(guide.path());
    let out = tempfile::tempdir().expect("tempdir");
    stage(guide.path(), out.path()).expect("stage");
    assert_eq!(before, snapshot(guide.path()));
}
