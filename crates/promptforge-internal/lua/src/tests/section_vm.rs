//! Section VM construction and phases: one environment across every
//! chunk, delayed single Engine injection, what a VM offers before it,
//! scalar returns, isolation between VMs, the control globals' target
//! checks, and the lifecycle reports around them.

use super::*;

#[test]
fn section_vm_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<SectionVm>();
}

#[test]
fn two_fresh_section_vms_yield_the_same_key_order() {
    // A section VM inherits Lua's per-state hash traversal, so two fresh VMs
    // can walk one table differently and neither walk is the shared order.
    // The deterministic installer in `SectionVm::new` pins both to the same
    // sorted sequence, so the two walks and the expected order all agree.
    let source = "local out = {} \
                  for k in pairs({zeta=1, alpha=2, mid=3, beta=4, omega=5}) \
                  do out[#out+1] = k end \
                  return out";
    let order = |section: &str| -> Vec<String> {
        let vm = TestVm::new().section(section).inject();
        let keys: Vec<String> = vm
            .lua()
            .load(source)
            .eval()
            .expect("pairs collects the string keys");
        vm.teardown(&null_emitter(), section);
        keys
    };
    let expected = vec!["alpha", "beta", "mid", "omega", "zeta"];
    assert_eq!(order("First"), expected);
    assert_eq!(order("Second"), expected);
}

#[test]
fn section_vm_preserves_one_environment_across_all_phases() {
    // The first chunk runs with every Engine global installed, so its top
    // level reads `args` and `store`; the functions it defines resolve the
    // same globals when later chunks call them.
    let first = program(
        "first_saw_args = args\n\
             first_saw_store = store.read('seed.txt')\n\
             function decorate(value) return '<' .. value .. '>' end",
    );
    let prologue = program(
        "var.decorated = decorate(args)\n\
             store.write('phase.txt', var.decorated)",
    );
    let between = program("phase_marker = 'model answer'");
    let epilog =
        program("return decorate(phase_marker) .. ':' .. first_saw_args .. ':' .. first_saw_store");
    let access = fresh_access();
    let store = store_view(&access);
    store
        .write("seed.txt", b"seeded")
        .expect("the memory store can seed a file");
    let sys = json!({ "id": 7 });
    let vm = TestVm::new()
        .args("input")
        .sys(&sys)
        .access(access)
        .inject();
    vm.install_engine_globals(&null_emitter(), "Test")
        .expect("Engine globals must install");
    run_scalar(&vm, &first, &null_emitter(), "Test")
        .expect("the first chunk must run with the full environment");

    assert_eq!(
        run_scalar(&vm, &prologue, &null_emitter(), "Test").expect("prologue must run"),
        None
    );
    assert_eq!(
        vm.var()
            .expect("var must serialize")
            .get("decorated")
            .and_then(Json::as_str),
        Some("<input>")
    );
    assert_eq!(
        store
            .read_string("phase.txt")
            .expect("shared store must read"),
        "<input>"
    );

    run_scalar(&vm, &between, &null_emitter(), "Test").expect("the between chunk must run");
    assert_eq!(
        run_scalar(&vm, &epilog, &null_emitter(), "Test")
            .expect("epilog must run")
            .as_deref(),
        Some("<model answer>:input:seeded")
    );
}

#[test]
fn a_refused_injection_hands_back_the_vm_for_its_teardown() {
    // A `var` seed that is not an object is refused, and the failure tears
    // the uninjected VM down under its section.
    let recorder = Recorder::default();
    let failure = SectionVm::new(&test_nonce(), recorder.emitter(), "Seed")
        .expect("VM must build")
        .inject_values_with_var(
            "",
            &json!({}),
            &fresh_access(),
            Some(&json!(5)),
            Argv::Frozen(None),
        )
        .expect_err("a non-object var seed is refused");
    assert!(
        matches!(failure.error(), Error::Internal(message) if message.contains("JSON object")),
        "the refusal names the seed's shape: {:?}",
        failure.error()
    );
    let error = failure.teardown(recorder.emitter(), "Seed");
    assert!(matches!(error, Error::Internal(_)));
    assert_eq!(
        recorder.observations(),
        [detail::LUA_TEARDOWN_STARTED, detail::LUA_TEARDOWN_SUCCEEDED]
            .into_iter()
            .map(|detail| ("Seed".to_owned(), detail.clone()))
            .collect::<Vec<_>>(),
        "the refused VM's teardown is observed"
    );
}

