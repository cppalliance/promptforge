//! Tests for the tool binding built from a catalog descriptor: the
//! descriptor's data is copied verbatim and its structured-output flag
//! selects the binding's output kind. The tool set keeps its offering
//! apart from the frontmatter's slots.

use std::sync::Mutex;

use promptforge_types::tools::{ToolDescriptor, ToolId};
use serde_json::json;

use super::{ToolBinding, ToolOutputKind, ToolSet, ToolView};

/// A descriptor for `tools/fetch`.
fn descriptor(structured: bool) -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse("tools/fetch").expect("the id is valid"),
        "Fetch a page",
        json!({"type": "object", "properties": {"url": {"type": "string"}}}),
    )
    .structured(structured)
}

#[test]
fn a_structured_descriptor_binds_with_structured_output() {
    let descriptor = descriptor(true);
    let binding = ToolBinding::from_descriptor("fetch_alias", &descriptor);
    assert_eq!(binding.output_kind, ToolOutputKind::Structured);
    assert_eq!(binding.alias(), "fetch_alias");
    assert_eq!(binding.id(), &descriptor.id);
    assert_eq!(binding.description(), "Fetch a page");
    assert_eq!(binding.schema(), &descriptor.parameters_schema);
    assert!(binding.model_description().is_none());
}

#[test]
fn a_plain_descriptor_binds_with_plain_output() {
    let binding = ToolBinding::from_descriptor("fetch_alias", &descriptor(false));
    assert_eq!(binding.output_kind, ToolOutputKind::Plain);
}

#[test]
fn the_test_seam_overrides_only_the_description() {
    let descriptor = descriptor(true);
    let binding = ToolBinding::for_test("fetch_alias", "test double", &descriptor);
    assert_eq!(binding.description(), "test double");
    assert_eq!(
        binding.output_kind,
        ToolOutputKind::Structured,
        "the output kind still follows the descriptor"
    );
}

#[test]
fn an_offered_binding_is_found_by_its_name_and_never_as_a_declared_slot() {
    let declared = ToolBinding::from_descriptor("fetch", &descriptor(false));
    let offered = ToolBinding::from_descriptor("tools_fetch", &descriptor(false));
    let set = ToolSet::from_parts(vec![declared.clone()], Vec::new(), vec![offered.clone()]);
    assert_eq!(set.offered(), std::slice::from_ref(&offered));
    assert_eq!(set.offered_binding("tools_fetch"), Some(&offered));
    assert_eq!(
        set.binding("tools_fetch"),
        None,
        "binding sees the frontmatter's slots alone"
    );
    assert_eq!(
        set.offered_binding("fetch"),
        None,
        "a declared slot is not part of the offering"
    );
    assert_eq!(set.binding("fetch"), Some(&declared));
}

#[test]
fn the_view_snapshots_the_offering() {
    let offered = ToolBinding::from_descriptor("tools_fetch", &descriptor(false));
    let set = Mutex::new(ToolSet::for_test(
        Vec::new(),
        Vec::new(),
        vec![offered.clone()],
    ));
    assert_eq!(
        ToolView::offered(&set).expect("the lock is healthy"),
        vec![offered]
    );
}
