//! Every shipped prompt under the workspace `prompts/` tree parses offline,
//! and so does every fixture this suite keeps as a positive example.

use std::fs;
use std::path::{Path, PathBuf};

use promptforge_api_runtime::parser::Prompt;
use promptforge_api_types::observe::NullObserver;

const SHIPPED_PARSE: &str = "fixture-shipped-prompts";

fn collect_markdown(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read repository prompt directory") {
        let path = entry.expect("read repository prompt entry").path();
        if path.is_dir() {
            collect_markdown(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

fn every_prompt_under_parses(directory: &Path) {
    let mut files = Vec::new();
    collect_markdown(directory, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "{} holds no prompts to check",
        directory.display()
    );

    for path in files {
        let source = fs::read_to_string(&path).expect("read prompt fixture");
        Prompt::parse(&source, SHIPPED_PARSE, &NullObserver::default())
            .unwrap_or_else(|error| panic!("{} must parse: {error}", path.display()));
    }
}

#[test]
fn every_shipped_prompt_parses_offline() {
    every_prompt_under_parses(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts"));
}

/// The `valid/` and `execution/` fixture directories are positive examples,
/// so a fixture no test happens to load still has to track the language
/// (`invalid/` is excluded by construction: those are meant to fail).
#[test]
fn every_positive_fixture_parses_offline() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/prompts");
    every_prompt_under_parses(&fixtures.join("valid"));
    every_prompt_under_parses(&fixtures.join("execution"));
}
