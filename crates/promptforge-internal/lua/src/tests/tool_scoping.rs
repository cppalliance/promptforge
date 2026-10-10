//! Tool scoping in a section VM: the prompt-wide `tools.always_offer` and
//! the section's `tools.offer` over canonical ids and tool objects, frozen
//! tool objects, and sections over an empty offering.

use super::*;

#[test]
fn always_offer_records_prompt_wide_wire_names_on_the_shared_set() {
    let set = shared_set(fixture_set(
        &[("fetch", "fetch a page"), ("search", "search the web")],
        &[],
    ));
    let mut vm = section_vm_with_set(&set, &null_emitter(), "Section")
        .expect("the section VM builds over the shared set");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    run_scalar(
        &vm,
        &program("tools.always_offer('fixtures/search')"),
        &null_emitter(),
        "Section",
    )
    .expect("tools.always_offer records the prompt-wide tool");
    vm.teardown(&null_emitter(), "Section");

    let bindings = set.lock().expect("the shared set locks");
    assert_eq!(
        bindings
            .offered()
            .iter()
            .map(|binding| (binding.alias(), binding.description(), binding.id().name()))
            .collect::<Vec<_>>(),
        [
            ("fixtures_fetch", "fetch a page", "fetch"),
            ("fixtures_search", "search the web", "search"),
        ]
    );
    assert_eq!(bindings.always(), ["fixtures_search"]);
}

#[test]
fn always_offer_records_a_model_description_override() {
    let set = shared_set(fixture_set(
        &[("fetch", "fetch a page"), ("search", "search the web")],
        &[],
    ));
    let mut vm = section_vm_with_set(&set, &null_emitter(), "Section")
        .expect("the section VM builds over the shared set");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    run_scalar(
        &vm,
        &program("tools.always_offer('fixtures/search', 'always override')"),
        &null_emitter(),
        "Section",
    )
    .expect("tools.always_offer records the override");
    vm.teardown(&null_emitter(), "Section");

    let bindings = set.lock().expect("the shared set locks");
    assert_eq!(
        bindings.offered()[1].model_description(),
        Some("always override"),
        "tools.always_offer's second argument updates the run's binding"
    );
}

