//! Shipped-prompt policy: every prompt under the workspace `prompts/` tree
//! parses offline and declares semantic capabilities rather than concrete tools.

use std::fs;
use std::path::{Path, PathBuf};

use promptforge_core::observe::NullObserver;
use promptforge_core::parser::Prompt;

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

#[test]
fn every_shipped_prompt_parses_offline() {
    let prompts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts");
    let mut files = Vec::new();
    collect_markdown(&prompts, &mut files);
    files.sort();
    assert_eq!(files.len(), 5, "every shipped markdown prompt is covered");

    for path in files {
        let source = fs::read_to_string(&path).expect("read shipped prompt");
        assert!(
            !source.contains("web_search") && !source.contains("web_fetch"),
            "{} must declare semantic capabilities, not concrete tools",
            path.display()
        );
        Prompt::parse(&source, SHIPPED_PARSE, &NullObserver::default())
            .unwrap_or_else(|error| panic!("{} must parse: {error}", path.display()));
    }
}
