//! Tests for Plugin prelude install: the restricted environment, the
//! collision checks, the top-level seal, and load failures.

use std::num::NonZeroU32;
use std::sync::Arc;

use mlua::{FromLuaMulti, Value};
use promptforge_types::plugins::{PluginId, Prelude};
use promptforge_types::untrusted::GuardNonce;
use serde_json::json;

use super::install_preludes;
use crate::tests::recording::null_emitter;
use crate::tests::refusal_line;
use crate::{
    Argv, CoroStep, LuaProgram, Request, SectionVm, YieldParse, install_section_loop_shim,
    install_ui,
};

#[path = "prelude-tests-environment.rs"]
mod environment;

const SECTION: &str = "Prelude";

/// A fresh default handle's access capability for a test VM.
fn fresh_access() -> Arc<crate::Access> {
    Arc::new(
        promptforge_vfs::VfsRef::default()
            .acquire(promptforge_vfs::Origin::new("prelude test fixture"))
            .expect("the stock backend acquires"),
    )
}

/// Builds a section VM through section setup up to where preludes install:
/// Engine injection with a bound `argv`, the Engine globals, the control
/// globals, and every coroutine yield shim. `var` seeds the guarded `var`.
fn section_vm_with_var(var: Option<&serde_json::Value>) -> SectionVm {
    let emitter = null_emitter();
    let mut vm =
        SectionVm::new(&GuardNonce::from_seed(7), &emitter, SECTION).expect("the VM builds");
    let argv = json!({ "mode": "argv" });
    vm.inject_values_with_var(
        "",
        &json!({ "id": 1 }),
        &fresh_access(),
        var,
        Argv::Frozen(Some(&argv)),
    )
    .expect("Engine values inject");
    vm.install_engine_globals(&emitter, SECTION)
        .expect("the Engine globals install");
    vm.install_scheduler_control_globals(|_| {
        Ok::<Vec<String>, std::convert::Infallible>(Vec::new())
    })
    .expect("the control globals install");
    vm.install_coro_shims(1)
        .expect("the coroutine shims install");
    install_section_loop_shim(vm.lua()).expect("the loop shim installs");
    vm
}

fn section_vm() -> SectionVm {
    section_vm_with_var(None)
}

fn prelude(plugin: &str, source: &str) -> Prelude {
    Prelude::new(PluginId::parse(plugin).expect("a valid Plugin id"), source)
}

/// Installs `preludes` and returns the failure's message.
fn install_failure(vm: &SectionVm, preludes: &[Prelude]) -> String {
    install_preludes(vm.lua(), preludes)
        .expect_err("the install must fail")
        .to_string()
}

/// Runs author code on the VM's main state and returns its values.
fn eval<T: FromLuaMulti>(vm: &SectionVm, source: &str) -> T {
    vm.lua().load(source).eval().expect("the author chunk runs")
}

/// Every name bound in the VM's `_G`, sorted.
fn global_names(vm: &SectionVm) -> Vec<String> {
    let mut names: Vec<String> = vm
        .lua()
        .globals()
        .pairs::<Value, Value>()
        .map(|pair| {
            let (key, _) = pair.expect("the globals walk");
            match key {
                Value::String(name) => name.to_string_lossy(),
                other => format!("{other:?}"),
            }
        })
        .collect();
    names.sort();
    names
}

/// The part of a failure message after `stack traceback:`.
fn traceback(message: &str) -> &str {
    message
        .split_once("stack traceback:")
        .map_or("", |(_, traceback)| traceback)
}

#[test]
fn a_table_global_is_sealed_at_its_top_level() {
    let vm = section_vm();
    install_preludes(
        vm.lua(),
        &[prelude(
            "kit",
            "kit = {}\nfunction kit.greet(name) return 'hi ' .. name end",
        )],
    )
    .expect("the prelude installs");

    let greeting: String = eval(&vm, "return kit.greet('ada')");
    assert_eq!(
        greeting, "hi ada",
        "the sealed table still reads its fields"
    );
    let (ok, message): (bool, String) = eval(
        &vm,
        "local ok, err = pcall(function() kit.extra = 1 end)\nreturn ok, tostring(err)",
    );
    assert!(!ok, "assigning a field of a sealed table raises");
    assert!(
        message.contains("kit is read-only")
            && message.contains("Plugin `kit`")
            && message.contains("'extra'"),
        "the refusal names the global, its Plugin, and the field: {message}"
    );
    let seal: String = eval(&vm, "return getmetatable(kit)");
    assert_eq!(seal, "kit is sealed");
    let seen: i64 = eval(
        &vm,
        "local n = 0\nfor _ in pairs(kit) do n = n + 1 end\nreturn n",
    );
    assert_eq!(seen, 0, "pairs over the sealed proxy sees nothing");
}

