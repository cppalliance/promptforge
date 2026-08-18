use std::sync::{Arc, Mutex};

use super::*;
use crate::observe::{NullObserver, Observation};
use crate::store::{Store, StoreError};
use crate::tools::{Tool, ToolError, ToolOutput};
use serde_json::json;

const EXECUTION: &str = "lua-test";

#[derive(Default)]
struct Recorder(Mutex<Vec<(String, String, Observation)>>);

impl Observer for Recorder {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .push((execution.to_owned(), section.to_owned(), event));
    }
}

impl Recorder {
    fn records(&self) -> Vec<(String, String, Observation)> {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .clone()
    }

    fn observations(&self) -> Vec<(String, Observation)> {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .iter()
            .map(|(_, section, detail)| (section.clone(), detail.clone()))
            .collect()
    }
}

#[derive(Debug)]
struct FailingStore;

impl FailingStore {
    fn error(path: &str) -> StoreError {
        StoreError::NotFound {
            path: path.to_owned(),
        }
    }
}

impl Store for FailingStore {
    fn write(&mut self, path: &str, _contents: &str) -> std::result::Result<(), StoreError> {
        Err(Self::error(path))
    }

    fn append(&mut self, path: &str, _contents: &str) -> std::result::Result<(), StoreError> {
        Err(Self::error(path))
    }

    fn read_lines(&self, path: &str) -> std::result::Result<String, StoreError> {
        Err(Self::error(path))
    }

    fn read(&self, path: &str) -> std::result::Result<String, StoreError> {
        Err(Self::error(path))
    }

    fn str_replace(
        &mut self,
        path: &str,
        _old: &str,
        _new: &str,
    ) -> std::result::Result<(), StoreError> {
        Err(Self::error(path))
    }

    fn delete(&mut self, path: &str) -> std::result::Result<(), StoreError> {
        Err(Self::error(path))
    }

    fn glob(&self, pattern: &str) -> std::result::Result<Vec<String>, StoreError> {
        Err(Self::error(pattern))
    }

    fn exists(&self, path: &str) -> std::result::Result<bool, StoreError> {
        Err(Self::error(path))
    }
}

struct BoundaryRecorder {
    store: StoreRef,
    snapshots: Mutex<Vec<Vec<String>>>,
}

impl Observer for BoundaryRecorder {
    fn observe(&self, _execution: &str, _section: &str, _event: Observation) {
        self.snapshots
            .lock()
            .expect("the snapshot mutex must not be poisoned")
            .push(self.store.glob("**").expect("the memory store can glob"));
    }
}

fn run(source: &str, args: &str) -> Result<LuaOutcome> {
    run_chunk(
        source,
        args,
        &json!({ "id": 1, "when": "t" }),
        &StoreRef::memory(),
        EXECUTION,
        &NullObserver,
        "Test",
    )
}

/// Run a chunk against a caller-supplied store, so a test can inspect the
/// store after the chunk has run.
fn run_with(source: &str, store: &StoreRef) -> Result<LuaOutcome> {
    run_chunk(
        source,
        "",
        &json!({ "id": 1, "when": "t" }),
        store,
        EXECUTION,
        &NullObserver,
        "Test",
    )
}

/// Runs one chunk on an existing VM and unwraps the scalar return, failing
/// the test on a `jump` transfer.
fn run_scalar(
    vm: &SectionVm,
    program: &LuaProgram,
    observer: &dyn Observer,
    section: &str,
) -> Result<Option<String>> {
    match vm.run_chunk(program, observer, section)? {
        LuaBlockResult::Returned(value) => Ok(value),
        LuaBlockResult::Jump(heading) => Err(Error::Lua(format!("unexpected jump to {heading}"))),
    }
}

fn program(source: &str) -> LuaProgram {
    LuaProgram::compile(
        source,
        "test program",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Test",
    )
    .expect("test Lua must compile")
}

#[derive(Debug)]
struct FixtureTool(&'static str);

#[async_trait::async_trait]
impl Tool for FixtureTool {
    fn id(&self) -> ToolId {
        ToolId::new("fixtures", self.0).expect("valid id")
    }

    fn wire_name(&self) -> &'static str {
        self.0
    }

    fn description(&self) -> &'static str {
        "fixture"
    }

    fn parameters_schema(&self) -> Json {
        json!({})
    }

    async fn call(&self, _arguments: Json) -> std::result::Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted(String::new()))
    }
}

fn execute_live_tool_needs(
    source: &LuaProgram,
    resolver: &dyn ToolResolver,
    _execution: &str,
    _observer: &dyn Observer,
    _section: &str,
) -> Result<ToolBindings> {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(FixtureTool("search")),
        Arc::new(FixtureTool("fetch")),
    ];
    let registry =
        ToolRegistry::new(tools.iter().map(AsRef::as_ref)).expect("unique test registry");
    let models = |description: &str, _: &crate::model::ModelNeedOpts| {
        Err(Error::ModelAbsent {
            capability: description.to_owned(),
        })
    };
    let producer = LiveBindingProducer::default();
    let lua = Lua::new();
    harden(&lua)?;
    let result = lua.scope(|scope| {
        producer
            .install(&lua, scope, resolver, &registry, &models)
            .map_err(|error| mlua::Error::external(error.to_string()))?;
        lua.load(source.bytecode.as_slice()).exec()
    });
    if let Some(error) = producer.take_callback_error()? {
        return Err(error);
    }
    result.map_err(Error::lua)?;
    producer.bindings().map(|(tools, _)| tools)
}

