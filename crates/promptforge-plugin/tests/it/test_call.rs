//! `TestCall`: the context a Plugin crate's own test lends a call it makes
//! directly.

use std::sync::Arc;

use promptforge_plugin::testing::TestCall;
use promptforge_plugin::{HostServices, ServiceKey, ToolCaller, ToolId};

const GREETING: ServiceKey<str> = ServiceKey::new("acme/greeting");

fn inspect() -> ToolId {
    ToolId::parse("acme/inspect").expect("a two-segment tool id parses")
}

#[test]
fn a_test_call_lends_its_tool_a_script_origin_and_a_working_filesystem() {
    let call = TestCall::new(inspect());
    let cx = call.context();
    assert_eq!(cx.tool(), &inspect());
    assert_eq!(cx.origin().caller, ToolCaller::Script);
    cx.access()
        .write("notes.md", b"hello")
        .expect("the in-memory filesystem accepts a write");
    assert_eq!(
        cx.access()
            .read("notes.md")
            .expect("the written file reads back"),
        b"hello"
    );
    assert!(
        cx.service(&GREETING).is_none(),
        "a call built without services has none"
    );
}

#[test]
fn a_test_call_lends_the_services_it_was_given() {
    let mut services = HostServices::new();
    services
        .provide(&GREETING, Arc::from("hello"))
        .expect("a valid, new id is accepted");
    let call = TestCall::new(inspect()).with_services(services);
    assert_eq!(call.context().service(&GREETING).as_deref(), Some("hello"));
}

#[test]
fn each_test_call_has_a_filesystem_of_its_own() {
    let first = TestCall::new(inspect());
    first
        .context()
        .access()
        .write("left.txt", b"1")
        .expect("the in-memory filesystem accepts a write");
    let second = TestCall::new(inspect());
    assert!(
        !second
            .context()
            .access()
            .exists("left.txt")
            .expect("an existence check succeeds"),
        "a second call starts from an empty filesystem"
    );
}
