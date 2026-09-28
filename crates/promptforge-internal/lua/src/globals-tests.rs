//! Tests for the `_G` guard: `argv` and `prose` stay guarded whatever author
//! code does to `_G`'s metatable, the author's metatable composes behind the
//! guard, and `setmetatable` and `getmetatable` behave as the base
//! functions for every other value.

use std::sync::Arc;

use mlua::{FromLuaMulti, Lua, LuaOptions, StdLib};
use promptforge_types::untrusted::GuardNonce;
use serde_json::json;

use super::{GLOBALS_CHUNK_NAME, GLOBALS_SOURCE};
use crate::tests::recording::null_emitter;
use crate::tests::{assert_chunk_name_resolves, refusal_line};
use crate::{Argv, SectionVm};

const SECTION: &str = "Globals";

/// The refusal line of an assignment to the frozen `argv` global.
const ARGV_REFUSAL: &str = "argv is frozen outside H1: assign it in H1 only";

/// The refusal line of an assignment to `prose`.
const PROSE_REFUSAL: &str = "prose is read-only: assign to `var` or a section global instead";

/// The text every test VM's `prose` renders.
const PROSE: &str = "the block's prose";

/// Builds a section VM the way section setup does up to the shared replay:
/// frozen `argv`, the host APIs, the control globals, and the coroutine
/// shims, with a block's lazy `prose` installed.
fn frozen_vm() -> SectionVm {
    let argv = json!({ "query": "papers" });
    section_vm(Argv::Frozen(Some(&argv)))
}

/// [`frozen_vm`] with `argv` installed in the given mode.
fn section_vm(argv: Argv<'_>) -> SectionVm {
    let emitter = null_emitter();
    let mut vm =
        SectionVm::new(&GuardNonce::from_seed(11), &emitter, SECTION).expect("the VM builds");
    let access = Arc::new(
        promptforge_vfs::VfsRef::default()
            .acquire(promptforge_vfs::Origin::new("globals test fixture"))
            .expect("the stock backend acquires"),
    );
    vm.inject_host_with_var("", &json!({ "id": 1 }), &access, None, argv)
        .expect("host values inject");
    vm.install_host_apis(&emitter, SECTION)
        .expect("the host APIs install");
    vm.install_scheduler_control_globals(|_| {
        Ok::<Vec<String>, std::convert::Infallible>(Vec::new())
    })
    .expect("the control globals install");
    vm.install_coro_shims(1)
        .expect("the coroutine shims install");
    vm.install_lazy_prose(|_| Ok(PROSE.to_owned()))
        .expect("the prose guard installs");
    vm
}

/// Runs author code on the VM's main state under the chunk name `probe`.
fn eval<T: FromLuaMulti>(vm: &SectionVm, source: &str) -> T {
    vm.lua()
        .load(source)
        .set_name("probe")
        .eval()
        .expect("the author chunk runs")
}

/// The caught refusals of assigning `argv` and `prose`, each reduced to its
/// first line.
fn assignment_refusals(vm: &SectionVm) -> (String, String) {
    let (argv, prose): (String, String) = eval(
        vm,
        "local _, argv_err = pcall(function() argv = 'hijacked' end)\n\
         local _, prose_err = pcall(function() prose = 'overwritten' end)\n\
         return tostring(argv_err), tostring(prose_err)",
    );
    (
        refusal_line(&argv).unwrap_or(&argv).to_owned(),
        refusal_line(&prose).unwrap_or(&prose).to_owned(),
    )
}

/// Asserts `argv` and `prose` still read as the host set them and still
/// refuse assignment with their unchanged refusals.
fn assert_guards_hold(vm: &SectionVm, after: &str) {
    let (query, prose): (String, String) = eval(vm, "return argv.query, prose");
    assert_eq!(
        (query.as_str(), prose.as_str()),
        ("papers", PROSE),
        "argv and prose read as the host set them after {after}"
    );
    let (argv_refusal, prose_refusal) = assignment_refusals(vm);
    assert_eq!(
        argv_refusal, ARGV_REFUSAL,
        "argv stays frozen after {after}"
    );
    assert_eq!(
        prose_refusal, PROSE_REFUSAL,
        "prose stays read-only after {after}"
    );
}