fn section_vm_with_bindings(
    _source: &LuaProgram,
    bindings: &ToolBindings,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<SectionVm> {
    SectionVm::new_for_section(
        None,
        bindings,
        &<ModelBindings as Default>::default(),
        execution,
        observer,
        section,
    )
}

fn fixture_bindings(source: &str) -> (LuaProgram, ToolBindings) {
    let shared = program(source);
    let resolver = |description: &str| {
        Ok(ToolId::new(
            "fixtures",
            if description == "search the web" {
                "search"
            } else {
                "fetch"
            },
        )
        .expect("valid id"))
    };
    let bindings = execute_live_tool_needs(&shared, &resolver, EXECUTION, &NullObserver, "Prompt")
        .expect("fixture needs must resolve");
    (shared, bindings)
}

#[test]
fn direct_output_is_absent_in_every_executable_lua_vm() {
    let library = program("assert(print == nil); assert(warn == nil); log('library load')");
    let library_vm = SectionVm::new(Some(&library), EXECUTION, &NullObserver, "Section")
        .expect("library VM must not expose direct output");
    library_vm.teardown(&NullObserver, "Section");

    let shared = program(
        "assert(print == nil)\n\
             assert(warn == nil)\n\
             tools.need('search', 'search the web')",
    );
    let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
    let bindings = execute_live_tool_needs(&shared, &resolver, EXECUTION, &NullObserver, "Prompt")
        .expect("live H1 VM must not expose direct output");
    let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
        .expect("section VM must not expose direct output");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(
        &vm,
        &program("assert(print == nil); assert(warn == nil)"),
        &NullObserver,
        "Section",
    )
    .expect("prologue must not expose direct output");
    run_scalar(
        &vm,
        &program("assert(print == nil); assert(warn == nil)"),
        &NullObserver,
        "Section",
    )
    .expect("epilog must not expose direct output");
    vm.teardown(&NullObserver, "Section");

    assert_eq!(
        run("return tostring(print) .. ':' .. tostring(warn)", "")
            .expect("compatibility VM must run")
            .returned
            .as_deref(),
        Some("nil:nil")
    );
}

#[test]
fn logs_are_correlated_and_ordered_across_chunks() {
    let recorder = Arc::new(Recorder::default());
    let bindings = ToolBindings::for_test(
        vec![ToolBinding::for_test(
            "search",
            "search the web",
            ToolId::new("fixtures", "search").expect("valid id"),
        )],
        Vec::new(),
    );
    let mut vm = section_vm_with_bindings(
        &program(""),
        &bindings,
        EXECUTION,
        recorder.as_ref(),
        "Gather",
    )
    .expect("section VM must install captured bindings");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let observer: Arc<dyn Observer> = recorder.clone();
    vm.install_host_apis(&observer, "Gather")
        .expect("host APIs must install");
    run_scalar(
        &vm,
        &program("log('prologue checkpoint')"),
        recorder.as_ref(),
        "Gather",
    )
    .expect("first chunk log must succeed");
    run_scalar(
        &vm,
        &program("log('epilog checkpoint')"),
        recorder.as_ref(),
        "Gather",
    )
    .expect("second chunk log must succeed");
    vm.teardown(recorder.as_ref(), "Gather");

    let details = recorder
        .records()
        .into_iter()
        .map(|(_, _, detail)| detail.to_string())
        .collect::<Vec<_>>();
    assert!(details.contains(&"Lua: prologue checkpoint".to_owned()));
    assert!(details.contains(&"Lua: epilog checkpoint".to_owned()));
    assert!(
        !details
            .iter()
            .any(|detail| detail.contains("binding") || detail.contains("replay"))
    );
}

#[test]
fn compatibility_chunk_logs_interleave_with_host_operations() {
    let recorder = Recorder::default();
    run_chunk(
        "log('before write')\n\
             store.write('state.txt', 'value')\n\
             log('after write')",
        "",
        &json!({}),
        &StoreRef::memory(),
        "compatibility-run",
        &recorder,
        "Compatibility",
    )
    .expect("compatibility logging must succeed");

    assert_eq!(
        recorder.records(),
        [
            (
                "compatibility-run".to_owned(),
                "Compatibility".to_owned(),
                Observation::Lua("before write".to_owned()),
            ),
            (
                "compatibility-run".to_owned(),
                "Compatibility".to_owned(),
                detail::STORE_WRITE_SUCCEEDED.clone(),
            ),
            (
                "compatibility-run".to_owned(),
                "Compatibility".to_owned(),
                Observation::Lua("after write".to_owned()),
            ),
        ]
    );
}

#[test]
fn log_accepts_exactly_one_bounded_control_free_utf8_string() {
    let invalid = [
        ("log()", "log expects exactly one argument"),
        ("log('one', 'two')", "log expects exactly one argument"),
        ("log(42)", "log message must be a UTF-8 string"),
        (
            "log(string.char(255))",
            "log message must be a UTF-8 string",
        ),
        (
            "log('first\\nsecond')",
            "log message must not contain newline or control characters",
        ),
        (
            "log('first\\tsecond')",
            "log message must not contain newline or control characters",
        ),
        (
            "log('first\u{2028}second')",
            "log message must not contain newline or control characters",
        ),
    ];
    for (source, expected) in invalid {
        let recorder = Recorder::default();
        let error = run_chunk(
            source,
            "",
            &json!({}),
            &StoreRef::memory(),
            EXECUTION,
            &recorder,
            "Validation",
        )
        .expect_err("invalid log input must fail");
        assert!(
            error.to_string().contains(expected),
            "wrong validation error for {source:?}: {error}"
        );
        assert!(
            recorder.records().is_empty(),
            "invalid log input must emit no report"
        );
    }

    let too_long = "é".repeat(LUA_LOG_CHARACTER_LIMIT + 1);
    let source = format!(
        "log({})",
        serde_json::to_string(&too_long).expect("test string must serialize")
    );
    let error = run(&source, "").expect_err("257 characters must fail");
    assert!(
        error
            .to_string()
            .contains("log message must be at most 256 characters")
    );

    let maximum = "é".repeat(LUA_LOG_CHARACTER_LIMIT);
    let source = format!(
        "log({})",
        serde_json::to_string(&maximum).expect("test string must serialize")
    );
    let recorder = Recorder::default();
    run_chunk(
        &source,
        "",
        &json!({}),
        &StoreRef::memory(),
        EXECUTION,
        &recorder,
        "Validation",
    )
    .expect("256 Unicode characters must succeed");
    assert_eq!(
        recorder.records(),
        [(
            EXECUTION.to_owned(),
            "Validation".to_owned(),
            Observation::Lua(maximum.clone()),
        )]
    );
}

#[test]
fn log_cumulative_byte_budget_is_enforced_before_the_event_budget() {
    // LUA-002: many small events must not emit unbounded total log bytes.
    // With a 4-event budget the byte budget is 4 * 256 = 1024 bytes; three
    // 400-byte messages (200 two-byte chars each) exceed it on the third
    // call, while only three of the four events have been spent - so the
    // BYTE ceiling, not the event ceiling, is what refuses the call.
    let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Budget").expect("VM builds");
    vm.apply_lua_limits(DEFAULT_LUA_MEMORY_BYTES, 4)
        .expect("limits apply");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host injects");
    let recorder = Arc::new(Recorder::default());
    let observer: Arc<dyn Observer> = recorder.clone();
    vm.install_host_apis(&observer, "Budget")
        .expect("host APIs must install");
    let program = program(
        "log(string.rep('é', 200))\n\
             log(string.rep('é', 200))\n\
             log(string.rep('é', 200))\n\
             return 'unreached'",
    );
    let error = run_scalar(&vm, &program, recorder.as_ref(), "Budget")
        .expect_err("the cumulative byte budget must refuse the third message");
    // LUA-002: the refusal is the stable typed quota error, not an opaque
    // Lua authoring string.
    assert!(
        matches!(
            error,
            Error::LuaQuota {
                resource: "log byte"
            }
        ),
        "the byte ceiling must surface as a typed LuaQuota: {error:?}"
    );
    let logged = recorder
        .records()
        .into_iter()
        .filter(|(_, _, event)| matches!(event, Observation::Lua(_)))
        .count();
    assert_eq!(
        logged, 2,
        "the first two messages fit under the byte budget; the third is refused"
    );
    vm.teardown(&NullObserver, "Budget");
}

#[test]
fn logging_does_not_change_results_or_store_effects_with_null_observer() {
    let source = "log('checkpoint')\n\
                      var.answer = args\n\
                      store.write('answer.txt', args)\n\
                      return var.answer";
    let recorded_store = StoreRef::memory();
    let recorder = Recorder::default();
    let observed_outcome = run_chunk(
        source,
        "same",
        &json!({}),
        &recorded_store,
        EXECUTION,
        &recorder,
        "Equivalence",
    )
    .expect("recorded execution must succeed");
    let null_store = StoreRef::memory();
    let silent = run_chunk(
        source,
        "same",
        &json!({}),
        &null_store,
        EXECUTION,
        &NullObserver,
        "Equivalence",
    )
    .expect("silent execution must succeed");

    assert_eq!(observed_outcome.returned, silent.returned);
    assert_eq!(observed_outcome.var, silent.var);
    assert_eq!(
        recorded_store
            .read("answer.txt")
            .expect("recorded write must persist"),
        null_store
            .read("answer.txt")
            .expect("silent write must persist")
    );
}

#[test]
fn installed_log_persists_across_chunks() {
    // `log` is installed once per section by `install_host_apis`, so a saved
    // reference stays live for every later chunk in the same VM.
    let recorder = Arc::new(Recorder::default());
    let observer: Arc<dyn Observer> = recorder.clone();
    let mut vm =
        SectionVm::new(None, EXECUTION, &NullObserver, "Section").expect("VM must construct");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    vm.install_host_apis(&observer, "Section")
        .expect("host APIs must install");
    run_scalar(
        &vm,
        &program("saved_log = log; log('first chunk')"),
        recorder.as_ref(),
        "Section",
    )
    .expect("first chunk log must succeed");
    run_scalar(
        &vm,
        &program("saved_log('retained call')"),
        recorder.as_ref(),
        "Section",
    )
    .expect("a retained log reference stays live for the section's lifecycle");
    vm.teardown(recorder.as_ref(), "Section");

    let details = recorder
        .records()
        .into_iter()
        .map(|(_, _, detail)| detail.to_string())
        .collect::<Vec<_>>();
    assert!(details.contains(&"Lua: first chunk".to_owned()));
    assert!(details.contains(&"Lua: retained call".to_owned()));
}

#[test]
fn concurrent_logs_keep_execution_ids_and_local_order() {
    let recorder = Arc::new(Recorder::default());
    let mut workers = Vec::new();
    for execution in ["execution-a", "execution-b"] {
        let recorder = Arc::clone(&recorder);
        workers.push(std::thread::spawn(move || {
            run_chunk(
                "log('first'); log('second')",
                "",
                &json!({}),
                &StoreRef::memory(),
                execution,
                recorder.as_ref(),
                "Concurrent",
            )
            .expect("concurrent log run must succeed");
        }));
    }
    for worker in workers {
        worker.join().expect("logging worker must finish");
    }

    let records = recorder.records();
    for execution in ["execution-a", "execution-b"] {
        assert_eq!(
            records
                .iter()
                .filter(|(actual, _, _)| actual == execution)
                .map(|(_, section, detail)| (section.clone(), detail.to_string()))
                .collect::<Vec<_>>(),
            [
                ("Concurrent".to_owned(), "Lua: first".to_owned()),
                ("Concurrent".to_owned(), "Lua: second".to_owned()),
            ]
        );
    }
}

#[test]
fn binding_records_exact_aliases_descriptions_identities_and_always_scope() {
    let source = "tools.need('web_search', 'search the web')\n\
                      tools.need('web_fetch2', 'fetch a page')\n\
                      tools.always('web_search')";
    let (_, bindings) = fixture_bindings(source);

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
fn tool_need_returns_inspectable_object() {
    let shared = program(
        "local tool = tools.need('search', 'search the web')\n\
             assert(tool.name == 'search')\n\
             assert(tool.description == 'search the web')\n\
             assert(type(tool.parameters) == 'table')\n\
             assert(tool.wire_name == 'search')\n\
             assert(tool.untrusted == false)\n\
             tools.always('search')",
    );
    let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
    let bindings = execute_live_tool_needs(&shared, &resolver, EXECUTION, &NullObserver, "Prompt")
        .expect("tools.need must return an inspectable Tool object");
    assert_eq!(bindings.bindings()[0].alias(), "search");

    let vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
        .expect("section install must expose the same inspectable Tool object");
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn binding_validates_aliases_exactly() {
    let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));

    for alias in [
        "",
        "_leading",
        "has.dot",
        "nonasciié",
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-a",
    ] {
        let need = program(&format!("tools.need({alias:?}, 'capability')"));
        let error = execute_live_tool_needs(&need, &resolver, EXECUTION, &NullObserver, "Prompt")
            .expect_err("invalid aliases must be rejected");
        assert!(
            error.to_string().contains("invalid tool alias"),
            "wrong error for {alias:?}: {error}"
        );
    }

    for valid in ["Upper", "has-dash", &format!("A{}", "2".repeat(63))] {
        let need = program(&format!("tools.need({valid:?}, 'capability')"));
        execute_live_tool_needs(&need, &resolver, EXECUTION, &NullObserver, "Prompt")
            .expect("planned alias forms must be valid");
    }
}

#[test]
fn live_h1_rejects_duplicate_aliases() {
    let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
    let error = execute_live_tool_needs(
        &program("tools.need('search', 'one'); tools.need('search', 'two')"),
        &resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .expect_err("duplicate aliases must fail");
    assert!(matches!(
        error,
        Error::DuplicateAlias { alias } if alias == "search"
    ));
}

#[test]
fn duplicate_alias_error_cannot_be_suppressed_with_lua_pcall() {
    let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
    let error = execute_live_tool_needs(
        &program("tools.need('search', 'one'); pcall(tools.need, 'search', 'two')"),
        &resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .expect_err("a caught duplicate callback must still fail binding");
    assert!(matches!(
        error,
        Error::DuplicateAlias { alias } if alias == "search"
    ));
}

#[test]
fn binding_rejects_unknown_and_duplicate_always_aliases() {
    let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
    for source in [
        "tools.always('missing')",
        "tools.need('search', 'one'); tools.always('search'); tools.always('search')",
    ] {
        let error = execute_live_tool_needs(
            &program(source),
            &resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .expect_err("invalid always declarations must fail");
        assert!(
            error.to_string().contains("not declared")
                || error.to_string().contains("more than once")
        );
    }
}

#[test]
fn captured_bindings_do_not_execute_h1_source() {
    let (_, bindings) =
        fixture_bindings("tools.need('search', 'search the web'); tools.always('search')");
    let mut vm = section_vm_with_bindings(
        &program("h1_was_executed = true"),
        &bindings,
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .expect("captured bindings must install without executing H1");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(
        &vm,
        &program("assert(h1_was_executed == nil); tools.add('search')"),
        &NullObserver,
        "Section",
    )
    .expect("captured binding must be available without H1 execution");
}

#[test]
fn h2_recording_closes_to_always_then_added_scope() {
    let (shared, bindings) = fixture_bindings(
        "tools.need('search', 'search the web'); \
             tools.need('fetch', 'fetch a page'); \
             tools.always('search')",
    );
    let prologue = program("tools.add('fetch', 'search', 'fetch')");
    let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(&vm, &prologue, &NullObserver, "Section").expect("H2 additions must record");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["search", "fetch"]
    );
}

#[test]
fn h2_add_accepts_tool_objects_and_arrays() {
    let resolver = |description: &str| {
        Ok(ToolId::new(
            "fixtures",
            if description == "search the web" {
                "search"
            } else {
                "fetch"
            },
        )
        .expect("valid id"))
    };
    let h1_error = execute_live_tool_needs(
        &program(
            "local search = tools.need('search', 'search the web'); \
                 tools.add(search)",
        ),
        &resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .expect_err("tools.add must stay H2-only even when passed a Tool object");
    assert!(
        h1_error
            .to_string()
            .contains("tools.add is only available during H2 recording"),
        "H1 tools.add(Tool) must report the phase error, not a type error: {h1_error}"
    );

    let (shared, bindings) = fixture_bindings(
        "search = tools.need('search', 'search the web'); \
             fetch = tools.need('fetch', 'fetch a page')",
    );
    let prologue = program(
        "tools.add(search); \
             tools.add({fetch}); \
             tools.add(search, 'fetch', {search, fetch})",
    );
    let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(&vm, &prologue, &NullObserver, "Section")
        .expect("tools.add must accept Tool objects, strings, and arrays");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["search", "fetch"]
    );
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn empty_add_is_a_no_op_and_failed_variadic_add_is_atomic() {
    let (shared, bindings) = fixture_bindings(
        "tools.need('search', 'search the web'); \
             tools.need('fetch', 'fetch a page')",
    );
    let prologue = program(
        "tools.add(); \
             local ok = pcall(tools.add, 'search', 'missing'); \
             if ok then error('invalid add unexpectedly succeeded') end; \
             tools.add('fetch')",
    );
    let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(&vm, &prologue, &NullObserver, "Section")
        .expect("caught failed add must not poison recording");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["fetch"],
        "empty add changes nothing and failed add records no partial aliases"
    );
}

#[test]
fn tool_operations_enforce_their_lifecycle_phase_even_when_captured() {
    let (shared, bindings) = fixture_bindings("tools.need('search', 'search the web')");
    let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");

    let error = run_scalar(
        &vm,
        &program("tools.need('other', 'fetch a page')"),
        &NullObserver,
        "Section",
    )
    .expect_err("current H2 table must reject need");
    assert!(
        error
            .to_string()
            .contains("only available during live H1 execution")
    );
}

#[test]
fn unknown_h2_alias_fails_before_scope_closure() {
    let (shared, bindings) = fixture_bindings("tools.need('search', 'search the web')");
    let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Section")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('missing')"),
        &NullObserver,
        "Section",
    )
    .expect_err("only declared aliases may enter H2 scope");
    assert!(error.to_string().contains("not declared"));
}

#[test]
fn captured_bindings_are_installed_without_payload_reports() {
    let bindings = ToolBindings::for_test(
        vec![ToolBinding::for_test(
            "private_alias",
            "private capability",
            ToolId::new("fixtures", "search").expect("valid id"),
        )],
        Vec::new(),
    );
    let recorder = Recorder::default();
    let mut vm = section_vm_with_bindings(&program(""), &bindings, EXECUTION, &recorder, "Section")
        .expect("captured binding installation must succeed");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let trace = format!("{:?}", recorder.observations());
    assert!(!trace.contains("private_alias"));
    assert!(!trace.contains("private capability"));
}

#[test]
fn section_vm_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<SectionVm>();
}

#[test]
fn section_vm_preserves_one_environment_across_all_phases() {
    let shared = program(
        "shared_saw_args = args\n\
             function decorate(value) return '<' .. value .. '>' end",
    );
    let prologue = program(
        "var.from_shared = decorate(args)\n\
             store.write('phase.txt', var.from_shared)",
    );
    let epilog =
        program("return shared_saw_args == nil and decorate(reply) or 'host leaked early'");
    let store = StoreRef::memory();
    let mut vm = SectionVm::new(Some(&shared), EXECUTION, &NullObserver, "Test")
        .expect("shared program must run");
    vm.inject_host("input", &json!({ "id": 7 }), &store, None)
        .expect("host values must inject");
    let null_observer: Arc<dyn Observer> = Arc::new(NullObserver);
    vm.install_host_apis(&null_observer, "Test")
        .expect("host APIs must install");

    assert_eq!(
        run_scalar(&vm, &prologue, &NullObserver, "Test").expect("prologue must run"),
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
            .read_lines("phase.txt")
            .expect("shared store must read_lines"),
        "1| <input>"
    );

    vm.bind_reply("model answer", &NullObserver, "Test")
        .expect("reply must bind into the same environment");
    assert_eq!(
        run_scalar(&vm, &epilog, &NullObserver, "Test")
            .expect("epilog must run")
            .as_deref(),
        Some("<model answer>")
    );
}

#[test]
fn section_vm_requires_delayed_single_host_injection() {
    let no_op = program("return args");
    let store = StoreRef::memory();
    let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");

    let error = run_scalar(&vm, &no_op, &NullObserver, "Test")
        .expect_err("programs cannot run before host injection");
    assert!(error.to_string().contains("not been injected"));

    vm.inject_host("first", &json!({}), &store, None)
        .expect("first injection must succeed");
    let error = vm
        .inject_host("second", &json!({}), &store, None)
        .expect_err("host values cannot be replaced");
    assert!(error.to_string().contains("already injected"));
}

#[test]
fn section_vm_host_injection_bypasses_shared_global_metatables() {
    let shared = program(
        "captured = {}\n\
             setmetatable(_G, { __newindex = function(_, key, value) captured[key] = value end })",
    );
    let inspect =
        program("return tostring(captured.args) .. ',' .. tostring(captured.store) .. ',' .. args");
    let mut vm = SectionVm::new(Some(&shared), EXECUTION, &NullObserver, "Test")
        .expect("shared program must run");
    vm.inject_host("private input", &json!({}), &StoreRef::memory(), None)
        .expect("raw host injection must bypass the shared metatable");

    assert_eq!(
        run_scalar(&vm, &inspect, &NullObserver, "Test")
            .expect("inspection must run")
            .as_deref(),
        Some("nil,nil,private input")
    );
}

#[test]
fn section_vm_reports_store_operations_in_each_chunk() {
    let write = program("store.write('state.txt', args)");
    let read = program("return store.read_lines('state.txt')");
    let recorder = Arc::new(Recorder::default());
    let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Gather").expect("VM must build");
    vm.inject_host("private input", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let observer: Arc<dyn Observer> = recorder.clone();
    vm.install_host_apis(&observer, "Gather")
        .expect("host APIs must install");

    run_scalar(&vm, &write, recorder.as_ref(), "Gather").expect("first chunk write must run");
    vm.bind_reply("private reply", recorder.as_ref(), "Gather")
        .expect("reply must bind");
    run_scalar(&vm, &read, recorder.as_ref(), "Gather").expect("second chunk read must run");
    vm.teardown(recorder.as_ref(), "Gather");

    assert_eq!(
        recorder.observations(),
        vec![
            ("Gather".to_owned(), detail::LUA_CHUNK_STARTED.clone(),),
            ("Gather".to_owned(), detail::STORE_WRITE_SUCCEEDED.clone(),),
            ("Gather".to_owned(), detail::LUA_CHUNK_SUCCEEDED.clone(),),
            (
                "Gather".to_owned(),
                detail::LUA_REPLY_BINDING_STARTED.clone(),
            ),
            (
                "Gather".to_owned(),
                detail::LUA_REPLY_BINDING_SUCCEEDED.clone(),
            ),
            ("Gather".to_owned(), detail::LUA_CHUNK_STARTED.clone(),),
            (
                "Gather".to_owned(),
                detail::STORE_READ_LINES_SUCCEEDED.clone(),
            ),
            ("Gather".to_owned(), detail::LUA_CHUNK_SUCCEEDED.clone(),),
            ("Gather".to_owned(), detail::LUA_TEARDOWN_STARTED.clone(),),
            ("Gather".to_owned(), detail::LUA_TEARDOWN_SUCCEEDED.clone(),),
        ]
    );
    let trace = format!("{:?}", recorder.observations());
    assert!(!trace.contains("private input"));
    assert!(!trace.contains("private reply"));
    assert!(!trace.contains("state.txt"));
}

#[test]
fn section_vm_accepts_only_scalar_top_level_returns() {
    let store = StoreRef::memory();
    for (source, expected) in [
        ("return 'text'", Some("text")),
        ("return 42", Some("42")),
        ("return 1.5", Some("1.5")),
        ("return true", Some("true")),
        ("return nil", None),
    ] {
        let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");
        vm.inject_host("", &json!({}), &store, None)
            .expect("host values must inject");
        assert_eq!(
            run_scalar(&vm, &program(source), &NullObserver, "Test")
                .expect("scalar return must work")
                .as_deref(),
            expected
        );
    }

    let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &store, None)
        .expect("host values must inject");
    let error = run_scalar(&vm, &program("return {}"), &NullObserver, "Test")
        .expect_err("table returns must be refused");
    assert!(error.to_string().contains("cannot return a table"));
}

#[test]
fn section_vms_isolate_mutated_shared_globals() {
    let shared = program("counter = 0");
    let increment = program("counter = counter + 1; return counter");
    let store = StoreRef::memory();
    let mut first = SectionVm::new(Some(&shared), EXECUTION, &NullObserver, "First")
        .expect("first VM must build");
    let mut second = SectionVm::new(Some(&shared), EXECUTION, &NullObserver, "Second")
        .expect("second VM must build");
    first
        .inject_host("", &json!({}), &store, None)
        .expect("first host must inject");
    second
        .inject_host("", &json!({}), &store, None)
        .expect("second host must inject");

    assert_eq!(
        run_scalar(&first, &increment, &NullObserver, "First")
            .expect("first increment must run")
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        run_scalar(&first, &increment, &NullObserver, "First")
            .expect("second first-VM increment must run")
            .as_deref(),
        Some("2")
    );
    assert_eq!(
        run_scalar(&second, &increment, &NullObserver, "Second")
            .expect("second VM increment must run")
            .as_deref(),
        Some("1")
    );
}

#[test]
fn shared_program_consumes_the_later_phase_instruction_budget() {
    let work = program("for i = 1, 3000000 do local value = i end");
    let mut vm = SectionVm::new(Some(&work), EXECUTION, &NullObserver, "Test")
        .expect("shared work must fit the budget");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");

    let error = run_scalar(&vm, &work, &NullObserver, "Test")
        .expect_err("the prologue must exhaust the budget left by shared execution");
    // LUA-002: an exhausted instruction budget is the typed quota error.
    assert!(
        matches!(
            error,
            Error::LuaQuota {
                resource: "instruction"
            }
        ),
        "instruction exhaustion must surface as a typed LuaQuota: {error:?}"
    );
}

#[test]
fn section_lifecycle_reports_are_ordered_exact_and_payload_free() {
    let shared = program("private_global = 'shared secret'");
    let prologue = program("var.value = args");
    let epilog = program("return reply");
    let recorder = Recorder::default();
    let mut vm = SectionVm::new(Some(&shared), EXECUTION, &recorder, "Gather")
        .expect("shared program must run");
    vm.inject_host("private input", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    run_scalar(&vm, &prologue, &recorder, "Gather").expect("prologue must run");
    vm.bind_reply("private reply", &recorder, "Gather")
        .expect("reply must bind");
    run_scalar(&vm, &epilog, &recorder, "Gather").expect("epilog must run");
    vm.teardown(&recorder, "Gather");

    let observations = recorder.observations();
    assert_eq!(
        observations,
        [
            detail::LUA_SHARED_LOAD_STARTED,
            detail::LUA_SHARED_LOAD_SUCCEEDED,
            detail::LUA_CHUNK_STARTED,
            detail::LUA_CHUNK_SUCCEEDED,
            detail::LUA_REPLY_BINDING_STARTED,
            detail::LUA_REPLY_BINDING_SUCCEEDED,
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
    assert!(!trace.contains("private reply"));
}

#[test]
fn section_lifecycle_failures_report_their_phase() {
    let recorder = Recorder::default();
    let failing_shared = program("error('private shared failure')");
    SectionVm::new(Some(&failing_shared), EXECUTION, &recorder, "Shared")
        .expect_err("shared execution must fail");
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
    let vm = SectionVm::new(None, EXECUTION, &NullObserver, "Prologue").expect("VM must build");
    run_scalar(&vm, &program("return nil"), &recorder, "Prologue")
        .expect_err("prologue before injection must fail");
    assert!(
        recorder
            .observations()
            .iter()
            .any(|(_, event)| *event == detail::LUA_CHUNK_FAILED)
    );
}

#[test]
fn lua_program_retains_source_and_round_trips_bytecode() {
    let source = "return greeting .. ' world'";
    let program = LuaProgram::compile(
        source,
        "section Gather prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Gather",
    )
    .expect("valid Lua must compile");
    assert_eq!(program.source(), source);

    for greeting in ["hello", "goodbye"] {
        let lua = Lua::new();
        lua.globals()
            .set("greeting", greeting)
            .expect("the test global must install");
        let function = program.load(&lua).expect("bytecode must load");
        let returned: String = function.call(()).expect("bytecode must execute");
        assert_eq!(returned, format!("{greeting} world"));
    }
}

#[test]
fn runtime_assert_failure_reports_chunk_name_and_line() {
    let location = "section `Web Search` epilog";
    let program = LuaProgram::compile(
        "local x = 1\nassert(false)\nreturn x",
        location,
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Web Search",
    )
    .expect("valid Lua must compile");
    let lua = Lua::new();
    let function = program.load(&lua).expect("bytecode must load");
    let error = function
        .call::<()>(())
        .expect_err("assert(false) must fail at runtime");
    let message = error.to_string();
    assert!(
        message.contains(location),
        "runtime error must name the chunk: {message}"
    );
    assert!(
        message.contains(":2:") || message.contains(":2\n"),
        "runtime error must include the failing line number: {message}"
    );
    assert!(
        !message.contains("?:"),
        "stripped debug info must not leave '?:' in the traceback: {message}"
    );
}

#[test]
fn current_sys_returns_fallback_when_unset_and_errors_on_poison() {
    // LUA-006: an unset live slot is a legitimate state and yields the
    // fallback; a poisoned lock is a real failure and must NOT masquerade as
    // the fallback.
    let vm = SectionVm::new(None, EXECUTION, &NullObserver, "Section").expect("VM must build");
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
fn map_chunk_line_to_absolute_rewrites_line_numbers() {
    let location = "section `Web Search` epilog";
    let msg = r#"[string "section `Web Search` epilog"]:2: assertion failed!"#;
    let result =
        map_chunk_line_to_absolute(msg, NonZeroU32::new(50).expect("50 is non-zero"), location);
    assert_eq!(
        result,
        r#"section `Web Search` epilog:51: [string "section `Web Search` epilog"]:51: assertion failed!"#
    );
}

#[test]
fn map_chunk_line_to_absolute_only_rewrites_matching_chunk() {
    let msg = r#"[string "section `Web Search` epilog"]:51: assertion failed!
stack traceback:
        [string "section `Main` prologue"]:3: in main chunk"#;
    let result = map_chunk_line_to_absolute(
        msg,
        NonZeroU32::new(22).expect("22 is non-zero"),
        "section `Main` prologue",
    );
    assert!(
        result.contains("[string \"section `Web Search` epilog\"]:51:"),
        "child absolute line must stay intact: {result}"
    );
    assert!(
        result.contains("[string \"section `Main` prologue\"]:24:")
            || result.starts_with("section `Main` prologue:24:"),
        "parent chunk line must map with parent source_line: {result}"
    );
    assert!(
        !result.contains("[string \"section `Main` prologue\"]:3:"),
        "parent chunk-relative line must be rewritten: {result}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_running_lua_block_cancels_cooperatively() {
    use crate::cancel::{self, CancelHandle};
    use std::time::{Duration, Instant};

    // An unbounded loop that, without cooperative cancellation, would run to
    // the instruction budget. With the cancel flag set, the very first
    // instruction-hook firing aborts it and maps to `Error::Interrupted`.
    let program = LuaProgram::compile(
        "local n = 0\nwhile true do n = n + 1 end",
        "cancel loop",
        NonZeroU32::MIN,
        EXECUTION,
        &NullObserver,
        "Loop",
    )
    .expect("an infinite loop still compiles");

    let handle = CancelHandle::new();
    handle.cancel();

    let start = Instant::now();
    let outcome = cancel::scope(handle, async {
        tokio::task::block_in_place(|| {
            let lua = Lua::new();
            install_instruction_budget(&lua);
            let func = program.load(&lua).expect("bytecode loads");
            func.call::<()>(())
                .map_err(|e| program.map_runtime_error(&e))
        })
    })
    .await;

    assert!(
        start.elapsed() < Duration::from_secs(5),
        "a cancelled Lua block must abort promptly, took {:?}",
        start.elapsed()
    );
    assert!(
        matches!(outcome, Err(crate::Error::Interrupted)),
        "expected Interrupted, got {outcome:?}"
    );
}

#[test]
fn map_chunk_line_to_absolute_keeps_original_digits_on_overflow() {
    // source_line + chunk_line - 1 must not wrap; on overflow the original
    // chunk-relative digits are preserved rather than a wrong absolute line.
    let msg = r#"[string "x"]:5: boom"#;
    let result = map_chunk_line_to_absolute(msg, NonZeroU32::MAX, "x");
    assert!(
        result.contains(r#"[string "x"]:5:"#),
        "overflowing mapping must keep the original line 5: {result}"
    );
    assert!(
        !result.contains(":4294967300:"),
        "no wrapped absolute line may appear: {result}"
    );
}

#[test]
fn map_chunk_line_to_absolute_no_match_passthrough() {
    let msg = "some other error without chunk info";
    let result = map_chunk_line_to_absolute(
        msg,
        NonZeroU32::new(10).expect("10 is non-zero"),
        "section `Main` prologue",
    );
    assert_eq!(result, msg);
}

#[test]
fn runtime_error_maps_to_absolute_prompt_line() {
    let location = "section `Web Search` epilog";
    let source_line = NonZeroU32::new(50).expect("50 is non-zero");
    let program = LuaProgram::compile(
        "local x = 1\nassert(false)\nreturn x",
        location,
        source_line,
        EXECUTION,
        &NullObserver,
        "Web Search",
    )
    .expect("valid Lua must compile");

    let lua = Lua::new();
    let function = program.load(&lua).expect("bytecode must load");
    let raw_error = function
        .call::<()>(())
        .expect_err("assert(false) must fail at runtime");

    let mapped = program.map_runtime_error(&raw_error);
    let msg = mapped.to_string();
    // chunk line 2 + source_line 50 - 1 = 51
    assert!(
        msg.contains(":51:"),
        "mapped error must contain absolute line 51: {msg}"
    );
    assert!(
        msg.contains(location),
        "mapped error must preserve the chunk name: {msg}"
    );
}

#[test]
fn malformed_lua_reports_location_and_retains_source_diagnostic() {
    let source = "local secret =\nreturn secret";
    let location = "section Gather prologue";
    let error = LuaProgram::compile(
        source,
        location,
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Gather",
    )
    .expect_err("malformed Lua must not compile");

    match &error {
        Error::LuaCompile {
            location: actual_location,
            lua_source: actual_source,
            message,
            ..
        } => {
            assert_eq!(actual_location, location);
            assert_eq!(actual_source, source);
            assert!(
                message.contains(location),
                "the Lua diagnostic must identify its source region: {message}"
            );
        }
        other => panic!("expected Error::LuaCompile, got {other:?}"),
    }
    assert!(
        error.to_string().contains(location),
        "the displayed error must identify its source region"
    );
}

#[test]
fn lua_compilation_reports_are_ordered_exact_and_payload_free() {
    let recorder = Recorder::default();
    let source = "return 'private source payload'";
    let location = "private/location";
    LuaProgram::compile(
        source,
        location,
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &recorder,
        "Gather",
    )
    .expect("valid Lua must compile");
    assert_eq!(
        recorder.observations(),
        vec![
            ("Gather".to_owned(), detail::LUA_COMPILATION_STARTED.clone(),),
            (
                "Gather".to_owned(),
                detail::LUA_COMPILATION_SUCCEEDED.clone(),
            ),
        ]
    );

    let recorder = Recorder::default();
    LuaProgram::compile(
        "local private =",
        location,
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &recorder,
        "Gather",
    )
    .expect_err("malformed Lua must fail");
    let observations = recorder.observations();
    assert_eq!(
        observations,
        vec![
            ("Gather".to_owned(), detail::LUA_COMPILATION_STARTED.clone(),),
            ("Gather".to_owned(), detail::LUA_COMPILATION_FAILED.clone(),),
        ]
    );
    let trace = format!("{observations:?}");
    assert!(!trace.contains("private"));
    assert!(!trace.contains(location));
}

#[test]
fn returns_args_verbatim() {
    assert_eq!(
        run("return args", "hello").unwrap().returned.as_deref(),
        Some("hello")
    );
}

#[test]
fn expression_only_compatibility_chunk_returns_its_value() {
    assert_eq!(run("42", "").unwrap().returned.as_deref(), Some("42"));
}

#[test]
fn no_return_is_none() {
    assert_eq!(run("local x = 1", "hello").unwrap().returned, None);
}

#[test]
fn reads_sys() {
    assert_eq!(
        run("return sys.id", "").unwrap().returned.as_deref(),
        Some("1")
    );
    assert_eq!(
        run("return sys.when", "").unwrap().returned.as_deref(),
        Some("t")
    );
}

#[test]
fn unknown_sys_field_is_a_lua_error() {
    let error = run("return sys.bogus", "").expect_err("missing sys field must fail");
    assert!(
        error.to_string().contains("unknown sys field 'bogus'"),
        "error was {error}"
    );
}

#[test]
fn writing_sys_field_is_a_lua_error() {
    let existing = run("sys.when = 'x'", "").expect_err("writing an existing sys field must fail");
    assert!(
        existing
            .to_string()
            .contains("sys is read-only; cannot set 'when'"),
        "error was {existing}"
    );

    let created = run("sys.extra = 1", "").expect_err("creating a sys field must fail");
    assert!(
        created
            .to_string()
            .contains("sys is read-only; cannot set 'extra'"),
        "error was {created}"
    );
}

#[test]
fn var_is_read_back() {
    let out = run("var.greeting = 'hi ' .. args", "bob").unwrap();
    assert_eq!(
        out.var.get("greeting").and_then(|v| v.as_str()),
        Some("hi bob")
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
fn instruction_budget_aborts_runaway() {
    assert!(run("while true do end", "").is_err());
}

#[test]
fn add_without_declarations_fails_as_undeclared_in_a_chunk() {
    let error =
        run("tools.add('web_search')", "").expect_err("an undeclared alias must fail loudly");
    assert!(
        error
            .to_string()
            .contains("tools.add alias \"web_search\" was not declared by tools.need"),
        "the error must name the undeclared alias: {error}"
    );
}

#[test]
fn add_without_declarations_fails_in_a_prologue_without_a_shared_library() {
    let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('web_search')"),
        &NullObserver,
        "Test",
    )
    .expect_err("an undeclared alias must fail loudly");
    assert!(
        error.to_string().contains("not declared by tools.need"),
        "the error must report the missing declaration: {error}"
    );
    vm.teardown(&NullObserver, "Test");
}

#[test]
fn add_with_empty_frozen_needs_fails_as_undeclared() {
    let shared = program("function helper() return 'no declarations' end");
    let resolver = |description: &str| -> Result<ToolId> {
        panic!("a declaration-free program must not resolve {description:?}")
    };
    let bindings = execute_live_tool_needs(&shared, &resolver, EXECUTION, &NullObserver, "Prompt")
        .expect("a need-free H1 program must execute");
    assert!(bindings.bindings().is_empty());
    let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Test")
        .expect("empty captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('web_search')"),
        &NullObserver,
        "Test",
    )
    .expect_err("an undeclared alias must fail loudly");
    assert!(
        error.to_string().contains("not declared by tools.need"),
        "the error must report the missing declaration: {error}"
    );
    vm.teardown(&NullObserver, "Test");
}

#[test]
fn add_with_a_description_argument_fails_alias_validation() {
    let (shared, bindings) = fixture_bindings("tools.need('search', 'search the web')");
    let mut vm = section_vm_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Test")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('search', 'Search the web for pages matching a query.')"),
        &NullObserver,
        "Test",
    )
    .expect_err("a description passed to tools.add must fail alias validation");
    assert!(
        error.to_string().contains("invalid tool alias"),
        "the error must report the invalid alias: {error}"
    );
    vm.teardown(&NullObserver, "Test");
}

#[test]
fn a_section_vm_without_declarations_snapshots_to_an_empty_scope() {
    let mut vm = SectionVm::new(None, EXECUTION, &NullObserver, "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("an empty scope must snapshot");
    assert!(scope.is_empty());
    vm.teardown(&NullObserver, "Test");
}

// --- The always-on `store` table ---

#[test]
fn store_exists_returns_boolean() {
    let store = StoreRef::memory();
    assert_eq!(
        run_with("return tostring(store.exists('missing.txt'))", &store)
            .unwrap()
            .returned
            .as_deref(),
        Some("false")
    );
    store.write("a.txt", "hi").expect("write");
    assert_eq!(
        run_with("return tostring(store.exists('a.txt'))", &store)
            .unwrap()
            .returned
            .as_deref(),
        Some("true")
    );
    assert_eq!(
        run_with(
            "store.delete('a.txt')\nreturn tostring(store.exists('a.txt'))",
            &store,
        )
        .unwrap()
        .returned
        .as_deref(),
        Some("false")
    );
}

#[test]
fn store_write_then_read_lines_returns_numbered_content() {
    let out = run(
        "store.write('a.txt', 'first\\nsecond')\nreturn store.read_lines('a.txt')",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| first\n2| second"));
}

#[test]
fn store_append_extends_the_file() {
    let out = run(
            "store.append('log.txt', 'one\\n')\nstore.append('log.txt', 'two')\nreturn store.read_lines('log.txt')",
            "",
        )
        .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| one\n2| two"));
}

#[test]
fn store_str_replace_edits_in_place() {
    let out = run(
            "store.write('a.txt', 'the quick brown fox')\nstore.str_replace('a.txt', 'quick', 'slow')\nreturn store.read_lines('a.txt')",
            "",
        )
        .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| the slow brown fox"));
}

#[test]
fn store_delete_then_read_raises() {
    let err = run(
            "store.write('a.txt', 'gone soon')\nstore.delete('a.txt')\nreturn store.read_lines('a.txt')",
            "",
        )
        .expect_err("reading a deleted file must raise");
    let msg = match &err {
        Error::Lua(msg) => msg.clone(),
        Error::LuaRuntime { message, .. } => message.clone(),
        other => panic!("expected a Lua-category error, got {other:?}"),
    };
    assert!(
        msg.contains("file not found"),
        "the Lua error must carry the store message, got: {msg}"
    );
}

#[test]
fn store_glob_returns_a_sorted_array() {
    let out = run(
            "store.write('src/b.rs', '')\nstore.write('src/a.rs', '')\nlocal g = store.glob('src/*.rs')\nreturn g[1] .. ',' .. g[2]",
            "",
        )
        .unwrap();
    assert_eq!(out.returned.as_deref(), Some("src/a.rs,src/b.rs"));
}

#[test]
fn store_error_surfaces_as_lua_error() {
    // An ambiguous `str_replace` anchor is a `StoreError`, which must reach
    // the caller as `Error::Lua` (mapped through `mlua::Error::external`).
    let err = run(
        "store.write('a.txt', 'na na na')\nstore.str_replace('a.txt', 'na', 'la')",
        "",
    )
    .expect_err("an ambiguous anchor must raise");
    let msg = match &err {
        Error::Lua(msg) => msg.clone(),
        Error::LuaRuntime { message, .. } => message.clone(),
        other => panic!("expected a Lua-category error, got {other:?}"),
    };
    assert!(
        msg.contains("expected exactly one"),
        "the Lua error must carry the ambiguity message, got: {msg}"
    );
}

#[test]
fn lua_runtime_error_preserves_its_mlua_source() {
    // F4: a Lua runtime failure is the source-bearing `LuaRuntime` variant and
    // retains the originating `mlua` error as a private `source()` instead of
    // flattening it to a string.
    let err = run("error('boom')", "").expect_err("an explicit error() must raise");
    assert!(
        matches!(err, Error::LuaRuntime { .. }),
        "a Lua runtime failure must use the source-bearing variant, got {err:?}"
    );
    assert!(
        std::error::Error::source(&err).is_some(),
        "the originating mlua error must be preserved as the error source"
    );
}

#[test]
fn store_writes_are_visible_on_the_shared_handle() {
    // The table is backed by the caller's handle, so a write from Lua is
    // observable through a clone of that same handle after the chunk ends.
    let store = StoreRef::memory();
    run_with("store.write('shared.txt', 'from lua')", &store).unwrap();
    assert_eq!(
        store.read_lines("shared.txt").expect("read_lines"),
        "1| from lua",
        "a Lua write must land in the shared store"
    );
}

#[test]
fn store_reports_are_ordered_exact_and_payload_free_on_failure() {
    let recorder = Recorder::default();
    let store = StoreRef::memory();
    let source = "store.write('secret/path.txt', 'private contents')\n\
                      store.read_lines('secret/path.txt')\n\
                      store.str_replace('secret/path.txt', 'missing secret', 'replacement')";
    let error = run_chunk(
        source,
        "private input",
        &json!({ "id": 1, "when": "t" }),
        &store,
        EXECUTION,
        &recorder,
        "Gather",
    )
    .expect_err("the missing anchor must fail");
    assert!(matches!(error, Error::Lua(_) | Error::LuaRuntime { .. }));

    let observations = recorder.observations();
    assert_eq!(
        observations,
        vec![
            ("Gather".to_string(), detail::STORE_WRITE_SUCCEEDED.clone()),
            (
                "Gather".to_string(),
                detail::STORE_READ_LINES_SUCCEEDED.clone(),
            ),
            ("Gather".to_string(), detail::STORE_REPLACE_FAILED.clone()),
        ]
    );
    let trace = format!("{observations:?}");
    for payload in [
        "secret/path.txt",
        "private contents",
        "missing secret",
        "replacement",
        "private input",
    ] {
        assert!(
            !trace.contains(payload),
            "observation leaked payload {payload:?}: {trace}"
        );
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "parametric coverage of all store ops"
)]
fn every_store_operation_reports_its_exact_success_and_failure() {
    struct Case {
        source: &'static str,
        success: Observation,
        failure: Observation,
        prepare: fn(&StoreRef),
    }

    fn empty(_store: &StoreRef) {}

    fn existing(store: &StoreRef) {
        store
            .write("a.txt", "old")
            .expect("the memory store can prepare a file");
    }

    let cases = [
        Case {
            source: "store.write('a.txt', 'new')",
            success: detail::STORE_WRITE_SUCCEEDED,
            failure: detail::STORE_WRITE_FAILED,
            prepare: empty,
        },
        Case {
            source: "store.append('a.txt', 'new')",
            success: detail::STORE_APPEND_SUCCEEDED,
            failure: detail::STORE_APPEND_FAILED,
            prepare: empty,
        },
        Case {
            source: "store.read_lines('a.txt')",
            success: detail::STORE_READ_LINES_SUCCEEDED,
            failure: detail::STORE_READ_LINES_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.read('a.txt')",
            success: detail::STORE_READ_SUCCEEDED,
            failure: detail::STORE_READ_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.inject('a.txt')",
            success: detail::STORE_INJECT_SUCCEEDED,
            failure: detail::STORE_INJECT_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.str_replace('a.txt', 'old', 'new')",
            success: detail::STORE_REPLACE_SUCCEEDED,
            failure: detail::STORE_REPLACE_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.delete('a.txt')",
            success: detail::STORE_DELETE_SUCCEEDED,
            failure: detail::STORE_DELETE_FAILED,
            prepare: existing,
        },
        Case {
            source: "local matches = store.glob('*.txt')",
            success: detail::STORE_GLOB_SUCCEEDED,
            failure: detail::STORE_GLOB_FAILED,
            prepare: existing,
        },
    ];

    for case in cases {
        let store = StoreRef::memory();
        (case.prepare)(&store);
        let recorder = Recorder::default();
        run_chunk(
            case.source,
            "",
            &json!({}),
            &store,
            EXECUTION,
            &recorder,
            "StoreRef",
        )
        .expect("the memory store operation succeeds");
        assert_eq!(
            recorder.observations(),
            vec![("StoreRef".to_owned(), case.success.clone())],
            "wrong success observation for {}",
            case.source
        );

        let store = StoreRef::new(Box::new(FailingStore));
        let recorder = Recorder::default();
        let error = run_chunk(
            case.source,
            "",
            &json!({}),
            &store,
            EXECUTION,
            &recorder,
            "StoreRef",
        )
        .expect_err("the failing backend rejects every operation");
        assert!(matches!(error, Error::Lua(_) | Error::LuaRuntime { .. }));
        assert_eq!(
            recorder.observations(),
            vec![("StoreRef".to_owned(), case.failure.clone())],
            "wrong failure observation for {}",
            case.source
        );
    }
}

#[test]
fn store_observations_happen_before_later_lua_side_effects() {
    let store = StoreRef::memory();
    let recorder = BoundaryRecorder {
        store: store.clone(),
        snapshots: Mutex::new(Vec::new()),
    };

    run_chunk(
        "store.write('first.txt', '')\nstore.write('second.txt', '')",
        "",
        &json!({}),
        &store,
        EXECUTION,
        &recorder,
        "StoreRef",
    )
    .expect("both writes succeed");

    assert_eq!(
        *recorder
            .snapshots
            .lock()
            .expect("the snapshot mutex must not be poisoned"),
        vec![
            vec!["first.txt".to_owned()],
            vec!["first.txt".to_owned(), "second.txt".to_owned()],
        ]
    );
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
fn untrusted_global_mints_a_fresh_nonce_per_call() {
    let outcome = run(
        "return untrusted('same') .. '\\n@@SPLIT@@\\n' .. untrusted('same')",
        "",
    )
    .expect("untrusted must run");
    let wrapped = outcome.returned.expect("two envelopes");
    let (first, second) = wrapped.split_once("\n@@SPLIT@@\n").expect("two envelopes");
    assert_ne!(first, second, "each call must mint a fresh nonce");
}

#[test]
fn untrusted_global_is_callable_from_the_shared_library() {
    let shared = program(
        "local wrapped = untrusted('a < b')\n\
         assert(wrapped:find('a &lt; b', 1, true), 'shared sees the escaped body')",
    );
    let vm = SectionVm::new(Some(&shared), EXECUTION, &NullObserver, "Test")
        .expect("the shared library must call untrusted during load");
    vm.teardown(&NullObserver, "Test");
}

#[test]
fn untrusted_global_rejects_a_non_string_argument() {
    let error = run("return untrusted({})", "").expect_err("a table is not a string");
    assert!(
        matches!(error, Error::Lua(_) | Error::LuaRuntime { .. }),
        "a non-string argument must surface as a Lua error, got {error:?}"
    );
}
