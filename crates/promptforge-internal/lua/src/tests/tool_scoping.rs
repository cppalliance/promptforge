//! Tool scoping in a section VM: the section's `tools.offer` over canonical
//! ids and tool objects, frozen tool objects, and sections over an empty
//! offering.

use super::*;

#[test]
fn tool_objects_are_frozen() {
    let vm = TestVm::new()
        .tools(fixture_set(&[("search", "search the web")]))
        .build();
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
    vm.teardown();
}

#[test]
fn a_tool_reads_through_its_object_and_never_through_a_global() {
    let vm = TestVm::new()
        .tools(fixture_set(&[("search", "search the web")]))
        .build();
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
    vm.teardown();
}

#[test]
fn section_scope_keeps_each_tool_at_its_first_offer() {
    let bindings = fixture_set(&[("fetch", "fetch a page"), ("search", "search the web")]);
    let prologue = program(
        "tools.offer('fixtures/search')\n\
         tools.offer({'fixtures/fetch', 'fixtures/search'})",
    );
    let vm = TestVm::new().tools(bindings).build();
    run_scalar(&vm, &prologue, &null_emitter(), "Section").expect("section offers must record");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_search", "fixtures_fetch"]
    );
}

#[test]
fn tools_offer_accepts_tool_objects_and_arrays() {
    let bindings = fixture_set(&[("fetch", "fetch a page"), ("search", "search the web")]);
    let prologue = program(
        "local search, fetch = tools.get('fixtures/search'), tools.get('fixtures/fetch')\n\
         tools.offer(search)\n\
         tools.offer({fetch})\n\
         tools.offer({'fixtures/fetch', search})",
    );
    let vm = TestVm::new().tools(bindings).build();
    run_scalar(&vm, &prologue, &null_emitter(), "Section")
        .expect("tools.offer must accept tool objects, ids, and arrays");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_search", "fixtures_fetch"]
    );
    vm.teardown();
}

#[test]
fn empty_offer_is_a_no_op_and_failed_bulk_offer_is_atomic() {
    let bindings = fixture_set(&[("fetch", "fetch a page"), ("search", "search the web")]);
    let prologue = program(
        "tools.offer(); \
             local ok = pcall(tools.offer, {'fixtures/search', 'fixtures/missing'}); \
             if ok then error('invalid offer unexpectedly succeeded') end; \
             tools.offer('fixtures/fetch')",
    );
    let vm = TestVm::new().tools(bindings).build();
    run_scalar(&vm, &prologue, &null_emitter(), "Section")
        .expect("caught failed offer must not poison recording");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_fetch"],
        "empty offer changes nothing and a failed offer records no partial entries"
    );
}

#[test]
fn offer_rejects_a_second_argument_and_records_nothing() {
    let bindings = fixture_set(&[("search", "search the web")]);
    let prologue = program(
        "local extra = 'tools.offer takes one tool or a list of them, got extra string'; \
         local ok, err = pcall(tools.offer, 'fixtures/search', 'a description'); \
         if ok or not string.find(tostring(err), extra, 1, true) then \
             error('a second argument to a single offer must fail loudly: ' .. tostring(err)) \
         end; \
         local ok, err = pcall(tools.offer, {'fixtures/search'}, 'a description'); \
         if ok or not string.find(tostring(err), extra, 1, true) then \
             error('a second argument to an array offer must fail loudly: ' .. tostring(err)) \
         end; \
         tools.offer('fixtures/search')",
    );
    let vm = TestVm::new().tools(bindings).build();
    run_scalar(&vm, &prologue, &null_emitter(), "Section")
        .expect("refused offers must not poison recording");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fixtures_search"],
        "refused calls record nothing and the later valid offer still lands"
    );
    vm.teardown();
}

#[test]
fn an_unknown_id_fails_before_scope_closure() {
    let vm = TestVm::new()
        .tools(fixture_set(&[("search", "search the web")]))
        .build();
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
    vm.teardown();
}

#[test]
fn tool_objects_install_without_payload_reports() {
    let bindings = ToolSet {
        offered: vec![ToolBinding::for_test(
            "private_alias",
            "private capability",
            &fixture_tool("search"),
        )],
        ..ToolSet::default()
    };
    let recorder = Recorder::default();
    let _vm = TestVm::new()
        .tools(bindings)
        .emitter(recorder.emitter())
        .build();
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
fn offer_over_an_empty_offering_fails_in_a_prologue() {
    let vm = TestVm::new().build();
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
    vm.teardown();
}

#[test]
fn a_section_vm_over_an_empty_offering_snapshots_to_an_empty_scope() {
    let vm = TestVm::new().build();
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("an empty scope must snapshot");
    assert!(scope.is_empty());
    vm.teardown();
}