/// Runs `source` on a stock Lua VM with the section VM's standard libraries
/// under the same chunk name, the base functions untouched.
fn stock_eval(source: &str) -> String {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH,
        LuaOptions::default(),
    )
    .expect("a stock VM builds");
    lua.load(source)
        .set_name("probe")
        .eval()
        .expect("the stock chunk runs")
}

#[test]
fn the_globals_chunk_name_resolves_to_the_guard_file() {
    assert_chunk_name_resolves("GLOBALS_CHUNK_NAME", GLOBALS_CHUNK_NAME, GLOBALS_SOURCE);
}

#[test]
fn no_setmetatable_call_on_g_frees_argv_or_prose() {
    for attempt in [
        "setmetatable(_G, nil)",
        "setmetatable(_G, {})",
        "captured = {}\n\
         setmetatable(_G, { __newindex = function(t, k, v) captured[k] = v end })",
        "setmetatable(_G, { __index = function() return 'author' end, \
         __newindex = function() end })",
    ] {
        let vm = frozen_vm();
        eval::<()>(&vm, attempt);
        assert_guards_hold(&vm, attempt);
        let captured: bool = eval(
            &vm,
            "return captured == nil or (captured.argv == nil and captured.prose == nil)",
        );
        assert!(
            captured,
            "the author's __newindex never sees argv or prose after {attempt}"
        );
    }
}

#[test]
fn the_h1_argv_is_a_plain_global_that_no_author_metatable_intercepts() {
    let vm = section_vm(Argv::Writable(None));
    let (absent, hooked, query): (bool, bool, String) = eval(
        &vm,
        "seen = {}\n\
         setmetatable(_G, {\n\
           __index = function(_, key) error('undefined global ' .. key, 2) end,\n\
           __newindex = function(_, key, value) seen[key] = value end,\n\
         })\n\
         local absent = argv == nil\n\
         argv = { query = 'repaired' }\n\
         return absent, seen.argv ~= nil, argv.query",
    );
    assert!(
        absent,
        "a nil H1 argv reads nil, not the strict handler's error"
    );
    assert!(!hooked, "the author's write hook never sees argv");
    assert_eq!(query, "repaired", "the repair lands in _G");
    assert_eq!(
        vm.argv_json().expect("the repair reads back"),
        Some(json!({ "query": "repaired" }))
    );
    let (prose_refusal, cleared): (String, bool) = eval(
        &vm,
        "local _, err = pcall(function() prose = 'x' end)\n\
         argv = nil\n\
         return tostring(err), argv == nil",
    );
    assert_eq!(refusal_line(&prose_refusal), Some(PROSE_REFUSAL));
    assert!(cleared, "H1 can still clear argv");
}

#[test]
fn editing_the_table_getmetatable_returns_cannot_free_argv_or_prose() {
    let vm = frozen_vm();
    let before: bool = eval(&vm, "return getmetatable(_G) == nil");
    assert!(before, "with no author metatable, getmetatable(_G) is nil");
    eval::<()>(
        &vm,
        "setmetatable(_G, {})\n\
         local mt = getmetatable(_G)\n\
         mt.__index = function() return 'author' end\n\
         mt.__newindex = function() end\n\
         mt.__metatable = nil",
    );
    assert_guards_hold(&vm, "editing the author metatable");
}

#[test]
fn no_sandbox_route_reaches_the_guard_or_a_raw_global() {
    let vm = frozen_vm();
    let reachable: String = eval(
        &vm,
        "local found = {}\n\
         for _, name in ipairs({ 'rawget', 'rawset', 'rawequal', 'rawlen', 'debug', 'load', \
         'loadstring', 'dofile', 'loadfile', 'require', 'package', 'getfenv', 'setfenv', \
         'collectgarbage', 'coroutine', 'io', 'os' }) do\n\
           if _G[name] ~= nil then found[#found + 1] = name end\n\
         end\n\
         return table.concat(found, ',')",
    );
    assert_eq!(
        reachable, "",
        "no raw access, debug, or code-loading global"
    );
}

