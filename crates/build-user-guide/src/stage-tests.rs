//! Tests for stage mode: chapters come from the docs root it is given,
//! which must be set and name a directory; one flat tree per book with its
//! config, back-link script, chapters, rendered overview, and SUMMARY, and
//! each book's single-file export beside the trees; a relative output path,
//! a broken SUMMARY link, and a `[workshop.stt]` claim are rejected;
//! output is deterministic and the guide and docs trees are only read.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::*;
use crate::BOOKS;
use crate::tests::{FakeGuide, fake_guide};

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

/// The entry names directly inside `dir` whose kind is a folder when
/// `folders` is set and a file otherwise, sorted.
fn names(dir: &Path, folders: bool) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.expect("entry"))
        .filter(|entry| entry.path().is_dir() == folders)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_unstable();
    names
}

/// Every entry name directly inside `dir`, sorted.
fn all_names(dir: &Path) -> Vec<String> {
    let mut all = names(dir, true);
    all.extend(names(dir, false));
    all.sort_unstable();
    all
}

/// The link targets of a SUMMARY, in order.
fn summary_targets(summary: &str) -> Vec<&str> {
    summary
        .lines()
        .filter_map(|line| line.split_once("](").map(|(_, rest)| rest))
        .filter_map(|rest| rest.strip_suffix(')'))
        .collect()
}

/// Stages `fake`, reading its chapters from its docs root, into `out`.
fn stage_fake(fake: &FakeGuide, out: &Path) -> Result<(), crate::AssembleError> {
    stage(&fake.guide, Some(&fake.docs), out)
}

#[test]
fn stage_writes_one_tree_per_book() {
    let fake = fake_guide();
    let src = fake.docs.join("src");
    fs::write(src.join("gateway").join("index.md"), "# Stale\n").expect("stale index");
    fs::create_dir_all(src.join("gateway").join("img")).expect("mkdir img");
    fs::write(src.join("gateway").join("img").join("flow.svg"), "<svg/>").expect("asset");
    let out = tempfile::tempdir().expect("tempdir");
    stage_fake(&fake, out.path()).expect("stage");

    let mut books: Vec<&str> = BOOKS.iter().map(|(book, _)| *book).collect();
    books.sort_unstable();
    assert_eq!(names(out.path(), true), books);

    for (book, title) in BOOKS {
        let root = out.path().join(book);
        let config = fake.guide.join("books").join(book).join("book.toml");
        assert_eq!(read(&root.join("book.toml")), read(&config), "{book}");
        assert_eq!(read(&root.join("back-link.js")), "// All docs link.\n");
        let index = read(&root.join("src").join("index.md"));
        assert!(
            index.starts_with(&format!("# {title}\n")),
            "{book}: {index}"
        );
        let mut entries = all_names(&src.join(book));
        entries.extend(["SUMMARY.md".to_owned(), "index.md".to_owned()]);
        entries.sort_unstable();
        entries.dedup();
        assert_eq!(
            all_names(&root.join("src")),
            entries,
            "{book} stages its own chapters flat, with no set folder"
        );
    }

    let workshop = out.path().join("workshop").join("src");
    assert!(workshop.join("01-the-window.md").is_file());
    assert!(workshop.join("02-the-editor.md").is_file());
    let gateway = out.path().join("gateway").join("src");
    assert_eq!(read(&gateway.join("img").join("flow.svg")), "<svg/>");
}

#[test]
fn stage_writes_each_books_export_beside_the_trees() {
    let fake = fake_guide();
    let out = tempfile::tempdir().expect("tempdir");
    stage_fake(&fake, out.path()).expect("stage");

    let mut exports: Vec<String> = BOOKS
        .iter()
        .map(|(book, _)| format!("promptforge-{book}-guide.md"))
        .collect();
    exports.sort_unstable();
    assert_eq!(names(out.path(), false), exports);
    for (book, title) in BOOKS {
        let export = read(&out.path().join(format!("promptforge-{book}-guide.md")));
        assert!(
            export.starts_with(&format!("# {title}\n")),
            "{book}: {export}"
        );
    }
    let workshop = read(&out.path().join("promptforge-workshop-guide.md"));
    assert!(
        workshop.contains("# The Window") && workshop.contains("# The Editor"),
        "{workshop}"
    );
}

#[test]
fn staging_reads_chapters_from_the_docs_root_it_is_given() {
    let fake = fake_guide();
    let old_copy = fake.guide.join("src").join("gateway");
    fs::create_dir_all(&old_copy).expect("mkdir old copy");
    fs::write(old_copy.join("01-start.md"), "# Start\n\nOld copy.\n").expect("old chapter");
    let chapter = "# Start\n\nFrom the docs root.\n";
    fs::write(
        fake.docs.join("src").join("gateway").join("01-start.md"),
        chapter,
    )
    .expect("chapter");
    let out = tempfile::tempdir().expect("tempdir");
    stage_fake(&fake, out.path()).expect("stage");

    let staged = out.path().join("gateway").join("src").join("01-start.md");
    assert_eq!(read(&staged), chapter);
    let export = read(&out.path().join("promptforge-gateway-guide.md"));
    assert!(
        export.contains("From the docs root.") && !export.contains("Old copy."),
        "{export}"
    );
}