#[test]
fn a_sealed_global_raises_the_whole_refusal_for_a_string_or_integer_key() {
    let vm = section_vm();
    install_preludes(vm.lua(), &[prelude("kit", "kit = {}")]).expect("the prelude installs");
    for (target, refusal) in [
        (
            "kit.extra",
            "kit is read-only: Plugin `kit` defines it; cannot set 'extra'",
        ),
        (
            "kit[1]",
            "kit is read-only: Plugin `kit` defines it; cannot set 'Integer(1)'",
        ),
    ] {
        let message: String = eval(
            &vm,
            &format!("local ok, err = pcall(function() {target} = 1 end)\nreturn tostring(err)"),
        );
        assert_eq!(refusal_line(&message), Some(refusal), "writing {target}");
    }
}

#[test]
fn a_non_table_global_installs_as_it_is() {
    let vm = section_vm();
    install_preludes(
        vm.lua(),
        &[prelude(
            "kit",
            "answer = 42\nlabel = 'kit'\nfunction shout(text) return string.upper(text) end",
        )],
    )
    .expect("the prelude installs");

    let (answer, label, kind, shouted): (i64, String, String, String) =
        eval(&vm, "return answer, label, type(shout), shout('hey')");
    assert_eq!(
        (answer, label.as_str(), kind.as_str(), shouted.as_str()),
        (42, "kit", "function", "HEY")
    );
}

#[test]
fn a_prelude_receives_its_plugins_name_as_its_chunk_argument() {
    let vm = section_vm();
    install_preludes(
        vm.lua(),
        &[
            prelude("kit", "local plugin = ...\nkit_name = plugin"),
            prelude("other-kit", "other_name = ..."),
        ],
    )
    .expect("the preludes install");
    let (kit, other): (String, String) = eval(&vm, "return kit_name, other_name");
    assert_eq!(
        (kit.as_str(), other.as_str()),
        ("kit", "other-kit"),
        "each prelude reads its own Plugin's local name from `...`"
    );
}

#[test]
fn a_prelude_that_assigns_no_global_installs_nothing() {
    let vm = section_vm();
    let before = global_names(&vm);
    install_preludes(
        vm.lua(),
        &[prelude(
            "quiet",
            "local helper = 1\nlocal function unused() return helper end",
        )],
    )
    .expect("the prelude installs");
    assert_eq!(global_names(&vm), before);
}

#[test]
fn a_global_that_collides_with_an_engine_global_fails_naming_both_sides() {
    let vm = section_vm();
    let message = install_failure(&vm, &[prelude("kit", "store = {}")]);
    assert!(
        message.contains("Plugin `kit`")
            && message.contains("`store`")
            && message.contains("Engine global"),
        "the collision names the Plugin, the global, and the Engine global: {message}"
    );
}

#[test]
fn a_global_named_tools_store_or_models_is_refused_and_leaves_the_namespace_in_place() {
    for name in ["tools", "store", "models"] {
        let vm = section_vm();
        let before: mlua::Table = vm.lua().globals().raw_get(name).expect("a raw read");
        let message = install_failure(&vm, &[prelude("kit", &format!("{name} = {{}}"))]);
        assert!(
            message.contains(&format!(
                "Plugin `kit`: its prelude defines the global `{name}`, \
                 which is reserved as an Engine global"
            )),
            "the collision names the Plugin, the global, and the reservation: {message}"
        );
        let after: mlua::Table = vm.lua().globals().raw_get(name).expect("a raw read");
        assert_eq!(
            after, before,
            "the refused prelude left `{name}` bound as it was"
        );
    }
}

#[test]
fn ui_and_item_collide_on_a_vm_that_binds_neither() {
    for name in ["ui", "item"] {
        let vm = section_vm();
        let bound: bool = eval(&vm, &format!("return {name} ~= nil"));
        assert!(!bound, "the fixture VM binds no `{name}`");
        let message = install_failure(&vm, &[prelude("kit", &format!("{name} = 1"))]);
        assert!(
            message.contains("Plugin `kit`")
                && message.contains(&format!("`{name}`"))
                && message.contains("reserved"),
            "the collision names the Plugin and the reserved global: {message}"
        );
    }
}