#[test]
fn getmetatable_on_g_returns_the_author_metatable_and_edits_take_effect_live() {
    let vm = frozen_vm();
    let (same, untouched, returned): (bool, bool, bool) = eval(
        &vm,
        "author = {}\n\
         local returned = setmetatable(_G, author) == _G\n\
         return getmetatable(_G) == author, next(author) == nil, returned",
    );
    assert!(same, "getmetatable(_G) is the author's own table");
    assert!(
        untouched,
        "setting the author's table writes nothing into it"
    );
    assert!(returned, "setmetatable(_G, mt) returns _G");

    let live: String = eval(
        &vm,
        "getmetatable(_G).__index = function(_, key) return 'default ' .. key end\n\
         return missing_one",
    );
    assert_eq!(
        live, "default missing_one",
        "a later __index edit applies at once"
    );
    let (hooked, raw): (String, bool) = eval(
        &vm,
        "local seen = {}\n\
         author.__newindex = function(_, key, value) seen[key] = value end\n\
         hooked_global = 'x'\n\
         author.__index = nil\n\
         return seen.hooked_global, hooked_global == nil",
    );
    assert_eq!(hooked, "x", "a later __newindex edit applies at once");
    assert!(raw, "the hooked write never landed in _G");
    assert_guards_hold(&vm, "live edits to the author metatable");

    let cleared: (bool, String) = eval(
        &vm,
        "setmetatable(_G, nil)\n\
         plain_global = 'raw'\n\
         return getmetatable(_G) == nil, plain_global",
    );
    assert_eq!(
        cleared,
        (true, "raw".to_owned()),
        "setmetatable(_G, nil) clears the author metatable"
    );
    assert_guards_hold(&vm, "clearing the author metatable");
}

#[test]
fn an_author_metatable_with_a_metatable_field_protects_g_as_lua_would() {
    let vm = frozen_vm();
    let (label, tone, replace, clear): (String, String, String, String) = eval(
        &vm,
        "setmetatable(_G, { __metatable = 'locked', __index = { tone = 'friendly' } })\n\
         local _, replace = pcall(function() setmetatable(_G, {}) end)\n\
         local _, clear = pcall(function() setmetatable(_G, nil) end)\n\
         return getmetatable(_G), tone, tostring(replace), tostring(clear)",
    );
    assert_eq!(
        label, "locked",
        "getmetatable(_G) returns the author's __metatable"
    );
    assert_eq!(tone, "friendly", "the protected metatable keeps working");
    assert_eq!(
        (replace.as_str(), clear.as_str()),
        (
            "[string \"probe\"]:2: cannot change a protected metatable",
            "[string \"probe\"]:3: cannot change a protected metatable",
        ),
        "a later setmetatable(_G) raises Lua's own refusal at the caller's line"
    );
    assert_guards_hold(&vm, "a protected author metatable");
}

#[test]
fn setmetatable_on_g_rejects_what_the_base_function_rejects() {
    let vm = frozen_vm();
    let (number, missing, boolean): (String, String, String) = eval(
        &vm,
        "local _, number = pcall(setmetatable, _G, 5)\n\
         local _, missing = pcall(setmetatable, _G)\n\
         local _, boolean = pcall(function() setmetatable(_G, false) end)\n\
         return tostring(number), tostring(missing), tostring(boolean)",
    );
    assert_eq!(
        number,
        "bad argument #2 to 'setmetatable' (nil or table expected, got number)"
    );
    assert_eq!(
        missing,
        "bad argument #2 to 'setmetatable' (nil or table expected, got no value)"
    );
    assert_eq!(
        boolean,
        "[string \"probe\"]:3: bad argument #2 to 'setmetatable' (nil or table expected, got boolean)"
    );
    let untouched: bool = eval(&vm, "return getmetatable(_G) == nil");
    assert!(untouched, "a rejected call records nothing");
}

