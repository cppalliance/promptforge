//! Tool scoping: filled slots and their recorded identities, the `always`
//! and `tools.add` scope operations, frozen Tool objects, and sections with
//! no declared slots.

use super::*;

#[test]
fn filled_slots_record_exact_aliases_descriptions_identities_and_always_scope() {
    let set = shared_set(fixture_set(
        &[
            ("web_search", "search the web", "search"),
            ("web_fetch2", "fetch a page", "fetch"),
        ],
        &[],
    ));
    let mut vm = section_vm_with_set(&set, &null_emitter(), "Section")
        .expect("the section VM builds over the shared set");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    run_scalar(
        &vm,
        &program("tools.always('web_search')"),
        &null_emitter(),
        "Section",
    )
    .expect("tools.always records the prompt-wide alias");
    vm.teardown(&null_emitter(), "Section");

    let bindings = set.lock().expect("the shared set locks");
    assert_eq!(
        bindings
            .bindings()
            .iter()
            .map(|binding| (binding.alias(), binding.description(), binding.id().name()))
            .collect::<Vec<_>>(),
        [
            ("web_search", "search the web", "search"),
            ("web_fetch2", "fetch a page", "fetch"),
        ]
    );
    assert_eq!(bindings.always(), ["web_search"]);
}

#[test]
fn always_records_a_model_description_override() {
    let set = shared_set(fixture_set(
        &[
            ("web_search", "search the web", "search"),
            ("web_fetch2", "fetch a page", "fetch"),
        ],
        &[],
    ));
    let mut vm = section_vm_with_set(&set, &null_emitter(), "Section")
        .expect("the section VM builds over the shared set");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    run_scalar(
        &vm,
        &program("tools.always('web_fetch2', 'always override')"),
        &null_emitter(),
        "Section",
    )
    .expect("tools.always records the override");
    vm.teardown(&null_emitter(), "Section");

    let bindings = set.lock().expect("the shared set locks");
    assert_eq!(
        bindings.bindings()[1].model_description(),
        Some("always override"),
        "tools.always's second argument updates the recorded override"
    );
}

