//! Section VM construction and phases: one environment across the shared
//! replay and every chunk, delayed single Engine injection, scalar returns,
//! isolation between VMs, and the lifecycle reports around them.

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
        let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), section)
            .expect("section VM construction cannot fail");
        vm.inject_values("", &json!({}), &fresh_access())
            .expect("Engine values must inject");
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
    // The shared library replays as the section's first chunk with every
    // Engine global installed, so its top level reads `args` and `store`
    // at load; the functions it defines resolve the same globals when later
    // chunks call them.
    let shared = program(
        "shared_saw_args = args\n\
             shared_saw_store = store.read('seed.txt')\n\
             function decorate(value) return '<' .. value .. '>' end",
    );
    let prologue = program(
        "var.from_shared = decorate(args)\n\
             store.write('phase.txt', var.from_shared)",
    );
    let between = program("phase_marker = 'model answer'");
    let epilog = program(
        "return decorate(phase_marker) .. ':' .. shared_saw_args .. ':' .. shared_saw_store",
    );
    let access = fresh_access();
    let store = store_view(&access);
    store
        .write("seed.txt", b"seeded")
        .expect("the memory store can seed a file");
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_values("input", &json!({ "id": 7 }), &access)
        .expect("Engine values must inject");
    vm.install_engine_globals(&null_emitter(), "Test")
        .expect("Engine globals must install");
    vm.replay_shared(&shared, &null_emitter(), "Test")
        .expect("shared program must run with the full environment");

    assert_eq!(
        run_scalar(&vm, &prologue, &null_emitter(), "Test").expect("prologue must run"),
        None
    );
    assert_eq!(
        vm.var()
            .expect("var must serialize")
            .get("from_shared")
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
fn section_vm_requires_delayed_single_value_injection() {
    let no_op = program("return args");
    let access = fresh_access();
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");

    let error = run_scalar(&vm, &no_op, &null_emitter(), "Test")
        .expect_err("programs cannot run before value injection");
    assert!(error.to_string().contains("not been injected"));

    vm.inject_values("first", &json!({}), &access)
        .expect("first injection must succeed");
    let error = vm
        .inject_values("second", &json!({}), &access)
        .expect_err("Engine values cannot be replaced");
    assert!(error.to_string().contains("already injected"));
}

#[test]
fn section_vm_value_injection_bypasses_shared_global_metatables() {
    // Engine values inject before the shared replay, and the captured alias
    // globals raw-set after it, so a metatable the shared library installs on
    // `_G` intercepts neither.
    let shared = program(
        "captured = {}\n\
             setmetatable(_G, { __newindex = function(_, key, value) captured[key] = value end })",
    );
    let inspect = program(
        "return tostring(captured.args) .. ',' .. tostring(captured.search) .. ',' .. args .. ',' .. type(search)",
    );
    let bindings = ToolSet::for_test(
        vec![ToolBinding::for_test(
            "search",
            "search the web",
            &fixture_tool("search"),
        )],
        Vec::new(),
    );
    let mut vm = SectionVm::new_for_section(
        &test_nonce(),
        &shared_set(bindings),
        &Arc::new(Mutex::new(ModelSet::default())),
        &null_emitter(),
        "Test",
    )
    .expect("VM must build");
    vm.inject_values("private input", &json!({}), &fresh_access())
        .expect("Engine values must inject");
    let observer = null_emitter();
    vm.install_engine_globals(&observer, "Test")
        .expect("Engine globals must install");
    vm.replay_shared(&shared, &null_emitter(), "Test")
        .expect("shared program must run");
    vm.install_captured_bindings()
        .expect("captured bindings must install");

    assert_eq!(
        run_scalar(&vm, &inspect, &null_emitter(), "Test")
            .expect("inspection must run")
            .as_deref(),
        Some("nil,nil,private input,userdata")
    );
}

#[test]
fn section_vm_reports_store_operations_in_each_chunk() {
    let write = program("store.write('state.txt', args)");
    let read = program("return store.read('state.txt')");
    let recorder = Arc::new(Recorder::default());
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Gather").expect("VM must build");
    vm.inject_values("private input", &json!({}), &fresh_access())
        .expect("Engine values must inject");
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
    let access = fresh_access();
    for (source, expected) in [
        ("return 'text'", Some("text")),
        ("return 42", Some("42")),
        ("return 1.5", Some("1.5")),
        ("return true", Some("true")),
        ("return nil", None),
    ] {
        let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
        vm.inject_values("", &json!({}), &access)
            .expect("Engine values must inject");
        assert_eq!(
            run_scalar(&vm, &program(source), &null_emitter(), "Test")
                .expect("scalar return must work")
                .as_deref(),
            expected
        );
    }

    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_values("", &json!({}), &access)
        .expect("Engine values must inject");
    let error = run_scalar(&vm, &program("return {}"), &null_emitter(), "Test")
        .expect_err("table returns must be refused");
    assert!(error.to_string().contains("cannot return a table"));
}

#[test]
fn section_vms_isolate_mutated_shared_globals() {
    let shared = program("counter = 0");
    let increment = program("counter = counter + 1; return counter");
    let access = fresh_access();
    let first = section_vm_with_shared(&shared, "", &access, &null_emitter(), "First")
        .expect("first VM must build");
    let second = section_vm_with_shared(&shared, "", &access, &null_emitter(), "Second")
        .expect("second VM must build");

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
    let shared = program("private_global = 'shared secret'");
    let prologue = program("var.value = args");
    let epilog = program("return 'epilog done'");
    let recorder = Arc::new(Recorder::default());
    let mut vm =
        SectionVm::new(&test_nonce(), recorder.emitter(), "Gather").expect("VM must build");
    vm.inject_values("private input", &json!({}), &fresh_access())
        .expect("Engine values must inject");
    let observer = recorder.emitter().clone();
    vm.install_engine_globals(&observer, "Gather")
        .expect("Engine globals must install");
    vm.replay_shared(&shared, recorder.emitter(), "Gather")
        .expect("shared program must run");
    run_scalar(&vm, &prologue, recorder.emitter(), "Gather").expect("prologue must run");
    run_scalar(&vm, &epilog, recorder.emitter(), "Gather").expect("epilog must run");
    vm.teardown(recorder.emitter(), "Gather");

    let observations = recorder.observations();
    assert_eq!(
        observations,
        [
            detail::LUA_SHARED_LOAD_STARTED,
            detail::LUA_SHARED_LOAD_SUCCEEDED,
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
    assert!(!trace.contains("shared secret"));
    assert!(!trace.contains("private input"));
}

#[test]
fn section_lifecycle_failures_report_their_phase() {
    let recorder = Arc::new(Recorder::default());
    let failing_shared = program("error('private shared failure')");
    let mut vm =
        SectionVm::new(&test_nonce(), recorder.emitter(), "Shared").expect("VM must build");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("Engine values must inject");
    let observer = recorder.emitter().clone();
    vm.install_engine_globals(&observer, "Shared")
        .expect("Engine globals must install");
    vm.replay_shared(&failing_shared, recorder.emitter(), "Shared")
        .expect_err("shared execution must fail");
    vm.teardown(recorder.emitter(), "Shared");
    assert_eq!(
        recorder.observations(),
        [
            detail::LUA_SHARED_LOAD_STARTED,
            detail::LUA_SHARED_LOAD_FAILED,
            detail::LUA_TEARDOWN_STARTED,
            detail::LUA_TEARDOWN_SUCCEEDED,
        ]
        .into_iter()
        .map(|detail| ("Shared".to_owned(), detail.clone()))
        .collect::<Vec<_>>()
    );

    let recorder = Recorder::default();
    let vm = SectionVm::new(&test_nonce(), &null_emitter(), "Prologue").expect("VM must build");
    run_scalar(&vm, &program("return nil"), recorder.emitter(), "Prologue")
        .expect_err("prologue before injection must fail");
    assert!(
        recorder
            .observations()
            .iter()
            .any(|(_, event)| *event == detail::LUA_CHUNK_FAILED)
    );
}