#[test]
fn tool_objects_are_frozen() {
    let bindings = fixture_set(&[("search", "search the web")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.get('fixtures/search').description = 'x'"),
        &null_emitter(),
        "Section",
    )
    .expect_err("assigning .description on a tool object must fail");
    assert!(
        error.to_string().contains("description"),
        "the error must name the frozen field: {error}"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn a_tool_reads_through_its_object_and_never_through_a_global() {
    let bindings = fixture_set(&[("search", "search the web")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    run_scalar(
        &vm,
        &program(
            "local search = tools.get('fixtures/search')\n\
             assert(search.id == 'fixtures/search')\n\
             assert(search.description == 'search the web')\n\
             assert(fixtures_search == nil)\n\
             assert(_G.search == nil)",
        ),
        &null_emitter(),
        "Section",
    )
    .expect("the offered tool reads through its object");
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn tools_bind_is_gone_from_every_section() {
    let bindings = fixture_set(&[("search", "search the web")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");

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
fn always_offer_refuses_an_id_the_run_does_not_offer_and_is_idempotent() {
    let set = shared_set(fixture_set(&[("search", "search the web")], &[]));
    let mut vm = section_vm_with_set(&set, &null_emitter(), "Section")
        .expect("the section VM builds over the shared set");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.always_offer('fixtures/missing')"),
        &null_emitter(),
        "Section",
    )
    .expect_err("offering a tool the run lacks is an error");
    assert!(
        error
            .to_string()
            .contains("tools.always_offer: \"fixtures/missing\" is not a catalog tool in this run"),
        "the error must name the id: {error}"
    );
    // The shared library replays into every section, so offering the same
    // tool again is a no-op, not a duplicate error.
    run_scalar(
        &vm,
        &program("tools.always_offer('fixtures/search'); tools.always_offer('fixtures/search')"),
        &null_emitter(),
        "Section",
    )
    .expect("offering the same tool again is idempotent");
    assert_eq!(
        set.lock().expect("the shared set locks").always(),
        &["fixtures_search".to_owned()],
        "the tool is recorded exactly once"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn section_scope_closes_to_always_then_offered() {
    let bindings = fixture_set(
        &[("fetch", "fetch a page"), ("search", "search the web")],
        &["fixtures_search"],
    );
    let prologue = program("tools.offer({'fixtures/fetch', 'fixtures/search'})");
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    run_scalar(&vm, &prologue, &null_emitter(), "Section").expect("section offers must record");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_search", "fixtures_fetch"]
    );
}

#[test]
fn tools_offer_accepts_tool_objects_and_arrays() {
    let bindings = fixture_set(
        &[("fetch", "fetch a page"), ("search", "search the web")],
        &[],
    );
    let prologue = program(
        "local search, fetch = tools.get('fixtures/search'), tools.get('fixtures/fetch')\n\
         tools.offer(search)\n\
         tools.offer({fetch})\n\
         tools.offer({'fixtures/fetch', search})",
    );
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    run_scalar(&vm, &prologue, &null_emitter(), "Section")
        .expect("tools.offer must accept tool objects, ids, and arrays");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_search", "fixtures_fetch"]
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn empty_offer_is_a_no_op_and_failed_bulk_offer_is_atomic() {
    let bindings = fixture_set(
        &[("fetch", "fetch a page"), ("search", "search the web")],
        &[],
    );
    let prologue = program(
        "tools.offer(); \
             local ok = pcall(tools.offer, {'fixtures/search', 'fixtures/missing'}); \
             if ok then error('invalid offer unexpectedly succeeded') end; \
             tools.offer('fixtures/fetch')",
    );
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    run_scalar(&vm, &prologue, &null_emitter(), "Section")
        .expect("caught failed offer must not poison recording");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_fetch"],
        "empty offer changes nothing and a failed offer records no partial entries"
    );
}

#[test]
fn offer_rejects_misshapen_override_arguments() {
    let bindings = fixture_set(&[("search", "search the web")], &[]);
    let prologue = program(
        "local ok, err = pcall(tools.offer, {'fixtures/search'}, 'bulk override'); \
         if ok or not string.find(tostring(err), 'tools.offer array form takes no override') then \
             error('array form with an override must fail loudly') \
         end; \
         local ok, err = pcall(tools.offer, 'fixtures/search', 42); \
         if ok or not string.find(tostring(err), 'tools.offer override must be a string') then \
             error('a non-string override must fail loudly') \
         end; \
         local ok, err = pcall(tools.offer, 'fixtures/search', 'override', 'extra'); \
         if ok or not string.find(tostring(err), 'one tool plus an optional override') then \
             error('a third argument must fail loudly') \
         end; \
         tools.offer('fixtures/search')",
    );
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    run_scalar(&vm, &prologue, &null_emitter(), "Section")
        .expect("rejected override forms must not poison recording");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_search"],
        "rejected calls record nothing and the later valid offer still lands"
    );
    assert_eq!(
        scope[0].model_description(),
        None,
        "rejected overrides leave the model description untouched"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn an_unknown_id_fails_before_scope_closure() {
    let bindings = fixture_set(&[("search", "search the web")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Section")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.offer('fixtures/missing')"),
        &null_emitter(),
        "Section",
    )
    .expect_err("only offered tools may enter the section scope");
    assert!(
        error
            .to_string()
            .contains("tools.offer: \"fixtures/missing\" is not a catalog tool in this run"),
        "the error names the id: {error}"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn tool_objects_install_without_payload_reports() {
    let bindings = ToolSet::for_test(
        Vec::new(),
        Vec::new(),
        vec![ToolBinding::for_test(
            "private_alias",
            "private capability",
            &fixture_tool("search"),
        )],
    );
    let recorder = Recorder::default();
    let mut vm = section_vm_with_bindings(&bindings, recorder.emitter(), "Section")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    let trace = format!("{:?}", recorder.observations());
    assert!(!trace.contains("private_alias"));
    assert!(!trace.contains("private capability"));
}

#[test]
fn offer_over_an_empty_offering_fails_in_a_chunk() {
    let error = run("tools.offer('web/search')", "").expect_err("an unoffered id must fail loudly");
    assert!(
        error
            .to_string()
            .contains("tools.offer: \"web/search\" is not a catalog tool in this run"),
        "the error must name the id: {error}"
    );
}

#[test]
fn offer_over_an_empty_offering_fails_in_a_prologue_without_a_shared_library() {
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("Engine values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.offer('web/search')"),
        &null_emitter(),
        "Test",
    )
    .expect_err("an unoffered id must fail loudly");
    assert!(
        error
            .to_string()
            .contains("is not a catalog tool in this run"),
        "the error must report the missing tool: {error}"
    );
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn offer_with_an_override_argument_records_the_model_description() {
    let bindings = fixture_set(&[("search", "search the web")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, &null_emitter(), "Test")
        .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("Engine values must inject");
    run_scalar(
        &vm,
        &program("tools.offer('fixtures/search', 'Search the web for pages matching a query.')"),
        &null_emitter(),
        "Test",
    )
    .expect("a description passed to tools.offer is the model-facing override");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");
    assert_eq!(
        scope[0].model_description(),
        Some("Search the web for pages matching a query."),
        "the offer override must reach the scoped binding"
    );
    vm.teardown(&null_emitter(), "Test");
}

#[test]
fn a_section_vm_over_an_empty_offering_snapshots_to_an_empty_scope() {
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Test").expect("VM must build");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("Engine values must inject");
    let (bindings, runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope = current_tool_bindings(&bindings, &runtime).expect("an empty scope must snapshot");
    assert!(scope.is_empty());
    vm.teardown(&null_emitter(), "Test");
}