#[test]
fn the_other_fields_of_the_author_metatable_apply_to_g_from_each_setmetatable() {
    let vm = frozen_vm();
    let (called, shown): (String, String) = eval(
        &vm,
        "author = {\n\
           __call = function(_, name) return 'called ' .. name end,\n\
           __tostring = function() return 'globals' end,\n\
         }\n\
         setmetatable(_G, author)\n\
         return _G('twice'), tostring(_G)",
    );
    assert_eq!(called, "called twice");
    assert_eq!(shown, "globals");
    let (before, after): (String, String) = eval(
        &vm,
        "author.__tostring = function() return 'edited' end\n\
         local before = tostring(_G)\n\
         setmetatable(_G, author)\n\
         return before, tostring(_G)",
    );
    assert_eq!(
        (before.as_str(), after.as_str()),
        ("globals", "edited"),
        "other fields are copied when setmetatable(_G, mt) runs"
    );
    let (plain, callable): (bool, bool) = eval(
        &vm,
        "setmetatable(_G, nil)\n\
         return tostring(_G):find('^table: ') ~= nil, (pcall(_G))",
    );
    assert!(plain, "clearing drops the copied __tostring");
    assert!(!callable, "clearing drops the copied __call");
    assert_guards_hold(&vm, "forwarded metamethods");
}

#[test]
fn setmetatable_and_getmetatable_on_other_values_match_stock_lua() {
    let probe = "\
local out = {}
local function case(f)
  local results = table.pack(pcall(f))
  for i = 1, results.n do results[i] = tostring(results[i]) end
  out[#out + 1] = table.concat(results, ' ', 1, results.n)
end
case(function()
  local t, mt = {}, {}
  return setmetatable(t, mt) == t, getmetatable(t) == mt
end)
case(function() return getmetatable(setmetatable({}, { __metatable = 'sealed' })) end)
case(function() return getmetatable(setmetatable({}, { __metatable = false })) end)
case(function()
  local t = setmetatable({}, { __metatable = 'sealed' })
  setmetatable(t, {})
end)
case(function()
  local t = setmetatable({}, { __metatable = 'sealed' })
  setmetatable(t, nil)
end)
case(function() setmetatable(1, {}) end)
case(function() setmetatable({}, 5) end)
case(function() setmetatable({}) end)
case(function() setmetatable() end)
case(function() getmetatable() end)
case(function() return getmetatable('').__index == string end)
case(function() return getmetatable(1) end)
case(function() return select('#', setmetatable({}, nil)), select('#', getmetatable({})) end)
case(function() return setmetatable({}, { __index = function(_, k) return k .. '!' end }).x end)
out[#out + 1] = tostring(select(2, pcall(setmetatable, 1, {})))
out[#out + 1] = tostring(select(2, pcall(setmetatable, setmetatable({}, { __metatable = 1 }), {})))
out[#out + 1] = tostring(select(2, pcall(getmetatable)))
return table.concat(out, '\\n')";
    let vm = frozen_vm();
    let guarded: String = eval(&vm, probe);
    assert_eq!(guarded, stock_eval(probe));
}

#[test]
fn sealed_host_values_keep_their_metatable_protection() {
    let vm = frozen_vm();
    let (sys_label, argv_label, sys_refusal, var_refusal): (String, String, String, String) = eval(
        &vm,
        "local _, sys_err = pcall(function() setmetatable(sys, {}) end)\n\
         local _, var_err = pcall(function() setmetatable(var, {}) end)\n\
         return getmetatable(sys), getmetatable(argv), tostring(sys_err), tostring(var_err)",
    );
    assert_eq!(sys_label, "sys is sealed");
    assert_eq!(argv_label, "argv is frozen");
    assert_eq!(
        sys_refusal,
        "[string \"probe\"]:1: cannot change a protected metatable"
    );
    assert_eq!(
        var_refusal,
        "[string \"probe\"]:2: cannot change a protected metatable"
    );
}