#[test]
fn argv_and_prose_collide_though_the_g_metatable_serves_them() {
    let vm = section_vm();
    vm.install_lazy_prose(|_| Ok("the block's prose".to_owned()))
        .expect("the prose guard installs");
    for name in ["argv", "prose"] {
        let raw: Value = vm.lua().globals().raw_get(name).expect("a raw read");
        assert!(
            raw.is_nil(),
            "`{name}` is not a raw global on the fixture VM"
        );
        let served: bool = eval(&vm, &format!("return {name} ~= nil"));
        assert!(served, "the `_G` metatable serves `{name}` to author code");
        let message = install_failure(&vm, &[prelude("kit", &format!("{name} = 1"))]);
        assert!(
            message.contains("Plugin `kit`")
                && message.contains(&format!("`{name}`"))
                && message.contains("reserved"),
            "the collision names the Plugin and the reserved global: {message}"
        );
    }
}

#[test]
fn a_prelude_sets_and_reads_metatables_as_the_base_functions_do() {
    let vm = section_vm();
    install_preludes(
        vm.lua(),
        &[prelude(
            "meta",
            "kit = {}\n\
             local shape = { __index = function(_, key) return key .. '!' end }\n\
             function kit.make() return setmetatable({}, shape) end\n\
             function kit.shaped(t) return getmetatable(t) == shape end\n\
             function kit.lock()\n\
               local t = setmetatable({}, { __metatable = 'locked' })\n\
               local ok, err = pcall(setmetatable, t, {})\n\
               return getmetatable(t), ok, tostring(err)\n\
             end",
        )],
    )
    .expect("the prelude installs");
    let (field, shaped): (String, bool) =
        eval(&vm, "local t = kit.make()\nreturn t.x, kit.shaped(t)");
    assert_eq!((field.as_str(), shaped), ("x!", true));
    let (label, ok, message): (String, bool, String) = eval(&vm, "return kit.lock()");
    assert_eq!(label, "locked");
    assert!(!ok, "a protected metatable refuses replacement");
    assert_eq!(message, "cannot change a protected metatable");
}

#[test]
fn two_preludes_defining_one_global_fail_naming_both_plugins() {
    let vm = section_vm();
    let message = install_failure(
        &vm,
        &[prelude("one", "shared = 1"), prelude("two", "shared = 2")],
    );
    assert!(
        message.contains("Plugin `two`")
            && message.contains("`shared`")
            && message.contains("Plugin `one`"),
        "the collision names the global and both Plugins: {message}"
    );
}

#[test]
fn a_global_whose_name_is_not_a_utf8_string_fails_naming_the_key() {
    for (source, key) in [
        ("_ENV[1] = true", "a key of type integer"),
        (
            "_ENV['\\xff'] = true",
            "a string key that is not valid UTF-8",
        ),
    ] {
        let vm = section_vm();
        let message = install_failure(&vm, &[prelude("kit", source)]);
        assert!(
            message.contains("Plugin `kit`")
                && message.contains(key)
                && message.contains("must be a UTF-8 string"),
            "the refusal names the Plugin and the key: {message}"
        );
    }
}

#[test]
fn a_prelude_that_raises_while_loading_fails_naming_its_plugin() {
    let vm = section_vm();
    let message = install_failure(&vm, &[prelude("boom", "local x = 1\nerror('boom')")]);
    assert!(
        message.starts_with("Plugin `boom`: its prelude failed to load: ")
            && message.contains("boom.")
            && message.contains(
                "A prelude only defines functions; it must not call tools while loading."
            ),
        "the failure names the Plugin and gives the rule: {message}"
    );
    assert!(
        traceback(&message).contains("plugin:boom:2:"),
        "the traceback names the prelude chunk and line: {message}"
    );
}

#[test]
fn a_prelude_that_calls_a_tool_while_loading_fails_naming_its_plugin() {
    let vm = section_vm();
    let message = install_failure(&vm, &[prelude("eager", "tools.call('eager/run')")]);
    assert!(
        message.starts_with("Plugin `eager`: its prelude failed to load: ")
            && message.contains("yield")
            && message.contains("it must not call tools while loading."),
        "the failure names the Plugin and gives the rule: {message}"
    );
    assert!(
        traceback(&message).contains("plugin:eager:1:"),
        "the traceback names the prelude chunk and line: {message}"
    );
}
