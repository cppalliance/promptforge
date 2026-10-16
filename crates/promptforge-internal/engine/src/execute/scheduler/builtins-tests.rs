//! Tests for the task built-ins' one vocabulary: each built-in is named,
//! looked up, advertised, and routed under the same name, in one order.

use std::collections::BTreeMap;

use super::{TaskBuiltin, advertise_task_builtins};
use crate::execute::scope::DispatchTarget;
use crate::lua::TaskAllowlist;
use crate::model::ToolSchema;

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