#[test]
fn staging_without_a_docs_root_fails_naming_promptforge_docs() {
    let fake = fake_guide();
    let out = tempfile::tempdir().expect("tempdir");
    let error = stage(&fake.guide, None, out.path()).expect_err("must fail");
    assert!(error.to_string().contains("PROMPTFORGE_DOCS"), "{error}");
    assert!(snapshot(out.path()).is_empty());
}

#[test]
fn staging_with_a_missing_docs_root_fails_naming_promptforge_docs_and_the_path() {
    let fake = fake_guide();
    let missing = fake.docs.join("no-such-checkout");
    let out = tempfile::tempdir().expect("tempdir");
    let error = stage(&fake.guide, Some(&missing), out.path()).expect_err("must fail");
    let message = error.to_string();
    assert!(message.contains("PROMPTFORGE_DOCS"), "{message}");
    assert!(
        message.contains(&missing.display().to_string()),
        "{message}"
    );
    assert!(snapshot(out.path()).is_empty());
}

#[test]
fn each_summary_opens_on_the_overview_and_links_only_sibling_files() {
    let fake = fake_guide();
    let out = tempfile::tempdir().expect("tempdir");
    stage_fake(&fake, out.path()).expect("stage");
    for (book, _) in BOOKS {
        let book_src = out.path().join(book).join("src");
        let summary = read(&book_src.join("SUMMARY.md"));
        assert!(!summary.contains("introduction.md"), "{book}: {summary}");
        let targets = summary_targets(&summary);
        assert_eq!(targets.first(), Some(&"index.md"), "{book}: {summary}");
        for target in targets {
            assert!(!target.contains('/'), "{book}: {target} is not a sibling");
            assert!(book_src.join(target).is_file(), "{book}: {target}");
        }
    }
}

#[test]
fn stage_runs_without_an_introduction() {
    let fake = fake_guide();
    fs::remove_file(fake.docs.join("src").join("introduction.md")).expect("remove intro");
    let out = tempfile::tempdir().expect("tempdir");
    stage_fake(&fake, out.path()).expect("stage without introduction");
}

#[test]
fn stage_rejects_a_relative_output_path() {
    let fake = fake_guide();
    let error = stage_fake(&fake, Path::new("relative-stage-out")).expect_err("must reject");
    let message = error.to_string();
    assert!(message.contains("relative-stage-out"), "{message}");
    assert!(message.contains("absolute"), "{message}");
    assert!(!Path::new("relative-stage-out").exists());
}

#[test]
fn stage_rejects_a_summary_link_that_does_not_resolve() {
    let fake = fake_guide();
    let chapter = fake
        .docs
        .join("src")
        .join("gateway")
        .join("02-proxy (draft).md");
    fs::write(chapter, "# Proxy\n").expect("chapter");
    let out = tempfile::tempdir().expect("tempdir");
    let error = stage_fake(&fake, out.path()).expect_err("must reject");
    let message = error.to_string();
    assert!(message.contains("02-proxy (draft"), "{message}");
    assert!(message.contains("gateway book"), "{message}");
}

#[test]
fn stage_rejects_a_stale_workshop_stt_claim_in_any_set_before_writing() {
    for (set, _) in BOOKS {
        let fake = fake_guide();
        fs::write(
            fake.docs.join("src").join(set).join("09-stale.md"),
            "# Stale\n\nLegacy `[workshop.stt]` input is accepted.\n",
        )
        .expect("stale chapter");
        let out = tempfile::tempdir().expect("tempdir");
        let error = stage_fake(&fake, out.path()).expect_err(set);
        let message = error.to_string();
        assert!(message.contains("09-stale.md"), "{set}: {message}");
        assert!(
            message.contains("removed [workshop.stt] section is not described as rejected"),
            "{set}: {message}"
        );
        assert!(snapshot(out.path()).is_empty(), "{set}");
    }
}

#[test]
fn staging_is_deterministic() {
    let fake = fake_guide();
    let first = tempfile::tempdir().expect("tempdir");
    let second = tempfile::tempdir().expect("tempdir");
    stage_fake(&fake, first.path()).expect("first");
    stage_fake(&fake, second.path()).expect("second");
    let staged = snapshot(first.path());
    for key in ["language/src/SUMMARY.md", "promptforge-language-guide.md"] {
        assert!(staged.contains_key(key), "{key}: {:?}", staged.keys());
    }
    assert_eq!(staged, snapshot(second.path()));
    stage_fake(&fake, first.path()).expect("restage");
    assert_eq!(staged, snapshot(first.path()));
}

#[test]
fn stage_leaves_the_guide_and_docs_trees_untouched() {
    let fake = fake_guide();
    let guide = snapshot(&fake.guide);
    let docs = snapshot(&fake.docs);
    let out = tempfile::tempdir().expect("tempdir");
    stage_fake(&fake, out.path()).expect("stage");
    assert_eq!(guide, snapshot(&fake.guide));
    assert_eq!(docs, snapshot(&fake.docs));
}
