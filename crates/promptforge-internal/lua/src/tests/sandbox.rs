//! The sandboxed global environment: no direct output, the safe standard
//! library without the dangerous globals, `setmetatable`'s refusals, and
//! the `untrusted` global.

use super::*;

#[test]
fn direct_output_is_absent_in_every_executable_lua_vm() {
    let library = program("assert(print == nil); assert(warn == nil); log('library load')");
    let library_vm =
        section_vm_with_shared(&library, "", &fresh_access(), &null_emitter(), "Section")
            .expect("library VM must not expose direct output");
    library_vm.teardown(&null_emitter(), "Section");

    let bindings = fixture_set(&[("search", "search the web")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("section VM must not expose direct output");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    run_scalar(
        &vm,
        &program("assert(print == nil); assert(warn == nil)"),
        &null_emitter(),
        "Section",
    )
    .expect("prologue must not expose direct output");
    run_scalar(
        &vm,
        &program("assert(print == nil); assert(warn == nil)"),
        &null_emitter(),
        "Section",
    )
    .expect("epilog must not expose direct output");
    vm.teardown(&null_emitter(), "Section");

    assert_eq!(
        run("return tostring(print) .. ':' .. tostring(warn)", "")
            .expect("compatibility VM must run")
            .returned
            .as_deref(),
        Some("nil:nil")
    );
}

#[test]
fn safe_stdlib_present() {
    let out = run("return string.upper(args)", "hi").unwrap();
    assert_eq!(out.returned.as_deref(), Some("HI"));
}

#[test]
fn dangerous_globals_absent() {
    let out = run(
            "return tostring(io) .. ',' .. tostring(os) .. ',' .. tostring(require) .. ',' .. tostring(load)",
            "",
        )
        .unwrap();
    assert_eq!(out.returned.as_deref(), Some("nil,nil,nil,nil"));
}

#[test]
fn setmetatable_refuses_finalizers_and_weak_globals_but_keeps_g_handlers() {
    let step = shim_vm(None)
        .start_block_coro(&program(
            "local _, gc = pcall(function() setmetatable({}, { __gc = function() end }) end)\n\
             local _, mode = pcall(function() setmetatable(_G, { __mode = 'v' }) end)\n\
             setmetatable(_G, { __index = function(_, key) return 'fallback ' .. key end })\n\
             return tostring(gc) .. '|' .. tostring(mode) .. '|' .. missing_name",
        ))
        .expect("the refusals are caught and the handler installs");
    let CoroStep::Done(LuaBlockResult::Returned(Some(returned))) = step else {
        panic!("the block must return a string, got {step:?}");
    };
    let parts: Vec<&str> = returned.split('|').collect();
    let [gc, mode, fallback] = parts.as_slice() else {
        panic!("expected three parts, got {returned}");
    };
    assert!(
        gc.ends_with("setmetatable: finalizers (__gc) are not available in the sandbox"),
        "a __gc metatable must be refused: {gc}"
    );
    assert!(
        mode.ends_with("setmetatable: weak tables (__mode) are not available for _G"),
        "a weak _G must be refused: {mode}"
    );
    assert_eq!(*fallback, "fallback missing_name");
}

#[test]
fn untrusted_global_escapes_and_envelopes_any_string() {
    let outcome = run("return untrusted('a < b')", "").expect("untrusted must run");
    let wrapped = outcome.returned.expect("untrusted returns a string");
    assert!(
        wrapped.starts_with("The text inside the untrusted_input_"),
        "the envelope opens with the preface, got:\n{wrapped}"
    );
    assert!(
        wrapped.contains("\na &lt; b\n"),
        "every literal '<' is escaped in the body, got:\n{wrapped}"
    );
    assert_eq!(
        wrapped.matches("<untrusted_input_").count(),
        1,
        "exactly one live open tag, got:\n{wrapped}"
    );
    assert_eq!(
        wrapped.matches("</untrusted_input_").count(),
        1,
        "exactly one live close tag, got:\n{wrapped}"
    );
}

#[test]
fn untrusted_global_wraps_every_call_under_the_run_nonce() {
    // One nonce per run: both calls from the same VM wrap under the same
    // nonce, so identical content produces a byte-identical envelope.
    let outcome = run(
        "return untrusted('same') .. '\\n@@SPLIT@@\\n' .. untrusted('same')",
        "",
    )
    .expect("untrusted must run");
    let wrapped = outcome.returned.expect("two envelopes");
    let (first, second) = wrapped.split_once("\n@@SPLIT@@\n").expect("two envelopes");
    assert_eq!(first, second, "every call in a run shares the run nonce");
}

#[test]
fn untrusted_global_is_callable_from_the_shared_library() {
    let shared = program(
        "local wrapped = untrusted('a < b')\n\
         assert(wrapped:find('a &lt; b', 1, true), 'shared sees the escaped body')",
    );
    let vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.replay_shared(&shared, &null_emitter(), "Test")
        .expect("the shared library must call untrusted during load");
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn untrusted_global_rejects_a_non_string_argument() {
    let error = run("return untrusted({})", "").expect_err("a table is not a string");
    assert!(
        matches!(error, Error::Lua(_) | Error::LuaRuntime { .. }),
        "a non-string argument must surface as a Lua error, got {error:?}"
    );
}
