//! Tests for the task built-ins' one vocabulary: each built-in is named,
//! looked up, advertised, and routed under the same name, in one order;
//! and for the failure kind of the allowlist read.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;

use super::{TaskBuiltin, advertise_task_builtins, task_allowlist};
use crate::Error;
use crate::execute::scope::DispatchTarget;
use crate::lua::{SectionVm, TaskAllowlist};
use crate::model::ToolSchema;
use crate::test_support::fresh_access;
use crate::test_support::recording::null_emitter;
use crate::test_support::scenario::TEST_SEED;
use crate::untrusted::GuardNonce;

#[test]
fn every_built_in_is_advertised_and_routed_under_its_own_name() {
    let mut schemas = Vec::new();
    let mut dispatch = BTreeMap::new();
    advertise_task_builtins(&mut schemas, &mut dispatch, &TaskAllowlist::Any)
        .expect("the fixed schemas validate");
    let names: Vec<&str> = TaskBuiltin::ALL
        .iter()
        .map(|builtin| builtin.name())
        .collect();
    assert_eq!(names, ["task", "task_cancel", "task_status", "await_tasks"]);
    let advertised: Vec<&str> = schemas.iter().map(ToolSchema::name).collect();
    assert_eq!(
        advertised, names,
        "the model sees the built-ins in that order"
    );
    for builtin in TaskBuiltin::ALL {
        assert_eq!(TaskBuiltin::from_name(builtin.name()), Some(builtin));
        assert!(
            matches!(dispatch.get(builtin.name()), Some(DispatchTarget::Builtin)),
            "required `{}` to route to the arena, actual {dispatch:?}",
            builtin.name()
        );
    }
    assert_eq!(TaskBuiltin::from_name("web_fetch"), None);
}

#[test]
fn a_poisoned_tool_runtime_maps_to_an_internal_error() {
    // No prompt author can poison the section's tool runtime: its writers
    // decode before they lock, and a Lua `error()` is not a panic. The
    // lock failure is Engine misuse, never a Lua failure.
    let emitter = null_emitter();
    let vm = SectionVm::new(&GuardNonce::from_seed(&TEST_SEED), &emitter, "Only")
        .expect("the section VM builds")
        .inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    let poisoner = Arc::clone(vm.tool_runtime());
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("poison the tool runtime");
    })
    .join();

    let error = task_allowlist(&vm).expect_err("a poisoned runtime cannot be read");
    assert!(
        matches!(&error, Error::Internal { message, .. } if message == "tool declaration runtime was poisoned"),
        "required an internal error, actual {error:?}"
    );
    vm.teardown(&emitter, "Only");
}
