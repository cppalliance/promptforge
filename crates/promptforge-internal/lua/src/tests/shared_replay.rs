//! The shared-library replay and the control globals: the `jump` and
//! `call` refusals, what load-time code sees, shared functions called from
//! later chunks, and the empty replay.

use super::*;

#[test]
fn jump_during_shared_replay_is_a_hard_error() {
    // Load-time control transfer has no section walk to transfer into, so a
    // recorded jump fails the replay outright.
    let shared = program("jump('## Anywhere')");
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host values must inject");
    let observer = null_emitter();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.install_control_globals(
        |_, _, _| Err(Error::Lua("call is not needed here".to_owned())),
        |_| {
            Err(Error::Lua(
                "list_from_section is not needed here".to_owned(),
            ))
        },
    )
    .expect("control globals must install");
    let error = vm
        .replay_shared(&shared, &null_emitter(), "Test")
        .expect_err("jump during the shared replay must fail");
    assert!(
        error
            .to_string()
            .contains("jump is not available during shared library load"),
        "the hard error must name the phase: {error}"
    );
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn call_with_a_non_string_target_errors() {
    // The control callback resolves its target through the same
    // `resolve_section_target` boundary as the engine: a number is not a
    // heading, and the error says so.
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host values must inject");
    let observer = null_emitter();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.install_control_globals(
        |target, _, _| resolve_section_target(target).map_err(Error::lua),
        |_| {
            Err(Error::Lua(
                "list_from_section is not needed here".to_owned(),
            ))
        },
    )
    .expect("control globals must install");
    let out = run_scalar(
        &vm,
        &program(
            "local ok, err = pcall(call, 42)\n\
             assert(not ok and tostring(err):find('section target must be a string'), tostring(err))\n\
             return 'ok'",
        ),
        &null_emitter(),
        "Test",
    )
    .expect("a non-string call target must error");
    assert_eq!(out.as_deref(), Some("ok"));
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn shared_replay_sees_the_tables_but_not_the_bare_alias_globals() {
    // The `tools`/`models` tables install with host injection, before the
    // replay, so shared top-level code may scope tools at load. The bare
    // alias globals install only after the replay, so a declared alias wins
    // over a same-named shared global.
    let bindings = ToolSet::for_test(
        vec![ToolBinding::for_test(
            "search",
            "search the web",
            &fixture_tool("search"),
        )],
        Vec::new(),
    );
    let shared = program(
        "tools.add('search')\n\
         assert(search == nil, 'the bare alias global installs after the replay')",
    );
    let mut vm = SectionVm::new_for_section(
        &test_nonce(),
        &shared_set(bindings),
        &Arc::new(Mutex::new(ModelSet::default())),
        &null_emitter(),
        "Test",
    )
    .expect("VM must build");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host values must inject");
    let observer = null_emitter();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.replay_shared(&shared, &null_emitter(), "Test")
        .expect("the tools table must work during the shared replay");
    vm.install_captured_bindings()
        .expect("captured bindings must install");

    assert_eq!(
        run_scalar(
            &vm,
            &program("return type(search)"),
            &null_emitter(),
            "Test"
        )
        .expect("the alias global installs after the replay")
        .as_deref(),
        Some("userdata")
    );
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");
    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["search"],
        "the load-time tools.add must be recorded"
    );
}

#[test]
fn shared_functions_resolve_host_globals_when_called_from_a_later_chunk() {
    // A shared function body resolves `tools`/`var` through the real globals
    // at call time, so a later chunk can drive host mutations through it.
    let bindings = ToolSet::for_test(
        vec![ToolBinding::for_test(
            "search",
            "search the web",
            &fixture_tool("search"),
        )],
        Vec::new(),
    );
    let shared = program(
        "function scope_and_store(alias)\n\
             tools.add(alias)\n\
             var.scoped = alias\n\
             return var.scoped\n\
         end",
    );
    let mut vm = SectionVm::new_for_section(
        &test_nonce(),
        &shared_set(bindings),
        &Arc::new(Mutex::new(ModelSet::default())),
        &null_emitter(),
        "Test",
    )
    .expect("VM must build");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host values must inject");
    let observer = null_emitter();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.replay_shared(&shared, &null_emitter(), "Test")
        .expect("shared library must load");
    vm.install_captured_bindings()
        .expect("captured bindings must install");

    assert_eq!(
        run_scalar(
            &vm,
            &program("return scope_and_store('search')"),
            &null_emitter(),
            "Test",
        )
        .expect("the shared function must mutate host state when called")
        .as_deref(),
        Some("search")
    );
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");
    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["search"]
    );
}

#[test]
fn absent_shared_library_replays_an_empty_chunk_on_the_same_path() {
    // No `lua shared` fence: startup still replays, with an empty compiled
    // chunk, and reports the same load boundary.
    let recorder = Arc::new(Recorder::default());
    let mut vm = SectionVm::new(&test_nonce(), recorder.emitter(), "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host values must inject");
    let observer = recorder.emitter().clone();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.replay_shared(
        &LuaProgram::empty().expect("the empty chunk compiles"),
        recorder.emitter(),
        "Test",
    )
    .expect("the empty replay must succeed");
    assert_eq!(
        run_scalar(&vm, &program("return 42"), recorder.emitter(), "Test")
            .expect("a chunk runs after the empty replay")
            .as_deref(),
        Some("42")
    );
    assert_eq!(
        recorder.observations(),
        [
            detail::LUA_SHARED_LOAD_STARTED,
            detail::LUA_SHARED_LOAD_SUCCEEDED,
            detail::LUA_CHUNK_STARTED,
            detail::LUA_CHUNK_SUCCEEDED,
        ]
        .into_iter()
        .map(|detail| ("Test".to_owned(), detail.clone()))
        .collect::<Vec<_>>()
    );
}
