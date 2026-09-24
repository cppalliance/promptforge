//! Closure fixtures: each generated workspace exposes one kind of leak, or
//! one kind of allowed mention, through the facade over one internal
//! crate. Each asserts the exact findings, so an allowed mention that is
//! reported fails as surely as a leak that is missed.

use std::collections::BTreeSet;

use super::super::fixture::{findings, leak, set, workspace, workspace_with_outer};

#[test]
#[ignore = "needs the pinned nightly"]
fn a_type_in_a_signature_that_is_not_re_exported_is_reported() {
    let root = workspace(
        "//! Inner.\n\n/// Kept internal.\npub struct Secret;\n\n\
         /// Leaks it.\npub fn make() -> Secret {\n    Secret\n}\n",
        "//! Facade.\npub use promptforge_inner::make;\n",
    );
    let leak = leak("promptforge_inner");
    assert_eq!(
        findings(root.path()),
        set([format!(
            "promptforge::make: mentions `promptforge_inner::Secret` in its signature: {leak}"
        )])
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn an_internal_trait_in_a_bound_or_supertrait_is_reported() {
    let root = workspace(
        "//! Inner.\n\n/// Internal hook.\npub trait Hook {}\n\n\
         /// Bounded by it.\npub fn run<T: Hook>(_hook: T) {}\n\n\
         /// Extends it.\npub trait Surface: Hook {}\n",
        "//! Facade.\npub use promptforge_inner::Surface;\npub use promptforge_inner::run;\n",
    );
    let leak = leak("promptforge_inner");
    assert_eq!(
        findings(root.path()),
        set([
            format!(
                "promptforge::Surface: mentions `promptforge_inner::Hook` in its supertrait: {leak}"
            ),
            format!("promptforge::run: mentions `promptforge_inner::Hook` in its bound: {leak}"),
        ])
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn a_trait_impl_that_mentions_an_internal_type_is_reported() {
    let root = workspace(
        "//! Inner.\n\n/// Kept internal.\npub struct Secret;\n\n\
         /// Exposed.\npub struct Visible;\n\n\
         impl From<Secret> for Visible {\n    fn from(_: Secret) -> Self {\n        Visible\n    }\n}\n",
        "//! Facade.\npub use promptforge_inner::Visible;\n",
    );
    let header = "impl core::convert::From<promptforge_inner::Secret> for promptforge::Visible";
    let leak = leak("promptforge_inner");
    assert_eq!(
        findings(root.path()),
        set([
            format!(
                "{header}: mentions `promptforge_inner::Secret` in its implemented trait: {leak}"
            ),
            format!(
                "{header} {{ fn from }}: mentions `promptforge_inner::Secret` in its signature: {leak}"
            ),
        ])
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn an_impl_of_an_internal_trait_on_a_surface_type_is_reported() {
    let root = workspace(
        "//! Inner.\n\n/// Internal hook.\npub trait Hook {}\n\n\
         /// Exposed.\npub struct Visible;\n\nimpl Hook for Visible {}\n",
        "//! Facade.\npub use promptforge_inner::Visible;\n",
    );
    let leak = leak("promptforge_inner");
    assert_eq!(
        findings(root.path()),
        set([format!(
            "impl promptforge_inner::Hook for promptforge::Visible: mentions \
             `promptforge_inner::Hook` in its implemented trait: {leak}"
        )])
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn an_impl_written_in_a_downstream_internal_crate_is_reported() {
    let root = workspace_with_outer(
        "//! Inner.\n\n/// Exposed.\npub struct Visible;\n",
        "//! Outer.\n\n/// Internal hook.\npub trait Hook {}\n\n\
         impl Hook for promptforge_inner::Visible {}\n",
        "//! Facade.\npub use promptforge_inner::Visible;\n",
    );
    let leak = leak("promptforge_outer");
    assert_eq!(
        findings(root.path()),
        set([format!(
            "impl promptforge_outer::Hook for promptforge::Visible: mentions \
             `promptforge_outer::Hook` in its implemented trait: {leak}"
        )])
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn a_type_from_an_allowlisted_crate_passes() {
    let root = workspace(
        "//! Inner.\n\n/// Exposed.\npub struct Visible;\n\nimpl serde::Serialize for Visible {}\n\n\
         /// Takes any serializable value.\npub fn encode(_value: &dyn serde::Serialize) {}\n",
        "//! Facade.\npub use promptforge_inner::Visible;\npub use promptforge_inner::encode;\n",
    );
    assert_eq!(findings(root.path()), BTreeSet::new());
}

#[test]
#[ignore = "needs the pinned nightly"]
fn a_link_to_an_internal_item_written_through_crate_or_super_is_reported() {
    let root = workspace(
        "//! Inner.\n\n/// Kept internal.\npub struct Secret;\n\n\
         /// Exposed; see [crate::Secret].\npub struct Visible;\n\n\
         /// A module.\npub mod nested {\n    \
         /// Also exposed; see [super::Secret] and [super::Visible].\n    pub struct Other;\n}\n",
        "//! Facade.\npub use promptforge_inner::Visible;\npub use promptforge_inner::nested::Other;\n",
    );
    let leak = leak("promptforge_inner");
    assert_eq!(
        findings(root.path()),
        set([
            format!(
                "promptforge::Other: mentions `promptforge_inner::Secret` through its doc link \
                 `[super::Secret]`: {leak}"
            ),
            format!(
                "promptforge::Visible: mentions `promptforge_inner::Secret` through its doc link \
                 `[crate::Secret]`: {leak}"
            ),
        ])
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn a_link_whose_target_the_facade_re_exports_passes() {
    let root = workspace(
        "//! Inner.\n\n/// Exposed; see [crate::Other], [Other::run], [Kind::A], and [String].\n\
         pub struct Visible;\n\n/// Also exposed.\npub struct Other;\n\n\
         impl Other {\n    /// Runs.\n    pub fn run(&self) {}\n}\n\n\
         /// Kinds.\npub enum Kind {\n    /// The first.\n    A,\n}\n",
        "//! Facade; see [Visible].\npub use promptforge_inner::Kind;\n\
         pub use promptforge_inner::Other;\npub use promptforge_inner::Visible;\n",
    );
    assert_eq!(findings(root.path()), BTreeSet::new());
}