#[test]
fn untrusted_global_is_callable_before_engine_values_inject() {
    // Construction installs `untrusted`, ahead of every later setup step.
    let vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.uninjected_lua()
        .load(
            "local wrapped = untrusted('a < b')\n\
             assert(wrapped:find('a &lt; b', 1, true), 'the caller sees the escaped body')",
        )
        .exec()
        .expect("untrusted must be callable right after construction");
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn current_sys_returns_fallback_when_unset_and_errors_on_poison() {
    // An unset live slot is a legitimate state and yields the
    // fallback; a poisoned lock is a real failure and must NOT masquerade as
    // the fallback.
    let vm = SectionVm::new(&test_nonce(), &null_emitter(), "Section").expect("VM must build");
    let fallback = json!({ "id": 7 });
    let got = vm
        .current_sys(&fallback)
        .expect("an unset live slot yields the fallback");
    assert_eq!(got, fallback, "unset must return the fallback verbatim");

    // Poison the live mutex via a panicking guard, then a snapshot must be a
    // concrete error rather than a silent fallback.
    let handle = vm.sys_live_handle();
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = handle.lock().expect("first lock is not poisoned");
        panic!("poison the sys_live mutex");
    }));
    assert!(
        poisoned.is_err(),
        "the panic must unwind and poison the lock"
    );
    let error = vm
        .current_sys(&fallback)
        .expect_err("a poisoned live slot must surface a concrete error");
    assert!(
        error.to_string().contains("poisoned"),
        "the error must name the poison: {error}"
    );
}

