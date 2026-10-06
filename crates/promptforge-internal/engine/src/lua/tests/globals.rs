//! The reserved-name list against the globals a section VM holds after the
//! executor's real setup path, compared in both directions: every global a
//! walked section or the H1 pass leaves in `_G` is a reserved name, and
//! every reserved global reads non-nil once the VM binds its conditional
//! ones (`ui`, `item`, `prose`). No Plugin prelude and no frontmatter
//! alias installs here, so what `_G` holds is exactly what the Engine installs.

use std::sync::{Arc, Mutex};

use mlua::Value;
use serde_json::json;

use promptforge_lua::{RESERVED_NAMES, Reserved, reserved_name};

use crate::execute::section_vm::{SectionVmSetup, VmSeed, setup_section_vm};
use crate::lua::{LuaProgram, SectionVm, ToolSet};
use crate::model::ModelSet;
use crate::test_support::recording::null_emitter;
use crate::untrusted::GuardNonce;

/// Builds a section VM through the real setup path with every conditional
/// Engine global bound: a caller-supplied application-state snapshot
/// (`ui`), a collection member (`item`), a non-nil `argv` (writable as in
/// the H1 pass, or frozen as in every other section), and a block's lazy
/// `prose`.
fn fully_bound_vm(argv_writable: bool) -> SectionVm {
    let emitter = null_emitter();
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::from_seed(0x9e5),
        &Arc::new(Mutex::new(ToolSet::default())),
        &Arc::new(Mutex::new(ModelSet::default())),
        &emitter,
        "Globals",
    )
    .expect("the section VM builds");
    let shared = LuaProgram::empty().expect("the empty shared program compiles");
    let sys = json!({});
    let argv = json!({ "prose": "" });
    let item = json!("member");
    let ui = Arc::new(json!({}));
    let access = Arc::new(
        promptforge_vfs::VfsRef::default()
            .acquire(promptforge_vfs::Origin::new("reserved names fixture"))
            .expect("the stock backend acquires"),
    );
    let setup = SectionVmSetup {
        args: "",
        argv: Some(&argv),
        argv_writable,
        sys: &sys,
        access: &access,
        seed: VmSeed {
            var: None,
            item: Some(&item),
        },
        emitter: &emitter,
        section_name: "Globals",
        shared: &shared,
        max_tool_iterations: 24,
        ui: Some(&ui),
        preludes: &[],
        frontmatter_aliases: &[],
        raw_shims: false,
    };
    let list_callback =
        |_: String| -> std::result::Result<Vec<String>, crate::Error> { Ok(Vec::new()) };
    setup_section_vm(&mut vm, &setup, list_callback).expect("the setup installs");
    vm.install_lazy_prose(|_| Ok("the block's prose".to_owned()))
        .expect("the prose guard installs");
    vm
}

/// Every key the VM's globals table holds, read raw (no metamethod runs).
fn raw_global_names(vm: &SectionVm) -> Vec<String> {
    vm.lua()
        .globals()
        .pairs::<Value, Value>()
        .map(|pair| match pair.expect("the globals walk").0 {
            Value::String(name) => name.to_string_lossy(),
            other => format!("{other:?}"),
        })
        .collect()
}

#[test]
fn every_global_section_setup_leaves_in_g_is_a_reserved_name() {
    for argv_writable in [false, true] {
        let vm = fully_bound_vm(argv_writable);
        for name in raw_global_names(&vm) {
            assert!(
                matches!(
                    reserved_name(&name),
                    Some(Reserved::EngineGlobal | Reserved::LuaGlobal)
                ),
                "section setup (argv writable: {argv_writable}) installs the global `{name}`, \
                 which RESERVED_NAMES does not list as an Engine or Lua global; list it there"
            );
        }
    }
}

#[test]
fn every_reserved_global_is_present_after_section_setup() {
    for argv_writable in [false, true] {
        let vm = fully_bound_vm(argv_writable);
        let globals = vm.lua().globals();
        for (name, kind) in RESERVED_NAMES {
            if kind == Reserved::LuaKeyword {
                continue;
            }
            let value: Value = globals.get(name).expect("a global read");
            assert!(
                !value.is_nil(),
                "RESERVED_NAMES lists `{name}` as {kind}, but a fully bound section VM \
                 (argv writable: {argv_writable}) reads it as nil; drop it or install it"
            );
        }
    }
}
