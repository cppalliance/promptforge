//! Tests for the context one tool call receives: the getters and the typed
//! service lookup.

use std::sync::Arc;

use promptforge_types::tools::{ToolCallOrigin, ToolCaller, ToolId};
use promptforge_vfs::{Access, Origin, VfsRef};

use super::ToolContext;
use crate::{HostServices, ServiceKey};

const GREETING: ServiceKey<str> = ServiceKey::new("acme/greeting");

fn fetch() -> ToolId {
    ToolId::parse("web/fetch").expect("a two-segment tool id parses")
}

fn fresh_access() -> Access {
    VfsRef::default()
        .acquire(Origin::new("context test"))
        .expect("a fresh memory filesystem acquires")
}

fn model_origin() -> ToolCallOrigin {
    ToolCallOrigin {
        execution: "context-test".to_owned(),
        section: "Fetch".to_owned(),
        caller: ToolCaller::Model,
    }
}

#[test]
fn each_getter_returns_the_value_the_context_was_built_from() {
    let (tool, access, origin) = (fetch(), fresh_access(), model_origin());
    let services = HostServices::new();
    let cx = ToolContext::new(&tool, &access, &origin, &services);
    assert!(std::ptr::eq(cx.tool(), &raw const tool));
    assert!(std::ptr::eq(cx.access(), &raw const access));
    assert!(std::ptr::eq(cx.origin(), &raw const origin));
    assert_eq!(cx.tool().name(), "fetch");
    assert_eq!(cx.origin().caller, ToolCaller::Model);
}

#[test]
fn a_service_is_found_by_its_key_and_an_absent_or_mistyped_one_is_not() {
    let (tool, access, origin) = (fetch(), fresh_access(), model_origin());
    let mut services = HostServices::new();
    services
        .provide(&GREETING, Arc::from("hello"))
        .expect("a valid, new id is accepted");
    let cx = ToolContext::new(&tool, &access, &origin, &services);
    assert_eq!(cx.service(&GREETING).as_deref(), Some("hello"));
    assert!(
        cx.service(&ServiceKey::<str>::new("acme/farewell"))
            .is_none(),
        "a service the run lacks is None"
    );
    assert!(
        cx.service(&ServiceKey::<u32>::new("acme/greeting"))
            .is_none(),
        "a service provided as another type is None"
    );
}
