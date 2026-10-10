//! The reserved-name list against the globals a section VM holds after the
//! executor's real setup path, compared in both directions: every global a
//! walked section or the H1 pass leaves in `_G` is a reserved name, and
//! every reserved global reads non-nil once the VM binds its conditional
//! ones (`ui`, `item`, `prose`). No Plugin prelude and no frontmatter tool
//! alias installs here, so what `_G` holds is exactly what the Engine
//! installs. Model roles bind here too, since a role label is never a global.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use mlua::Value;
use serde_json::json;

use promptforge_lua::{RESERVED_NAMES, Reserved, reserved_name};
use promptforge_model_client::model::ModelInvocation;
use promptforge_types::detail::model_id_from_validated;

use crate::execute::section_vm::{SectionVmSetup, VmSeed, setup_section_vm};
use crate::lua::{LuaProgram, SectionVm, ToolSet};
use crate::model::{ModelBinding, ModelSet};
use crate::test_support::recording::null_emitter;
use crate::untrusted::GuardNonce;

/// The role labels every fully bound VM fills: one plain label, and one
/// that names an Engine global.
const ROLE_LABELS: [&str; 2] = ["writer", "tools"];

/// One role bound under each of [`ROLE_LABELS`].
fn roles() -> ModelSet {
    ModelSet {
        bindings: ROLE_LABELS
            .iter()
            .map(|label| {
                ModelBinding::new(
                    *label,
                    "a role",
                    model_id_from_validated("gateway", "test-model"),
                    ModelInvocation {
                        temperature: None,
                        max_tokens: None,
                        thinking: None,
                    },
                    NonZeroU32::new(4096).expect("4096 is non-zero"),
                )
            })
            .collect(),
        default: None,
    }
}

/// Builds a section VM through the real setup path with every conditional
/// Engine global bound: a caller-supplied application-state snapshot
/// (`ui`), a collection member (`item`), a non-nil `argv` (writable as in
/// the H1 pass, or frozen as in every other section), a block's lazy
/// `prose`, and the model roles in [`ROLE_LABELS`].
fn fully_bound_vm(argv_writable: bool) -> SectionVm {
    let emitter = null_emitter();
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::from_seed(0x9e5),
        &Arc::new(Mutex::new(ToolSet::default())),
        &Arc::new(Mutex::new(roles())),
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

#[test]
fn no_role_label_is_a_global_after_section_or_h1_setup() {
    for argv_writable in [false, true] {
        let vm = fully_bound_vm(argv_writable);
        let (writer, tools, call): (String, String, String) = vm
            .lua()
            .load("return type(writer), type(tools), type(tools.call)")
            .eval()
            .expect("the globals probe evaluates");
        assert_eq!(
            (writer.as_str(), tools.as_str(), call.as_str()),
            ("nil", "table", "function"),
            "a role label installs no global and leaves a same-named Engine global in place \
             (argv writable: {argv_writable})"
        );
    }
}

#[test]
fn models_get_returns_the_role_handle_after_section_or_h1_setup() {
    for argv_writable in [false, true] {
        let vm = fully_bound_vm(argv_writable);
        for label in ROLE_LABELS {
            let (kind, name, model_id): (String, String, String) = vm
                .lua()
                .load(format!(
                    "local h = models.get({label:?}) return type(h), h.name, h.model_id"
                ))
                .eval()
                .expect("the handle probe evaluates");
            assert_eq!(
                (kind.as_str(), name.as_str(), model_id.as_str()),
                ("userdata", label, "test-model"),
                "models.get({label:?}) hands back the role's handle \
                 (argv writable: {argv_writable})"
            );
        }
    }
}