#[test]
fn tool_handles_are_frozen() {
    let bindings = fixture_set(&[("search", "search the web", "search")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    let error = run_scalar(
        &vm,
        &program("search.description = 'x'"),
        &null_emitter(),
        "Section",
    )
    .expect_err("assigning .description on a Tool object must fail");
    assert!(
        error.to_string().contains("description"),
        "the error must name the frozen field: {error}"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn bound_slot_globals_are_inspectable_tool_objects() {
    let bindings = fixture_set(&[("search", "search the web", "search")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("section install must expose the inspectable Tool object");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    run_scalar(
        &vm,
        &program(
            "assert(search.name == 'search')\n\
             assert(search.description == 'search the web')\n\
             assert(type(search.parameters) == 'table')\n\
             assert(search.wire_name == 'search')\n\
             assert(search.untrusted == false)",
        ),
        &null_emitter(),
        "Section",
    )
    .expect("the bound slot's global is an inspectable Tool object");
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn scoping_validates_aliases_exactly() {
    for alias in [
        "",
        "_leading",
        "has.dot",
        "nonasciié",
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-a",
    ] {
        let bindings = fixture_set(&[("search", "search the web", "search")], &[]);
        let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
            .expect("captured bindings must install");
        vm.inject_host("", &json!({}), &fresh_access())
            .expect("host must inject");
        let error = run_scalar(
            &vm,
            &program(&format!("tools.add({alias:?})")),
            &null_emitter(),
            "Section",
        )
        .expect_err("invalid aliases must be rejected");
        assert!(
            error.to_string().contains("invalid alias"),
            "wrong error for {alias:?}: {error}"
        );
        vm.teardown(&null_emitter(), "Section");
    }

    for valid in ["Upper", "has-dash", &format!("A{}", "2".repeat(63))] {
        let bindings = fixture_set(&[(valid, "a capability", "search")], &[]);
        let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
            .expect("captured bindings must install");
        vm.inject_host("", &json!({}), &fresh_access())
            .expect("host must inject");
        run_scalar(
            &vm,
            &program(&format!("tools.add({valid:?})")),
            &null_emitter(),
            "Section",
        )
        .expect("planned alias forms must be valid");
        vm.teardown(&null_emitter(), "Section");
    }
}

#[test]
fn tools_bind_is_gone_from_every_section() {
    let bindings = fixture_set(&[("search", "search the web", "search")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");

    let gone = run_scalar(
        &vm,
        &program("return tostring(tools.bind)"),
        &null_emitter(),
        "Section",
    )
    .expect("the probe runs");
    assert_eq!(gone.as_deref(), Some("nil"), "tools.bind is removed");
    let error = run_scalar(
        &vm,
        &program("tools.bind('other', 'fetch a page')"),
        &null_emitter(),
        "Section",
    )
    .expect_err("calling the removed tools.bind fails");
    assert!(
        error.to_string().contains("nil"),
        "a removed function fails as a nil call: {error}"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn always_rejects_an_unbound_alias_and_is_idempotent() {
    let set = shared_set(fixture_set(&[("search", "search the web", "search")], &[]));
    let mut vm = section_vm_with_set(&set, &null_emitter(), "Section")
        .expect("the section VM builds over the shared set");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    let error = run_scalar(
        &vm,
        &program("tools.always('missing')"),
        &null_emitter(),
        "Section",
    )
    .expect_err("advertising an unfilled alias is an error");
    assert!(
        error
            .to_string()
            .contains("tools.always alias \"missing\" is not a bound tool slot"),
        "the error must identify the unfilled alias: {error}"
    );
    // The shared library replays into every section, so re-parking the same
    // alias is a no-op, not a duplicate error.
    run_scalar(
        &vm,
        &program("tools.always('search'); tools.always('search')"),
        &null_emitter(),
        "Section",
    )
    .expect("re-parking the same alias is idempotent");
    assert_eq!(
        set.lock().expect("the shared set locks").always(),
        &["search".to_owned()],
        "the alias is recorded exactly once"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn section_scope_closes_to_always_then_added() {
    let bindings = fixture_set(
        &[
            ("search", "search the web", "search"),
            ("fetch", "fetch a page", "fetch"),
        ],
        &["search"],
    );
    let prologue = program("tools.add({'fetch', 'search'})");
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    run_scalar(&vm, &prologue, &null_emitter(), "Section").expect("section additions must record");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["search", "fetch"]
    );
}

#[test]
fn tools_add_accepts_tool_objects_and_arrays() {
    let bindings = fixture_set(
        &[
            ("search", "search the web", "search"),
            ("fetch", "fetch a page", "fetch"),
        ],
        &[],
    );
    let prologue = program(
        "tools.add(search); \
             tools.add({fetch}); \
             tools.add({'fetch', search})",
    );
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    run_scalar(&vm, &prologue, &null_emitter(), "Section")
        .expect("tools.add must accept Tool objects, strings, and arrays");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["search", "fetch"]
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn empty_add_is_a_no_op_and_failed_bulk_add_is_atomic() {
    let bindings = fixture_set(
        &[
            ("search", "search the web", "search"),
            ("fetch", "fetch a page", "fetch"),
        ],
        &[],
    );
    let prologue = program(
        "tools.add(); \
             local ok = pcall(tools.add, {'search', 'missing'}); \
             if ok then error('invalid add unexpectedly succeeded') end; \
             tools.add('fetch')",
    );
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    run_scalar(&vm, &prologue, &null_emitter(), "Section")
        .expect("caught failed add must not poison recording");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fetch"],
        "empty add changes nothing and failed add records no partial aliases"
    );
}

#[test]
fn add_rejects_misshapen_override_arguments() {
    let bindings = fixture_set(&[("search", "search the web", "search")], &[]);
    let prologue = program(
        "local ok, err = pcall(tools.add, {'search'}, 'bulk override'); \
         if ok or not string.find(tostring(err), 'array form takes no override') then \
             error('array form with an override must fail loudly') \
         end; \
         local ok, err = pcall(tools.add, 'search', 42); \
         if ok or not string.find(tostring(err), 'override must be a string') then \
             error('a non-string override must fail loudly') \
         end; \
         local ok, err = pcall(tools.add, 'search', 'override', 'extra'); \
         if ok or not string.find(tostring(err), 'one alias plus an optional override') then \
             error('a third argument must fail loudly') \
         end; \
         tools.add('search')",
    );
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    run_scalar(&vm, &prologue, &null_emitter(), "Section")
        .expect("rejected override forms must not poison recording");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["search"],
        "rejected calls record nothing and the later valid add still lands"
    );
    assert_eq!(
        scope[0].model_description(),
        None,
        "rejected overrides leave the model description untouched"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn unknown_scoped_alias_fails_before_scope_closure() {
    let bindings = fixture_set(&[("search", "search the web", "search")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('missing')"),
        &null_emitter(),
        "Section",
    )
    .expect_err("only bound aliases may enter the section scope");
    assert!(
        error
            .to_string()
            .contains("tools.add alias \"missing\" is not a bound tool slot"),
        "the error names the unbound alias: {error}"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn captured_bindings_are_installed_without_payload_reports() {
    let bindings = ToolSet::for_test(
        vec![ToolBinding::for_test(
            "private_alias",
            "private capability",
            &fixture_tool("search"),
        )],
        Vec::new(),
    );
    let recorder = Recorder::default();
    let mut vm = section_vm_with_bindings(&bindings, recorder.emitter(), "Section")
        .expect("captured binding installation must succeed");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");
    let trace = format!("{:?}", recorder.observations());
    assert!(!trace.contains("private_alias"));
    assert!(!trace.contains("private capability"));
}

#[test]
fn add_without_declarations_fails_as_unbound_in_a_chunk() {
    let error = run("tools.add('web_search')", "").expect_err("an unbound alias must fail loudly");
    assert!(
        error
            .to_string()
            .contains("tools.add alias \"web_search\" is not a bound tool slot"),
        "the error must name the unbound alias: {error}"
    );
}

#[test]
fn add_without_declarations_fails_in_a_prologue_without_a_shared_library() {
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('web_search')"),
        &null_emitter(),
        "Test",
    )
    .expect_err("an unbound alias must fail loudly");
    assert!(
        error.to_string().contains("is not a bound tool slot"),
        "the error must report the missing slot: {error}"
    );
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn add_with_empty_frozen_bindings_fails_as_unbound() {
    let bindings = ToolSet::default();
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Test")
        .expect("empty captured bindings must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('web_search')"),
        &null_emitter(),
        "Test",
    )
    .expect_err("an unbound alias must fail loudly");
    assert!(
        error.to_string().contains("is not a bound tool slot"),
        "the error must report the missing slot: {error}"
    );
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn add_with_an_override_argument_records_the_model_description() {
    let bindings = fixture_set(&[("search", "search the web", "search")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Test")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host values must inject");
    run_scalar(
        &vm,
        &program("tools.add('search', 'Search the web for pages matching a query.')"),
        &null_emitter(),
        "Test",
    )
    .expect("a description passed to tools.add is the model-facing override");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");
    assert_eq!(
        scope[0].model_description(),
        Some("Search the web for pages matching a query."),
        "the add override must reach the scoped binding"
    );
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn a_section_vm_without_declarations_snapshots_to_an_empty_scope() {
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host values must inject");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("an empty scope must snapshot");
    assert!(scope.is_empty());
    vm.teardown(&null_emitter(), "Test");
}
