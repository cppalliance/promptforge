//! Re-export resolution fixtures: a module re-export and a second facade
//! path for one item are reported; the rustdoc JSON supplies the item
//! kinds syntax cannot. The allowlist's approved crates pass closure, and
//! the closure requirement names each one.

use super::super::fixture::{findings, set, workspace};
use super::*;

#[test]
fn every_approved_crate_passes_closure_and_any_other_is_refused() {
    let surface = Surface::default();
    for krate in ["serde", "serde_core", "serde_json", "serde_yaml_ng"] {
        let path = [krate.to_owned(), "Value".to_owned()];
        assert_eq!(surface.refusal(krate, &path), None, "{krate}");
    }
    let path = ["toml".to_owned(), "Value".to_owned()];
    assert_eq!(
        surface.refusal("toml", &path).as_deref(),
        Some("an item of crate `toml`, which is not on the allowlist")
    );
}

#[test]
fn the_closure_requirement_names_every_crate_it_permits() {
    let required = required();
    let words: Vec<&str> = required
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .collect();
    for krate in STD_CRATES.iter().chain(&ALLOWLIST) {
        assert!(words.contains(krate), "{krate} in {required}");
    }
}

#[test]
fn a_path_table_entry_the_index_lacks_is_no_re_export_target() {
    let summary = rustdoc_types::ItemSummary {
        crate_id: 0,
        path: vec!["promptforge_inner".to_owned(), "Gone".to_owned()],
        kind: ItemKind::Struct,
    };
    let krate = Crate {
        root: Id(0),
        crate_version: None,
        includes_private: false,
        index: HashMap::new(),
        paths: HashMap::from([(Id(1), summary)]),
        external_crates: HashMap::new(),
        target: rustdoc_types::Target {
            triple: String::new(),
            target_features: Vec::new(),
        },
        format_version: rustdoc_types::FORMAT_VERSION,
    };
    assert!(local_paths(&krate).is_empty());
}

#[test]
#[ignore = "needs the pinned nightly"]
fn a_module_re_export_is_reported() {
    let root = workspace(
        "//! Inner.\n\n/// A module.\npub mod nested {\n    /// Inside.\n    pub struct Deep;\n}\n",
        "//! Facade.\npub use promptforge_inner::nested;\n",
    );
    assert_eq!(
        findings(root.path()),
        set([
            "promptforge::nested: mentions `promptforge_inner::nested` as its re-export \
              target: required a single-item re-export, found a module"
                .to_owned()
        ])
    );
}

#[test]
#[ignore = "needs the pinned nightly"]
fn a_second_facade_path_for_one_item_is_reported() {
    let root = workspace(
        "//! Inner.\n\n/// Exposed.\npub struct Visible;\n",
        "//! Facade.\npub use promptforge_inner::Visible;\n\n\
         /// Again.\npub mod again {\n    pub use promptforge_inner::Visible;\n}\n",
    );
    assert_eq!(
        findings(root.path()),
        set([
            "promptforge::again::Visible: mentions `promptforge_inner::Visible` as its \
              re-export target: required exactly one facade path per item, found a second \
              facade path beside `promptforge::Visible`"
                .to_owned()
        ])
    );
}
