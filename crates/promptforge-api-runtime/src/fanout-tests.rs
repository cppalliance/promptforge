//! Tests for resolving a fanout worker heading among sibling sections.

use super::*;

#[test]
fn resolve_sibling_finds_exact_match() {
    let sections = vec![sibling("Worker", 3), sibling("Topics", 3)];
    let found = resolve_sibling("### Worker", &sections).expect("must resolve");
    assert_eq!(found.name(), "Worker");
}

#[test]
fn resolve_sibling_missing_heading_lists_available() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("### Missing", &sections).expect_err("missing heading must error");
    assert!(err.to_string().contains("### Worker"), "error was: {err}");
}

#[test]
fn resolve_sibling_bare_name_errors() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("Worker", &sections).expect_err("bare name without ### must error");
    assert!(err.to_string().contains("### markers"), "error was: {err}");
}

fn sibling(name: &str, level: u8) -> Section {
    crate::test_support::synthetic_section(
        name,
        level,
        vec![promptforge_parser::test_support::prose_block(String::new())],
        Vec::new(),
    )
}

#[test]
fn resolve_sibling_requires_whitespace_after_markers() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("###Worker", &sections)
        .expect_err("no whitespace after markers must error");
    assert!(err.to_string().contains("whitespace"), "error was: {err}");
}

#[test]
fn resolve_sibling_marker_only_heading_errors_as_nameless() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("### ", &sections).expect_err("a marker-only heading must error");
    assert!(err.to_string().contains("has no name"), "error was: {err}");
}

#[test]
fn resolve_sibling_requires_exact_level() {
    let sections = vec![sibling("Worker", 3)];
    // Same name, wrong marker level, must not resolve.
    let err = resolve_sibling("## Worker", &sections)
        .expect_err("a level mismatch must not resolve by name alone");
    assert!(err.to_string().contains("not found"), "error was: {err}");
    // The exact address resolves.
    let ok = resolve_sibling("### Worker", &sections).expect("exact address resolves");
    assert_eq!(ok.name(), "Worker");
}

#[test]
fn resolve_sibling_rejects_more_than_one_match() {
    let sections = vec![sibling("Worker", 3), sibling("Worker", 3)];
    let err = resolve_sibling("### Worker", &sections)
        .expect_err("two identical siblings must be rejected as ambiguous");
    assert!(err.to_string().contains("ambiguous"), "error was: {err}");
}
