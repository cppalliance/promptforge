//! Doc-text fixtures: internal crate names in either spelling are found
//! in facade module docs and surface item docs, and only as whole names.

use super::super::fixture::{findings, set, workspace};
use super::*;

#[test]
fn a_crate_name_matches_only_as_a_whole_name() {
    for text in [
        "built on promptforge-inner.",
        "`promptforge-inner`",
        "promptforge_inner::Visible",
        "(promptforge-inner)",
        "promptforge-inner",
    ] {
        let name = if text.contains('_') {
            "promptforge_inner"
        } else {
            "promptforge-inner"
        };
        assert!(contains_name(text, name), "{text}");
    }
    for text in [
        "promptforge-innerly",
        "my-promptforge-inner",
        "promptforge-inner-two",
        "xpromptforge_inner",
        "promptforge_inner2",
    ] {
        let name = if text.contains('_') {
            "promptforge_inner"
        } else {
            "promptforge-inner"
        };
        assert!(!contains_name(text, name), "{text}");
    }
}

#[test]
#[ignore = "needs the pinned nightly"]
fn an_internal_crate_name_in_doc_text_is_reported() {
    let root = workspace(
        "//! Inner.\n\n/// Exposed.\n/// Built in promptforge-inner.\npub struct Visible {\n    \
         /// Named `promptforge_inner::Visible` inside.\n    pub field: u8,\n}\n\n\
         /// Not a name: promptforge-innerly.\npub struct Clean;\n",
        "//! Facade over promptforge-inner.\npub use promptforge_inner::Clean;\n\
         pub use promptforge_inner::Visible;\n",
    );
    let required = "required surface doc text that names no internal crate";
    assert_eq!(
        findings(root.path()),
        set([
            format!(
                "promptforge: mentions internal crate name `promptforge-inner` in its doc text: \
                 {required}, found `promptforge-inner` on doc line 1"
            ),
            format!(
                "promptforge::Visible: mentions internal crate name `promptforge-inner` in its \
                 doc text: {required}, found `promptforge-inner` on doc line 2"
            ),
            format!(
                "promptforge::Visible::field: mentions internal crate name `promptforge_inner` \
                 in its doc text: {required}, found `promptforge_inner` on doc line 1"
            ),
        ])
    );
}
