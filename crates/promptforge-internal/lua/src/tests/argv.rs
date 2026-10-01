//! The `argv` global: frozen outside H1 at any depth, writable in H1, and
//! read back as JSON data.

use super::*;

/// Builds a section VM with `argv` installed the way the executor installs
/// it: writable for the H1 pass, frozen for every other section.
fn argv_vm(argv: Option<&Json>, writable: bool) -> SectionVm {
    let mut vm = SectionVm::new_for_section(
        &test_nonce(),
        &shared_set(ToolSet::default()),
        &Arc::new(Mutex::new(ModelSet::default())),
        &null_emitter(),
        "Argv",
    )
    .expect("section VM must build");
    let argv = if writable {
        Argv::Writable(argv)
    } else {
        Argv::Frozen(argv)
    };
    vm.inject_values_with_var("", &json!({}), &fresh_access(), None, argv)
        .expect("values must inject");
    vm
}

/// Runs one chunk on an argv VM, returning the block's failure.
fn run_argv(vm: &SectionVm, source: &str) -> Result<Option<String>> {
    run_scalar(vm, &program(source), &null_emitter(), "Argv")
}

#[test]
fn frozen_argv_reads_through_the_guard() {
    let argv = json!({ "query": "papers", "nested": { "hits": 2 } });
    let vm = argv_vm(Some(&argv), false);
    let out = run_argv(
        &vm,
        "assert(argv.query == 'papers', 'a field reads through')\n\
         assert(argv.nested.hits == 2, 'a nested field reads through')\n\
         assert(argv.absent == nil, 'an absent field reads nil')\n\
         return argv.query",
    )
    .expect("frozen argv must read");
    assert_eq!(out.as_deref(), Some("papers"));
    vm.teardown(&null_emitter(), "Argv");
}

#[test]
fn frozen_argv_rejects_reassignment() {
    let argv = json!({ "query": "papers" });
    let vm = argv_vm(Some(&argv), false);
    let error = run_argv(&vm, "argv = { query = 'hijacked' }")
        .expect_err("reassigning argv outside H1 must fail");
    assert!(
        error.to_string().contains("argv is frozen"),
        "the error names the freeze: {error}"
    );
    vm.teardown(&null_emitter(), "Argv");
}

#[test]
fn frozen_argv_rejects_writes_at_any_depth() {
    let argv = json!({ "query": "papers", "nested": { "hits": 2 } });
    let vm = argv_vm(Some(&argv), false);
    let error = run_argv(&vm, "argv.query = 'hijacked'")
        .expect_err("a field write on frozen argv must fail");
    assert!(
        error.to_string().contains("argv is frozen"),
        "the error names the freeze: {error}"
    );
    let error = run_argv(&vm, "argv.nested.hits = 3")
        .expect_err("a nested field write on frozen argv must fail");
    assert!(
        error.to_string().contains("argv is frozen"),
        "the deep freeze rejects nested writes: {error}"
    );
    vm.teardown(&null_emitter(), "Argv");
}

#[test]
fn a_frozen_argv_write_raises_the_whole_refusal_for_a_string_or_integer_key() {
    let argv = json!({ "query": "papers" });
    let vm = argv_vm(Some(&argv), false);
    for (target, refusal) in [
        (
            "argv.query",
            "argv is frozen outside H1: cannot set field 'query'",
        ),
        (
            "argv[1]",
            "argv is frozen outside H1: cannot set field Integer(1)",
        ),
    ] {
        let source =
            format!("local ok, err = pcall(function() {target} = 1 end)\nreturn tostring(err)");
        let out = run_argv(&vm, &source).expect("the pcall catches the refusal");
        let caught = out.expect("the chunk returns the caught error");
        assert_eq!(refusal_line(&caught), Some(refusal), "writing {target}");
    }
    let label = run_argv(&vm, "return getmetatable(argv)").expect("getmetatable reads");
    assert_eq!(label.as_deref(), Some("argv is frozen"));
    vm.teardown(&null_emitter(), "Argv");
}

#[test]
fn frozen_nil_argv_reads_nil_and_rejects_assignment() {
    let vm = argv_vm(None, false);
    let out = run_argv(&vm, "assert(argv == nil, 'no argv reads nil') return 'ok'")
        .expect("a nil argv reads nil");
    assert_eq!(out.as_deref(), Some("ok"));
    let error =
        run_argv(&vm, "argv = {}").expect_err("assigning a nil argv outside H1 must still fail");
    assert!(
        error.to_string().contains("argv is frozen"),
        "the error names the freeze: {error}"
    );
    vm.teardown(&null_emitter(), "Argv");
}

#[test]
fn frozen_scalar_argv_reads_and_rejects_assignment() {
    let argv = json!(5);
    let vm = argv_vm(Some(&argv), false);
    let out =
        run_argv(&vm, "assert(argv == 5) return tostring(argv)").expect("a scalar argv reads");
    assert_eq!(out.as_deref(), Some("5"));
    let error = run_argv(&vm, "argv = 6").expect_err("reassigning a scalar argv must fail");
    assert!(
        error.to_string().contains("argv is frozen"),
        "the error names the freeze: {error}"
    );
    vm.teardown(&null_emitter(), "Argv");
}

#[test]
fn the_frozen_argv_guard_leaves_other_globals_alone() {
    let argv = json!({ "query": "papers" });
    let vm = argv_vm(Some(&argv), false);
    let out = run_argv(
        &vm,
        "scratch = 42\n\
         assert(scratch == 42, 'a bare global still assigns')\n\
         assert(absent_global == nil, 'an absent global still reads nil')\n\
         return 'ok'",
    )
    .expect("ordinary globals must be untouched by the argv guard");
    assert_eq!(out.as_deref(), Some("ok"));
    vm.teardown(&null_emitter(), "Argv");
}

#[test]
fn writable_argv_repairs_and_reads_back() {
    // The H1 repair pattern: malformed args start as nil argv; H1 assigns
    // the repaired table; the Engine reads the repair back at the freeze.
    let vm = argv_vm(None, true);
    let out = run_argv(
        &vm,
        "assert(argv == nil, 'malformed args start as nil argv')\n\
         argv = { query = 'repaired' }\n\
         argv.extra = 1\n\
         return argv.query",
    )
    .expect("H1 argv is writable");
    assert_eq!(out.as_deref(), Some("repaired"));
    let read_back = vm.argv_json().expect("the repair reads back");
    assert_eq!(read_back, Some(json!({ "query": "repaired", "extra": 1 })));
    vm.teardown(&null_emitter(), "Argv");
}

#[test]
fn writable_argv_field_writes_read_back() {
    let argv = json!({ "query": "broken" });
    let vm = argv_vm(Some(&argv), true);
    let out = run_argv(&vm, "argv.query = 'fixed' return argv.query")
        .expect("a field write on the writable argv runs");
    assert_eq!(out.as_deref(), Some("fixed"));
    let read_back = vm.argv_json().expect("the field write reads back");
    assert_eq!(read_back, Some(json!({ "query": "fixed" })));
    vm.teardown(&null_emitter(), "Argv");
}

#[test]
fn argv_read_back_rejects_a_non_data_assignment() {
    let vm = argv_vm(None, true);
    run_argv(&vm, "argv = function() end return 'ok'").expect("the assignment itself runs");
    let error = vm
        .argv_json()
        .expect_err("a function argv cannot read back as JSON");
    assert!(
        error.to_string().contains("argv must be JSON data"),
        "the error says why: {error}"
    );
    vm.teardown(&null_emitter(), "Argv");
}