#[test]
fn a_section_vm_reads_back_an_empty_var_after_injection() {
    let vm = TestVm::new().inject();
    assert_eq!(vm.var().expect("var must serialize"), json!({}));
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn section_vm_value_injection_bypasses_chunk_global_metatables() {
    // Engine values inject before any chunk runs, so a metatable a chunk
    // installs on `_G` intercepts none of them, and no tool ever installs
    // as a global after it.
    let intercept = program(
        "captured = {}\n\
             setmetatable(_G, { __newindex = function(_, key, value) captured[key] = value end })",
    );
    let inspect = program(
        "return tostring(captured.args) .. ',' .. tostring(captured.fixtures_search) .. ',' .. \
         args .. ',' .. type(fixtures_search) .. ',' .. tools.get('fixtures/search').id",
    );
    let bindings = fixture_set(&[("search", "search the web")]);
    let vm = SectionVm::new_for_section(
        &test_nonce(),
        &Arc::new(bindings),
        &Arc::new(Mutex::new(ModelSet::default())),
        &null_emitter(),
        "Test",
    )
    .expect("VM must build")
    .inject_values("private input", &json!({}), &fresh_access())
    .expect("Engine values must inject");
    let observer = null_emitter();
    vm.install_engine_globals(&observer, "Test")
        .expect("Engine globals must install");
    run_scalar(&vm, &intercept, &null_emitter(), "Test").expect("the intercepting chunk must run");

    assert_eq!(
        run_scalar(&vm, &inspect, &null_emitter(), "Test")
            .expect("inspection must run")
            .as_deref(),
        Some("nil,nil,private input,nil,fixtures/search")
    );
}

#[test]
fn section_vm_reports_store_operations_in_each_chunk() {
    let write = program("store.write('state.txt', args)");
    let read = program("return store.read('state.txt')");
    let recorder = Arc::new(Recorder::default());
    let vm = TestVm::new()
        .section("Gather")
        .args("private input")
        .inject();
    let observer = recorder.emitter().clone();
    vm.install_engine_globals(&observer, "Gather")
        .expect("Engine globals must install");

    run_scalar(&vm, &write, recorder.emitter(), "Gather").expect("first chunk write must run");
    run_scalar(&vm, &read, recorder.emitter(), "Gather").expect("second chunk read must run");
    vm.teardown(recorder.emitter(), "Gather");

    assert_eq!(
        recorder.observations(),
        vec![
            ("Gather".to_owned(), detail::LUA_CHUNK_STARTED.clone(),),
            ("Gather".to_owned(), detail::VFS_WRITE_SUCCEEDED.clone(),),
            ("Gather".to_owned(), detail::LUA_CHUNK_SUCCEEDED.clone(),),
            ("Gather".to_owned(), detail::LUA_CHUNK_STARTED.clone(),),
            ("Gather".to_owned(), detail::VFS_READ_SUCCEEDED.clone(),),
            ("Gather".to_owned(), detail::LUA_CHUNK_SUCCEEDED.clone(),),
            ("Gather".to_owned(), detail::LUA_TEARDOWN_STARTED.clone(),),
            ("Gather".to_owned(), detail::LUA_TEARDOWN_SUCCEEDED.clone(),),
        ]
    );
    let trace = format!("{:?}", recorder.observations());
    assert!(!trace.contains("private input"));
    assert!(!trace.contains("state.txt"));
}

#[test]
fn section_vm_accepts_only_scalar_top_level_returns() {
    for (source, expected) in [
        ("return 'text'", Some("text")),
        ("return 42", Some("42")),
        ("return 1.5", Some("1.5")),
        ("return true", Some("true")),
        ("return nil", None),
    ] {
        let vm = TestVm::new().build();
        assert_eq!(
            run_scalar(&vm, &program(source), &null_emitter(), "Test")
                .expect("scalar return must work")
                .as_deref(),
            expected
        );
    }

    let vm = TestVm::new().build();
    let error = run_scalar(&vm, &program("return {}"), &null_emitter(), "Test")
        .expect_err("table returns must be refused");
    assert!(error.to_string().contains("cannot return a table"));
}

#[test]
fn section_vms_isolate_mutated_globals() {
    let seed = program("counter = 0");
    let increment = program("counter = counter + 1; return counter");
    let access = fresh_access();
    let first = TestVm::new().access(Arc::clone(&access)).build();
    run_scalar(&first, &seed, &null_emitter(), "First").expect("first VM must seed");
    let second = TestVm::new().access(access).build();
    run_scalar(&second, &seed, &null_emitter(), "Second").expect("second VM must seed");

    assert_eq!(
        run_scalar(&first, &increment, &null_emitter(), "First")
            .expect("first increment must run")
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        run_scalar(&first, &increment, &null_emitter(), "First")
            .expect("second first-VM increment must run")
            .as_deref(),
        Some("2")
    );
    assert_eq!(
        run_scalar(&second, &increment, &null_emitter(), "Second")
            .expect("second VM increment must run")
            .as_deref(),
        Some("1")
    );
}

#[test]
fn section_lifecycle_reports_are_ordered_exact_and_payload_free() {
    let first = program("private_global = 'chunk secret'");
    let prologue = program("var.value = args");
    let epilog = program("return 'epilog done'");
    let recorder = Arc::new(Recorder::default());
    let vm = SectionVm::new(&test_nonce(), recorder.emitter(), "Gather")
        .expect("VM must build")
        .inject_values("private input", &json!({}), &fresh_access())
        .expect("Engine values must inject");
    let observer = recorder.emitter().clone();
    vm.install_engine_globals(&observer, "Gather")
        .expect("Engine globals must install");
    run_scalar(&vm, &first, recorder.emitter(), "Gather").expect("the first chunk must run");
    run_scalar(&vm, &prologue, recorder.emitter(), "Gather").expect("prologue must run");
    run_scalar(&vm, &epilog, recorder.emitter(), "Gather").expect("epilog must run");
    vm.teardown(recorder.emitter(), "Gather");

    let observations = recorder.observations();
    assert_eq!(
        observations,
        [
            detail::LUA_CHUNK_STARTED,
            detail::LUA_CHUNK_SUCCEEDED,
            detail::LUA_CHUNK_STARTED,
            detail::LUA_CHUNK_SUCCEEDED,
            detail::LUA_CHUNK_STARTED,
            detail::LUA_CHUNK_SUCCEEDED,
            detail::LUA_TEARDOWN_STARTED,
            detail::LUA_TEARDOWN_SUCCEEDED,
        ]
        .into_iter()
        .map(|detail| ("Gather".to_owned(), detail.clone()))
        .collect::<Vec<_>>()
    );
    let trace = format!("{observations:?}");
    assert!(!trace.contains("chunk secret"));
    assert!(!trace.contains("private input"));
}

#[test]
fn section_lifecycle_failures_report_their_phase() {
    let recorder = Arc::new(Recorder::default());
    let failing = program("error('private chunk failure')");
    let vm = TestVm::new()
        .emitter(recorder.emitter())
        .section("Failing")
        .build();
    run_scalar(&vm, &failing, recorder.emitter(), "Failing")
        .expect_err("the failing chunk must fail");
    vm.teardown();
    let observations = recorder.observations();
    assert_eq!(
        observations,
        [
            detail::LUA_CHUNK_STARTED,
            detail::LUA_CHUNK_FAILED,
            detail::LUA_TEARDOWN_STARTED,
            detail::LUA_TEARDOWN_SUCCEEDED,
        ]
        .into_iter()
        .map(|detail| ("Failing".to_owned(), detail.clone()))
        .collect::<Vec<_>>()
    );
    assert!(!format!("{observations:?}").contains("private chunk failure"));
}

#[test]
fn call_with_a_non_string_target_errors() {
    // The control callback resolves its target through the same
    // `resolve_section_target` boundary as the Engine: a number is not a
    // heading, and the error says so.
    let vm = TestVm::new().build();
    vm.install_control_globals(
        |target, _, _| resolve_section_target(target).map_err(Error::lua),
        |_| Err(Error::Lua("prompt reads are not needed here".to_owned())),
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
    vm.teardown();
}

#[test]
fn a_chunk_sees_the_tools_table_and_no_tool_global_ever_installs() {
    // The `tools`/`models` tables install with Engine injection, before any
    // chunk runs, so top-level code may scope tools. No tool becomes a
    // global, during the chunk or after it.
    let bindings = fixture_set(&[("search", "search the web")]);
    let scope = program(
        "tools.offer(tools.get('fixtures/search'))\n\
         assert(fixtures_search == nil, 'no tool installs as a global')",
    );
    let vm = TestVm::new().tools(bindings).build();
    run_scalar(&vm, &scope, &null_emitter(), "Test").expect("the tools table must work");

    assert_eq!(
        run_scalar(
            &vm,
            &program("return type(fixtures_search)"),
            &null_emitter(),
            "Test"
        )
        .expect("the probe runs after the scoping chunk")
        .as_deref(),
        Some("nil")
    );
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");
    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_search"],
        "the top-level tools.offer must be recorded"
    );
}

#[test]
fn functions_resolve_engine_globals_when_called_from_a_later_chunk() {
    // A function body resolves `tools`/`var` through the real globals at
    // call time, so a later chunk can mutate Engine state through it.
    let bindings = fixture_set(&[("search", "search the web")]);
    let define = program(
        "function scope_and_store(id)\n\
             tools.offer(id)\n\
             var.scoped = id\n\
             return var.scoped\n\
         end",
    );
    let vm = TestVm::new().tools(bindings).build();
    run_scalar(&vm, &define, &null_emitter(), "Test").expect("the defining chunk must run");

    assert_eq!(
        run_scalar(
            &vm,
            &program("return scope_and_store('fixtures/search')"),
            &null_emitter(),
            "Test",
        )
        .expect("the function must mutate Engine state when called")
        .as_deref(),
        Some("fixtures/search")
    );
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");
    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_search"]
    );
}
