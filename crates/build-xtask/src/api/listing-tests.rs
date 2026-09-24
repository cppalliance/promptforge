//! The listing's comparison with `public-api.txt`, and the snapshot
//! workflow end to end: a difference fails `--check`, `--bless` writes
//! the listing and refuses while a violation remains, and a violation
//! alone fails `--check` while a plain run only prints it.

use super::super::fixture::{leak, workspace, write};
use super::super::{Mode, execute, report};
use super::*;

fn owned(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

#[test]
fn a_matching_listing_has_no_difference_whatever_its_line_endings() {
    let lines = owned(&["pub fn promptforge::a()", "pub struct promptforge::B"]);
    assert!(difference(Some(&text(&lines)), &lines).is_empty());
    assert!(
        difference(
            Some("pub fn promptforge::a()\r\npub struct promptforge::B\r\n"),
            &lines
        )
        .is_empty()
    );
}

#[test]
fn a_changed_listing_reports_removed_then_added_lines() {
    let lines = owned(&["pub fn promptforge::a()", "pub struct promptforge::C"]);
    assert_eq!(
        difference(
            Some("pub fn promptforge::a()\npub struct promptforge::B\n"),
            &lines
        ),
        owned(&[
            "public-api.txt differs from the surface listing:",
            "- pub struct promptforge::B",
            "+ pub struct promptforge::C",
        ])
    );
}

#[test]
fn a_missing_listing_reports_every_line_as_added() {
    let lines = owned(&["pub struct promptforge::B"]);
    assert_eq!(
        difference(None, &lines),
        owned(&[
            "public-api.txt is missing; the surface listing is:",
            "+ pub struct promptforge::B"
        ])
    );
}

#[test]
fn an_unsorted_listing_with_the_same_lines_still_differs() {
    let lines = owned(&["pub fn promptforge::a()", "pub struct promptforge::B"]);
    let report = difference(
        Some("pub struct promptforge::B\npub fn promptforge::a()\n"),
        &lines,
    );
    assert_eq!(report.len(), 2, "{report:?}");
    assert!(report[1].contains("not sorted"), "{report:?}");
}

const INNER: &str =
    "//! Inner.\n\n/// Exposed.\npub struct Visible {\n    /// A count.\n    pub count: u8,\n}\n";

const LEAKING_INNER: &str = "//! Inner.\n\n/// Kept internal.\npub struct Secret;\n\n\
    /// Leaks it.\npub fn make() -> Secret {\n    Secret\n}\n";

const LEAKING_FACADE: &str = "//! Facade.\npub use promptforge_inner::make;\n";

#[test]
#[ignore = "needs the pinned nightly"]
fn a_snapshot_difference_fails_check_until_blessed() {
    let root = workspace(INNER, "//! Facade.\npub use promptforge_inner::Visible;\n");
    let first = execute(root.path(), Mode::Check).expect("the fixture documents");
    assert!(first.failed, "{:?}", first.lines);
    assert!(
        first
            .lines
            .contains(&"public-api.txt is missing; the surface listing is:".to_owned())
    );
    assert!(
        first
            .lines
            .contains(&"+ pub struct promptforge::Visible".to_owned())
    );
    assert!(
        first
            .lines
            .contains(&"+ pub promptforge::Visible::count: u8".to_owned())
    );
    let blessed = execute(root.path(), Mode::Bless).expect("the fixture documents");
    assert!(!blessed.failed, "{:?}", blessed.lines);
    let clean = execute(root.path(), Mode::Check).expect("the fixture documents");
    assert!(!clean.failed, "{:?}", clean.lines);
    assert_eq!(
        clean.lines,
        ["api: 0 violations; the listing matches public-api.txt"]
    );
    write(
        root.path(),
        "crates/promptforge-internal/inner/src/lib.rs",
        &format!(
            "{INNER}\nimpl Visible {{\n    /// Doubles it.\n    pub fn double(&self) -> u16 {{\n        u16::from(self.count) * 2\n    }}\n}}\n"
        ),
    );
    let changed = execute(root.path(), Mode::Check).expect("the fixture documents");
    assert!(changed.failed, "{:?}", changed.lines);
    assert!(
        changed
            .lines
            .contains(&"+ pub fn promptforge::Visible::double(&self) -> u16".to_owned()),
        "{:?}",
        changed.lines
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn bless_refuses_while_a_violation_remains() {
    let root = workspace(LEAKING_INNER, LEAKING_FACADE);
    let outcome = execute(root.path(), Mode::Bless).expect("the fixture documents");
    assert!(outcome.failed, "{:?}", outcome.lines);
    assert_eq!(
        outcome.lines.last().map(String::as_str),
        Some("api: bless refused: 1 violations remain")
    );
    assert!(!path(root.path()).exists());
}

#[test]
#[ignore = "needs the pinned nightly"]
fn a_violation_alone_fails_check_and_a_plain_run_only_prints_it() {
    let root = workspace(LEAKING_INNER, LEAKING_FACADE);
    let listing = report(root.path()).expect("the fixture documents").listing;
    std::fs::write(path(root.path()), text(&listing)).expect("the listing writes");
    let leak = leak("promptforge_inner");
    let expected = [
        format!("promptforge::make: mentions `promptforge_inner::Secret` in its signature: {leak}"),
        "api: 1 violations; the listing matches public-api.txt".to_owned(),
    ];
    let check = execute(root.path(), Mode::Check).expect("the fixture documents");
    assert!(check.failed, "{:?}", check.lines);
    assert_eq!(check.lines, expected);
    let plain = execute(root.path(), Mode::Report).expect("the fixture documents");
    assert!(!plain.failed, "{:?}", plain.lines);
    assert_eq!(plain.lines, expected);
}
