//! Doc-text fixtures: internal crate names in either spelling are found
//! in facade module docs and surface item docs, and only as whole names.

use super::super::fixture::{findings, set, workspace};
use super::*;

#[test]
fn a_crate_name_matches_only_as_a_whole_name() {
    for text in [
        "built on runvil-inner.",
        "`runvil-inner`",
        "runvil_inner::Visible",
        "(runvil-inner)",
        "runvil-inner",
    ] {
        let name = if text.contains('_') {
            "runvil_inner"
        } else {
            "runvil-inner"
        };
        assert!(contains_name(text, name), "{text}");
    }
    for text in [
        "runvil-innerly",
        "my-runvil-inner",
        "runvil-inner-two",
        "xrunvil_inner",
        "runvil_inner2",
    ] {
        let name = if text.contains('_') {
            "runvil_inner"
        } else {
            "runvil-inner"
        };
        assert!(!contains_name(text, name), "{text}");
    }
}

#[test]
#[ignore = "needs the pinned nightly"]
fn an_internal_crate_name_in_doc_text_is_reported() {
    let root = workspace(
        "//! Inner.\n\n/// Exposed.\n/// Built in runvil-inner.\npub struct Visible {\n    \
         /// Named `runvil_inner::Visible` inside.\n    pub field: u8,\n}\n\n\
         /// Not a name: runvil-innerly.\npub struct Clean;\n",
        "//! Facade over runvil-inner.\npub use runvil_inner::Clean;\n\
         pub use runvil_inner::Visible;\n",
    );
    let required = "required surface doc text that names no internal crate";
    assert_eq!(
        findings(root.path()),
        set([
            format!(
                "runvil: mentions internal crate name `runvil-inner` in its doc text: \
                 {required}, found `runvil-inner` on doc line 1"
            ),
            format!(
                "runvil::Visible: mentions internal crate name `runvil-inner` in its \
                 doc text: {required}, found `runvil-inner` on doc line 2"
            ),
            format!(
                "runvil::Visible::field: mentions internal crate name `runvil_inner` \
                 in its doc text: {required}, found `runvil_inner` on doc line 1"
            ),
        ])
    );
}
