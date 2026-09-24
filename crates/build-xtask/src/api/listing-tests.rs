//! The listing's comparison with `public-api.txt`, and the snapshot
//! workflow end to end: a difference fails `--check`, `--bless` writes
//! the listing and refuses while a violation remains, and a violation
//! alone fails `--check` while a plain run only prints it.
//!
//! The notation fixtures pin what a data line says beyond its path: the
//! `#[non_exhaustive]` prefix, the kind suffix, and the private-fields
//! marker, each read from the rustdoc JSON the pinned nightly writes.

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
            .contains(&"+ pub struct promptforge::Visible { .. }".to_owned())
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

const SHAPES: &str = "//! Inner.\n\n\
    /// A unit struct.\npub struct AllowAll;\n\n\
    /// A tuple struct whose field is private.\npub struct ExecId(u64);\n\n\
    /// A braced struct with every field public.\n\
    pub struct Open {\n    /// A count.\n    pub count: u8,\n}\n\n\
    /// A braced struct with a private field.\n\
    pub struct Guarded {\n    /// A count.\n    pub count: u8,\n    seed: u64,\n}\n\n\
    /// A union with a private field.\n\
    pub union Word {\n    /// The bits.\n    pub bits: u32,\n    seed: f32,\n}\n";

const SHAPES_FACADE: &str = "//! Facade.\n\
    pub use promptforge_inner::AllowAll;\npub use promptforge_inner::ExecId;\n\
    pub use promptforge_inner::Guarded;\npub use promptforge_inner::Open;\n\
    pub use promptforge_inner::Word;\n";

const VARIANTS: &str = "//! Inner.\n\n\
    /// An exhaustive enum.\n\
    pub enum Verdict {\n    /// A unit variant.\n    Allow,\n    /// A tuple variant.\n    Deny(u8),\n    /// A struct variant.\n    Ask {\n        /// A reason.\n        reason: u8,\n    },\n}\n\n\
    /// A non-exhaustive enum.\n#[non_exhaustive]\n\
    pub enum VfsError {\n    /// A non-exhaustive struct variant.\n    #[non_exhaustive]\n    Missing {\n        /// A code.\n        code: u8,\n    },\n    /// A unit variant.\n    Denied,\n}\n\n\
    /// An enum whose variant has a discriminant.\n\
    pub enum Code {\n    /// The first.\n    First = 1,\n}\n";

const VARIANTS_FACADE: &str = "//! Facade.\n\
    pub use promptforge_inner::Code;\npub use promptforge_inner::Verdict;\n\
    pub use promptforge_inner::VfsError;\n";

const UNIT: &str = "//! Inner.\n\n/// A unit struct.\npub struct AllowAll;\n";

const UNIT_FACADE: &str = "//! Facade.\npub use promptforge_inner::AllowAll;\n";

/// The surface listing of the fixture workspace built from `inner` and
/// `facade`, as a set to test membership against.
fn listing(inner: &str, facade: &str) -> BTreeSet<String> {
    let root = workspace(inner, facade);
    report(root.path())
        .expect("the fixture documents")
        .listing
        .into_iter()
        .collect()
}

fn assert_lines(listing: &BTreeSet<String>, shown: &[&str], absent: &[&str]) {
    for line in shown {
        assert!(
            listing.contains(*line),
            "{line} is missing from {listing:?}"
        );
    }
    for line in absent {
        assert!(!listing.contains(*line), "{line} is still in {listing:?}");
    }
}

#[test]
#[ignore = "needs the pinned nightly"]
fn a_struct_or_union_line_ends_with_its_kind_and_names_hidden_fields() {
    let listing = listing(SHAPES, SHAPES_FACADE);
    assert_lines(
        &listing,
        &[
            "pub struct promptforge::AllowAll;",
            "pub struct promptforge::ExecId(/* private fields */)",
            "pub struct promptforge::Open { .. }",
            "pub struct promptforge::Guarded { /* private fields */ }",
            "pub union promptforge::Word { /* private fields */ }",
            "pub promptforge::Guarded::count: u8",
        ],
        &[
            "pub struct promptforge::AllowAll",
            "pub struct promptforge::ExecId",
            "pub struct promptforge::Open",
            "pub struct promptforge::Guarded",
            "pub union promptforge::Word",
        ],
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn an_enum_or_variant_line_carries_non_exhaustive_and_its_variant_kind() {
    let listing = listing(VARIANTS, VARIANTS_FACADE);
    assert_lines(
        &listing,
        &[
            "pub enum promptforge::Verdict",
            "pub promptforge::Verdict::Allow",
            "pub promptforge::Verdict::Deny(..)",
            "pub promptforge::Verdict::Ask { .. }",
            "#[non_exhaustive] pub enum promptforge::VfsError",
            "#[non_exhaustive] pub promptforge::VfsError::Missing { .. }",
            "pub promptforge::VfsError::Denied",
            "pub promptforge::Code::First = 1",
        ],
        &[
            "pub enum promptforge::VfsError",
            "pub promptforge::VfsError::Missing { .. }",
            "#[non_exhaustive] pub enum promptforge::Verdict",
            "#[non_exhaustive] pub promptforge::VfsError::Denied",
        ],
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn marking_a_blessed_unit_struct_non_exhaustive_fails_check() {
    let root = workspace(UNIT, UNIT_FACADE);
    let blessed = execute(root.path(), Mode::Bless).expect("the fixture documents");
    assert!(!blessed.failed, "{:?}", blessed.lines);
    write(
        root.path(),
        "crates/promptforge-internal/inner/src/lib.rs",
        &UNIT.replace(
            "pub struct AllowAll;",
            "#[non_exhaustive]\npub struct AllowAll;",
        ),
    );
    let changed = execute(root.path(), Mode::Check).expect("the fixture documents");
    assert!(changed.failed, "{:?}", changed.lines);
    assert!(
        changed
            .lines
            .contains(&"- pub struct promptforge::AllowAll;".to_owned()),
        "{:?}",
        changed.lines
    );
    assert!(
        changed
            .lines
            .contains(&"+ #[non_exhaustive] pub struct promptforge::AllowAll;".to_owned()),
        "{:?}",
        changed.lines
    );
}
