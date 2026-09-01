use std::sync::{Arc, Mutex};

use super::*;
use crate::program::map_chunk_line_to_absolute;
use promptforge_core_support::observe::{NullObserver, Observation};
use promptforge_store::{Store, StoreError};
use promptforge_tools::{Tool, ToolError, ToolOutput};
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

/// Returns the message carried by either Lua-category error representation.
fn lua_error_message(error: &Error) -> &str {
    match error {
        Error::Lua(message) | Error::LuaRuntime { message, .. } => message,
        other => panic!("expected a Lua-category error, got {other:?}"),
    }
}

#[derive(Debug)]
struct FailingStore;

impl FailingStore {
    fn error(path: &str) -> StoreError {
        StoreError::not_found(path)
    }
}

impl Store for FailingStore {
    fn write(&mut self, path: &str, _contents: &str) -> std::result::Result<(), StoreError> {
        Err(Self::error(path))
    }

    fn append(&mut self, path: &str, _contents: &str) -> std::result::Result<(), StoreError> {
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
        &null_observer(),
        "Test",
    )
}

/// Mints the guard nonce for a test-owned VM; every wrap that VM's
/// `untrusted` global performs shares it, matching the per-run nonce the
/// executor mints.
fn test_nonce() -> GuardNonce {
    GuardNonce::fresh()
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
        &null_observer(),
        "Test",
    )
}

/// A null observer in the owned form the persistent host-API install takes.
fn null_observer() -> Arc<dyn Observer> {
    Arc::new(NullObserver::default())
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
        &NullObserver::default(),
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

fn execute_live_tool_binds(
    source: &LuaProgram,
    resolver: &dyn ToolResolver,
    _execution: &str,
    _observer: &dyn Observer,
    _section: &str,
) -> Result<ToolSet> {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(FixtureTool("search")),
        Arc::new(FixtureTool("fetch")),
    ];
    let catalog = ToolCatalog::new(&tools).expect("unique test catalog");
    let models = |description: &str, _: &promptforge_model_client::model::ModelBindOpts| {
        Err(promptforge_model_client::Error::ModelAbsent {
            capability: description.to_owned(),
        })
    };
    let producer = LiveBindingProducer::new(
        Arc::new(Mutex::new(ToolSet::default())),
        Arc::new(Mutex::new(ModelSet::default())),
    );
    let lua = Lua::new();
    harden(&lua)?;
    let result = lua.scope(|scope| {
        producer
            .install(&lua, scope, resolver, &catalog, &models)
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
    bindings: &ToolSet,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<SectionVm> {
    let vm = SectionVm::new_for_section(
        &test_nonce(),
        bindings,
        &ModelSet::default(),
        execution,
        observer,
        section,
    )?;
    vm.install_captured_bindings()?;
    Ok(vm)
}

/// Builds a section VM through the engine's startup order for a shared
/// library: construction, host injection, persistent host APIs, then the
/// shared replay. Tests that need control globals or captured bindings add
/// them by hand.
fn section_vm_with_shared(
    shared: &LuaProgram,
    args: &str,
    store: &StoreRef,
    observer: &Arc<dyn Observer>,
    section: &str,
) -> Result<SectionVm> {
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, observer.as_ref(), section)?;
    vm.inject_host(args, &json!({}), store, None)?;
    vm.install_host_apis(observer, section)?;
    vm.replay_shared(shared, observer.as_ref(), section)?;
    Ok(vm)
}

fn fixture_bindings(source: &str) -> ToolSet {
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
    execute_live_tool_binds(
        &shared,
        &resolver,
        EXECUTION,
        &NullObserver::default(),
        "Prompt",
    )
    .expect("fixture binds must resolve")
}

#[test]
fn direct_output_is_absent_in_every_executable_lua_vm() {
    let library = program("assert(print == nil); assert(warn == nil); log('library load')");
    let library_vm = section_vm_with_shared(
        &library,
        "",
        &StoreRef::memory(),
        &null_observer(),
        "Section",
    )
    .expect("library VM must not expose direct output");
    library_vm.teardown(&NullObserver::default(), "Section");

    let shared = program(
        "assert(print == nil)\n\
             assert(warn == nil)\n\
             tools.bind('search', 'search the web')",
    );
    let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
    let bindings = execute_live_tool_binds(
        &shared,
        &resolver,
        EXECUTION,
        &NullObserver::default(),
        "Prompt",
    )
    .expect("live H1 VM must not expose direct output");
    let mut vm =
        section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
            .expect("section VM must not expose direct output");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(
        &vm,
        &program("assert(print == nil); assert(warn == nil)"),
        &NullObserver::default(),
        "Section",
    )
    .expect("prologue must not expose direct output");
    run_scalar(
        &vm,
        &program("assert(print == nil); assert(warn == nil)"),
        &NullObserver::default(),
        "Section",
    )
    .expect("epilog must not expose direct output");
    vm.teardown(&NullObserver::default(), "Section");

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
    let bindings = ToolSet::for_test(
        vec![ToolBinding::for_test(
            "search",
            "search the web",
            Arc::new(FixtureTool("search")),
        )],
        Vec::new(),
    );
    let mut vm = section_vm_with_bindings(&bindings, EXECUTION, recorder.as_ref(), "Gather")
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

    assert_eq!(
        recorder.records(),
        [
            (
                EXECUTION.to_owned(),
                "Gather".to_owned(),
                detail::LUA_CHUNK_STARTED,
            ),
            (
                EXECUTION.to_owned(),
                "Gather".to_owned(),
                Observation::Lua("prologue checkpoint".to_owned()),
            ),
            (
                EXECUTION.to_owned(),
                "Gather".to_owned(),
                detail::LUA_CHUNK_SUCCEEDED,
            ),
            (
                EXECUTION.to_owned(),
                "Gather".to_owned(),
                detail::LUA_CHUNK_STARTED,
            ),
            (
                EXECUTION.to_owned(),
                "Gather".to_owned(),
                Observation::Lua("epilog checkpoint".to_owned()),
            ),
            (
                EXECUTION.to_owned(),
                "Gather".to_owned(),
                detail::LUA_CHUNK_SUCCEEDED,
            ),
            (
                EXECUTION.to_owned(),
                "Gather".to_owned(),
                detail::LUA_TEARDOWN_STARTED,
            ),
            (
                EXECUTION.to_owned(),
                "Gather".to_owned(),
                detail::LUA_TEARDOWN_SUCCEEDED,
            ),
        ]
    );
}

#[test]
fn compatibility_chunk_logs_interleave_with_host_operations() {
    let recorder = Arc::new(Recorder::default());
    let observer: Arc<dyn Observer> = recorder.clone();
    run_chunk(
        "log('before write')\n\
             store.write('state.txt', 'value')\n\
             log('after write')",
        "",
        &json!({}),
        &StoreRef::memory(),
        "compatibility-run",
        &observer,
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
        let recorder = Arc::new(Recorder::default());
        let observer: Arc<dyn Observer> = recorder.clone();
        let error = run_chunk(
            source,
            "",
            &json!({}),
            &StoreRef::memory(),
            EXECUTION,
            &observer,
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
    let recorder = Arc::new(Recorder::default());
    let observer: Arc<dyn Observer> = recorder.clone();
    run_chunk(
        &source,
        "",
        &json!({}),
        &StoreRef::memory(),
        EXECUTION,
        &observer,
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
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Budget")
        .expect("VM builds");
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
    vm.teardown(&NullObserver::default(), "Budget");
}

#[test]
fn logging_does_not_change_results_or_store_effects_with_null_observer() {
    let source = "log('checkpoint')\n\
                      var.answer = args\n\
                      store.write('answer.txt', args)\n\
                      return var.answer";
    let recorded_store = StoreRef::memory();
    let recorder = Arc::new(Recorder::default());
    let observer: Arc<dyn Observer> = recorder.clone();
    let observed_outcome = run_chunk(
        source,
        "same",
        &json!({}),
        &recorded_store,
        EXECUTION,
        &observer,
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
        &null_observer(),
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
    let mut vm = SectionVm::new(
        &test_nonce(),
        EXECUTION,
        &NullObserver::default(),
        "Section",
    )
    .expect("VM must construct");
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
            let observer: Arc<dyn Observer> = recorder.clone();
            run_chunk(
                "log('first'); log('second')",
                "",
                &json!({}),
                &StoreRef::memory(),
                execution,
                &observer,
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
    let source = "tools.bind('web_search', 'search the web')\n\
                      tools.bind('web_fetch2', 'fetch a page')\n\
                      tools.always('web_search')";
    let bindings = fixture_bindings(source);

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
fn bind_and_always_record_model_description_overrides() {
    let bindings = fixture_bindings(
        "tools.bind('web_search', 'search the web', 'bind override')\n\
             tools.bind('web_fetch2', 'fetch a page')\n\
             tools.always('web_fetch2', 'always override')",
    );

    assert_eq!(
        bindings.bindings()[0].model_description(),
        Some("bind override"),
        "tools.bind's third argument records the model-facing override"
    );
    assert_eq!(
        bindings.bindings()[1].model_description(),
        Some("always override"),
        "tools.always's second argument updates the recorded override"
    );
}

#[test]
fn tool_handles_are_frozen() {
    let bindings = fixture_bindings("search = tools.bind('search', 'search the web')");
    let mut vm =
        section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
            .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let error = run_scalar(
        &vm,
        &program("search.description = 'x'"),
        &NullObserver::default(),
        "Section",
    )
    .expect_err("assigning .description on a Tool object must fail");
    assert!(
        error.to_string().contains("description"),
        "the error must name the frozen field: {error}"
    );
    vm.teardown(&NullObserver::default(), "Section");
}

#[test]
fn tool_bind_returns_inspectable_object() {
    let shared = program(
        "local tool = tools.bind('search', 'search the web')\n\
             assert(tool.name == 'search')\n\
             assert(tool.description == 'search the web')\n\
             assert(type(tool.parameters) == 'table')\n\
             assert(tool.wire_name == 'search')\n\
             assert(tool.untrusted == false)\n\
             tools.always('search')",
    );
    let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
    let bindings = execute_live_tool_binds(
        &shared,
        &resolver,
        EXECUTION,
        &NullObserver::default(),
        "Prompt",
    )
    .expect("tools.bind must return an inspectable Tool object");
    assert_eq!(bindings.bindings()[0].alias(), "search");

    let vm = section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
        .expect("section install must expose the same inspectable Tool object");
    vm.teardown(&NullObserver::default(), "Section");
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
        let bind = program(&format!("tools.bind({alias:?}, 'capability')"));
        let error = execute_live_tool_binds(
            &bind,
            &resolver,
            EXECUTION,
            &NullObserver::default(),
            "Prompt",
        )
        .expect_err("invalid aliases must be rejected");
        assert!(
            error.to_string().contains("invalid tool alias"),
            "wrong error for {alias:?}: {error}"
        );
    }

    for valid in ["Upper", "has-dash", &format!("A{}", "2".repeat(63))] {
        let bind = program(&format!("tools.bind({valid:?}, 'capability')"));
        execute_live_tool_binds(
            &bind,
            &resolver,
            EXECUTION,
            &NullObserver::default(),
            "Prompt",
        )
        .expect("planned alias forms must be valid");
    }
}

#[test]
fn live_h1_rejects_duplicate_aliases() {
    let resolver = |_: &str| Ok(ToolId::new("fixtures", "search").expect("valid id"));
    let error = execute_live_tool_binds(
        &program("tools.bind('search', 'one'); tools.bind('search', 'two')"),
        &resolver,
        EXECUTION,
        &NullObserver::default(),
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
    let error = execute_live_tool_binds(
        &program("tools.bind('search', 'one'); pcall(tools.bind, 'search', 'two')"),
        &resolver,
        EXECUTION,
        &NullObserver::default(),
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
    for (source, expected) in [
        (
            "tools.always('missing')",
            "tools.always alias \"missing\" was not declared by tools.bind",
        ),
        (
            "tools.bind('search', 'one'); tools.always('search'); tools.always('search')",
            "tools.always alias \"search\" was recorded more than once",
        ),
    ] {
        let error = execute_live_tool_binds(
            &program(source),
            &resolver,
            EXECUTION,
            &NullObserver::default(),
            "Prompt",
        )
        .expect_err("invalid always declarations must fail");
        assert!(
            error.to_string().contains(expected),
            "error must identify the rejected always declaration: {error}"
        );
    }
}

#[test]
fn captured_bindings_do_not_execute_h1_source() {
    let bindings = fixture_bindings(
        "h1_was_executed = true; \
         tools.bind('search', 'search the web'); \
         tools.always('search')",
    );
    let mut vm =
        section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
            .expect("captured bindings must install without executing H1");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(
        &vm,
        &program("assert(h1_was_executed == nil); tools.add('search')"),
        &NullObserver::default(),
        "Section",
    )
    .expect("captured binding must be available without H1 execution");
}

#[test]
fn h2_recording_closes_to_always_then_added_scope() {
    let bindings = fixture_bindings(
        "tools.bind('search', 'search the web'); \
             tools.bind('fetch', 'fetch a page'); \
             tools.always('search')",
    );
    let prologue = program("tools.add({'fetch', 'search'})");
    let mut vm =
        section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
            .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(&vm, &prologue, &NullObserver::default(), "Section")
        .expect("H2 additions must record");
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
    let h1_error = execute_live_tool_binds(
        &program(
            "local search = tools.bind('search', 'search the web'); \
                 tools.add(search)",
        ),
        &resolver,
        EXECUTION,
        &NullObserver::default(),
        "Prompt",
    )
    .expect_err("tools.add must stay H2-only even when passed a Tool object");
    assert!(
        h1_error
            .to_string()
            .contains("tools.add is only available during H2 recording"),
        "H1 tools.add(Tool) must report the phase error, not a type error: {h1_error}"
    );

    let bindings = fixture_bindings(
        "search = tools.bind('search', 'search the web'); \
             fetch = tools.bind('fetch', 'fetch a page')",
    );
    let prologue = program(
        "tools.add(search); \
             tools.add({fetch}); \
             tools.add({'fetch', search})",
    );
    let mut vm =
        section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
            .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(&vm, &prologue, &NullObserver::default(), "Section")
        .expect("tools.add must accept Tool objects, strings, and arrays");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");

    assert_eq!(
        scope.iter().map(ToolBinding::alias).collect::<Vec<_>>(),
        ["search", "fetch"]
    );
    vm.teardown(&NullObserver::default(), "Section");
}

#[test]
fn empty_add_is_a_no_op_and_failed_bulk_add_is_atomic() {
    let bindings = fixture_bindings(
        "tools.bind('search', 'search the web'); \
             tools.bind('fetch', 'fetch a page')",
    );
    let prologue = program(
        "tools.add(); \
             local ok = pcall(tools.add, {'search', 'missing'}); \
             if ok then error('invalid add unexpectedly succeeded') end; \
             tools.add('fetch')",
    );
    let mut vm =
        section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
            .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(&vm, &prologue, &NullObserver::default(), "Section")
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
fn add_rejects_misshapen_override_arguments() {
    let bindings = fixture_bindings("tools.bind('search', 'search the web')");
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
    let mut vm =
        section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
            .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    run_scalar(&vm, &prologue, &NullObserver::default(), "Section")
        .expect("rejected override forms must not poison recording");
    let (bindings, runtime) = vm.tool_bag_handles();
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
    vm.teardown(&NullObserver::default(), "Section");
}

#[test]
fn tool_operations_enforce_their_lifecycle_phase_even_when_captured() {
    let bindings = fixture_bindings("tools.bind('search', 'search the web')");
    let mut vm =
        section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
            .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");

    let error = run_scalar(
        &vm,
        &program("tools.bind('other', 'fetch a page')"),
        &NullObserver::default(),
        "Section",
    )
    .expect_err("current H2 table must reject bind");
    assert!(
        error
            .to_string()
            .contains("only available during live H1 execution")
    );
}

#[test]
fn unknown_h2_alias_fails_before_scope_closure() {
    let bindings = fixture_bindings("tools.bind('search', 'search the web')");
    let mut vm =
        section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Section")
            .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('missing')"),
        &NullObserver::default(),
        "Section",
    )
    .expect_err("only declared aliases may enter H2 scope");
    assert!(error.to_string().contains("not declared"));
}

#[test]
fn captured_bindings_are_installed_without_payload_reports() {
    let bindings = ToolSet::for_test(
        vec![ToolBinding::for_test(
            "private_alias",
            "private capability",
            Arc::new(FixtureTool("search")),
        )],
        Vec::new(),
    );
    let recorder = Recorder::default();
    let mut vm = section_vm_with_bindings(&bindings, EXECUTION, &recorder, "Section")
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
    // The shared library replays as the section's first chunk with the full
    // host environment installed, so its top level reads `args` and `store`
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
    let epilog =
        program("return decorate(reply) .. ':' .. shared_saw_args .. ':' .. shared_saw_store");
    let store = StoreRef::memory();
    store
        .write("seed.txt", "seeded")
        .expect("the memory store can seed a file");
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");
    vm.inject_host("input", &json!({ "id": 7 }), &store, None)
        .expect("host values must inject");
    let null_observer: Arc<dyn Observer> = Arc::new(NullObserver::default());
    vm.install_host_apis(&null_observer, "Test")
        .expect("host APIs must install");
    vm.replay_shared(&shared, &NullObserver::default(), "Test")
        .expect("shared program must run with the full environment");

    assert_eq!(
        run_scalar(&vm, &prologue, &NullObserver::default(), "Test").expect("prologue must run"),
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
        store.read("phase.txt").expect("shared store must read"),
        "<input>"
    );

    vm.bind_reply("model answer", &NullObserver::default(), "Test")
        .expect("reply must bind into the same environment");
    assert_eq!(
        run_scalar(&vm, &epilog, &NullObserver::default(), "Test")
            .expect("epilog must run")
            .as_deref(),
        Some("<model answer>:input:seeded")
    );
}

#[test]
fn section_vm_requires_delayed_single_host_injection() {
    let no_op = program("return args");
    let store = StoreRef::memory();
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");

    let error = run_scalar(&vm, &no_op, &NullObserver::default(), "Test")
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
    // Host values inject before the shared replay, and the captured alias
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
            Arc::new(FixtureTool("search")),
        )],
        Vec::new(),
    );
    let mut vm = SectionVm::new_for_section(
        &test_nonce(),
        &bindings,
        &ModelSet::default(),
        EXECUTION,
        &NullObserver::default(),
        "Test",
    )
    .expect("VM must build");
    vm.inject_host("private input", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let observer = null_observer();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.replay_shared(&shared, &NullObserver::default(), "Test")
        .expect("shared program must run");
    vm.install_captured_bindings()
        .expect("captured bindings must install");

    assert_eq!(
        run_scalar(&vm, &inspect, &NullObserver::default(), "Test")
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
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Gather")
        .expect("VM must build");
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
            ("Gather".to_owned(), detail::STORE_READ_SUCCEEDED.clone(),),
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
        let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
            .expect("VM must build");
        vm.inject_host("", &json!({}), &store, None)
            .expect("host values must inject");
        assert_eq!(
            run_scalar(&vm, &program(source), &NullObserver::default(), "Test")
                .expect("scalar return must work")
                .as_deref(),
            expected
        );
    }

    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");
    vm.inject_host("", &json!({}), &store, None)
        .expect("host values must inject");
    let error = run_scalar(&vm, &program("return {}"), &NullObserver::default(), "Test")
        .expect_err("table returns must be refused");
    assert!(error.to_string().contains("cannot return a table"));
}

#[test]
fn section_vms_isolate_mutated_shared_globals() {
    let shared = program("counter = 0");
    let increment = program("counter = counter + 1; return counter");
    let store = StoreRef::memory();
    let first = section_vm_with_shared(&shared, "", &store, &null_observer(), "First")
        .expect("first VM must build");
    let second = section_vm_with_shared(&shared, "", &store, &null_observer(), "Second")
        .expect("second VM must build");

    assert_eq!(
        run_scalar(&first, &increment, &NullObserver::default(), "First")
            .expect("first increment must run")
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        run_scalar(&first, &increment, &NullObserver::default(), "First")
            .expect("second first-VM increment must run")
            .as_deref(),
        Some("2")
    );
    assert_eq!(
        run_scalar(&second, &increment, &NullObserver::default(), "Second")
            .expect("second VM increment must run")
            .as_deref(),
        Some("1")
    );
}

#[test]
fn shared_program_consumes_the_later_phase_instruction_budget() {
    // The replay shares the section VM's single instruction counter, so work
    // the shared library does at load shrinks the budget left for chunks.
    let work = program("for i = 1, 3000000 do local value = i end");
    let vm = section_vm_with_shared(&work, "", &StoreRef::memory(), &null_observer(), "Test")
        .expect("shared work must fit the budget");

    let error = run_scalar(&vm, &work, &NullObserver::default(), "Test")
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
fn shared_replay_consumes_the_configured_log_budget() {
    // `apply_lua_limits` lands before the replay, so the replay spends the
    // configured log budget rather than the construction defaults.
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Budget")
        .expect("VM builds");
    vm.apply_lua_limits(DEFAULT_LUA_MEMORY_BYTES, 1)
        .expect("limits apply");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host injects");
    let observer = null_observer();
    vm.install_host_apis(&observer, "Budget")
        .expect("host APIs must install");
    let error = vm
        .replay_shared(
            &program("log('one')\nlog('two')"),
            &NullObserver::default(),
            "Budget",
        )
        .expect_err("the second log must exhaust the configured budget");
    assert!(
        matches!(
            error,
            Error::LuaQuota {
                resource: "log event"
            }
        ),
        "log-budget exhaustion must surface as a typed LuaQuota: {error:?}"
    );
    vm.teardown(&NullObserver::default(), "Budget");
}

#[test]
fn jump_during_shared_replay_is_a_hard_error() {
    // Load-time control transfer has no section walk to transfer into, so a
    // recorded jump fails the replay outright.
    let shared = program("jump('## Anywhere')");
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let observer = null_observer();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.install_control_globals(
        |_, _, _| Err(Error::Lua("execute is not needed here".to_owned())),
        |_, _, _| Err(Error::Lua("fanout is not needed here".to_owned())),
        |_| {
            Err(Error::Lua(
                "list_from_section is not needed here".to_owned(),
            ))
        },
    )
    .expect("control globals must install");
    let error = vm
        .replay_shared(&shared, &NullObserver::default(), "Test")
        .expect_err("jump during the shared replay must fail");
    assert!(
        error
            .to_string()
            .contains("jump is not available during shared library load"),
        "the hard error must name the phase: {error}"
    );
    vm.teardown(&NullObserver::default(), "Test");
}

#[test]
fn execute_with_a_non_string_target_errors() {
    // The control callback resolves its target through the same
    // `resolve_section_target` boundary as the engine: a number is not a
    // heading, and the error says so.
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let observer = null_observer();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.install_control_globals(
        |target, _, _| resolve_section_target(target).map_err(Error::lua),
        |_, _, _| Err(Error::Lua("fanout is not needed here".to_owned())),
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
            "local ok, err = pcall(execute, 42)\n\
             assert(not ok and tostring(err):find('section target must be a string'), tostring(err))\n\
             return 'ok'",
        ),
        &NullObserver::default(),
        "Test",
    )
    .expect("a non-string execute target must error");
    assert_eq!(out.as_deref(), Some("ok"));
    vm.teardown(&NullObserver::default(), "Test");
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
            Arc::new(FixtureTool("search")),
        )],
        Vec::new(),
    );
    let shared = program(
        "tools.add('search')\n\
         assert(search == nil, 'the bare alias global installs after the replay')",
    );
    let mut vm = SectionVm::new_for_section(
        &test_nonce(),
        &bindings,
        &ModelSet::default(),
        EXECUTION,
        &NullObserver::default(),
        "Test",
    )
    .expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let observer = null_observer();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.replay_shared(&shared, &NullObserver::default(), "Test")
        .expect("the tools table must work during the shared replay");
    vm.install_captured_bindings()
        .expect("captured bindings must install");

    assert_eq!(
        run_scalar(
            &vm,
            &program("return type(search)"),
            &NullObserver::default(),
            "Test"
        )
        .expect("the alias global installs after the replay")
        .as_deref(),
        Some("userdata")
    );
    let (bindings, runtime) = vm.tool_bag_handles();
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
            Arc::new(FixtureTool("search")),
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
        &bindings,
        &ModelSet::default(),
        EXECUTION,
        &NullObserver::default(),
        "Test",
    )
    .expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let observer = null_observer();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.replay_shared(&shared, &NullObserver::default(), "Test")
        .expect("shared library must load");
    vm.install_captured_bindings()
        .expect("captured bindings must install");

    assert_eq!(
        run_scalar(
            &vm,
            &program("return scope_and_store('search')"),
            &NullObserver::default(),
            "Test",
        )
        .expect("the shared function must mutate host state when called")
        .as_deref(),
        Some("search")
    );
    let (bindings, runtime) = vm.tool_bag_handles();
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
    let mut vm =
        SectionVm::new(&test_nonce(), EXECUTION, recorder.as_ref(), "Test").expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let observer: Arc<dyn Observer> = recorder.clone();
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");
    vm.replay_shared(
        &LuaProgram::empty().expect("the empty chunk compiles"),
        recorder.as_ref(),
        "Test",
    )
    .expect("the empty replay must succeed");
    assert_eq!(
        run_scalar(&vm, &program("return 42"), recorder.as_ref(), "Test")
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

#[test]
fn section_lifecycle_reports_are_ordered_exact_and_payload_free() {
    let shared = program("private_global = 'shared secret'");
    let prologue = program("var.value = args");
    let epilog = program("return reply");
    let recorder = Arc::new(Recorder::default());
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, recorder.as_ref(), "Gather")
        .expect("VM must build");
    vm.inject_host("private input", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let observer: Arc<dyn Observer> = recorder.clone();
    vm.install_host_apis(&observer, "Gather")
        .expect("host APIs must install");
    vm.replay_shared(&shared, recorder.as_ref(), "Gather")
        .expect("shared program must run");
    run_scalar(&vm, &prologue, recorder.as_ref(), "Gather").expect("prologue must run");
    vm.bind_reply("private reply", recorder.as_ref(), "Gather")
        .expect("reply must bind");
    run_scalar(&vm, &epilog, recorder.as_ref(), "Gather").expect("epilog must run");
    vm.teardown(recorder.as_ref(), "Gather");

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
    let recorder = Arc::new(Recorder::default());
    let failing_shared = program("error('private shared failure')");
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, recorder.as_ref(), "Shared")
        .expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let observer: Arc<dyn Observer> = recorder.clone();
    vm.install_host_apis(&observer, "Shared")
        .expect("host APIs must install");
    vm.replay_shared(&failing_shared, recorder.as_ref(), "Shared")
        .expect_err("shared execution must fail");
    vm.teardown(recorder.as_ref(), "Shared");
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
    let vm = SectionVm::new(
        &test_nonce(),
        EXECUTION,
        &NullObserver::default(),
        "Prologue",
    )
    .expect("VM must build");
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
        &NullObserver::default(),
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
        &NullObserver::default(),
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
    let vm = SectionVm::new(
        &test_nonce(),
        EXECUTION,
        &NullObserver::default(),
        "Section",
    )
    .expect("VM must build");
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
fn local_tools_schema_and_membership_reads_fail_closed_on_poison() {
    let local = LocalTools::default();
    let handle = local.entries_handle();
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = handle.lock().expect("first lock is not poisoned");
        panic!("poison the local tools registry");
    }));
    assert!(poisoned.is_err(), "the panic must poison the registry");

    let schemas = local
        .schemas()
        .expect_err("schema reads must fail on a poisoned registry");
    let contains = local
        .contains("anything")
        .expect_err("membership reads must fail on a poisoned registry");
    for error in [schemas, contains] {
        assert!(
            error
                .to_string()
                .contains("local tools registry was poisoned"),
            "the concrete poison error must surface: {error}"
        );
    }
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
    use promptforge_core_support::cancel::{self, CancelHandle};
    use std::time::{Duration, Instant};

    // An unbounded loop that, without cooperative cancellation, would run to
    // the instruction budget. With the cancel flag set, the very first
    // instruction-hook firing aborts it and maps to `Error::Interrupted`.
    let program = LuaProgram::compile(
        "local n = 0\nwhile true do n = n + 1 end",
        "cancel loop",
        NonZeroU32::MIN,
        EXECUTION,
        &NullObserver::default(),
        "Loop",
    )
    .expect("an infinite loop still compiles");

    let handle = CancelHandle::new();
    handle.cancel();

    let start = Instant::now();
    let outcome = cancel::scope(handle, async {
        tokio::task::block_in_place(|| {
            let lua = Lua::new();
            install_instruction_budget(&lua).expect("hook installs on a fresh VM");
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
        &NullObserver::default(),
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
        &NullObserver::default(),
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
fn var_guard_allows_json_data_and_reads_back() {
    let out = run(
        "var.n = 1\nvar.s = 'x'\nvar.t = { a = {1, 2} }\nvar.b = true",
        "",
    )
    .expect("JSON data writes must pass the guard");
    assert_eq!(
        out.var,
        json!({ "n": 1, "s": "x", "t": { "a": [1, 2] }, "b": true })
    );
}

#[test]
fn var_rejects_a_function_at_the_assigning_line() {
    let error = run("var.f = function() end", "")
        .expect_err("a function assigned into var must fail at the assigning line");
    assert!(
        error
            .to_string()
            .contains("var.f must be JSON data, got function"),
        "error was {error}"
    );
}

#[test]
fn var_rejects_a_nested_function_at_the_assigning_line() {
    let error = run("var.t = { f = function() end }", "")
        .expect_err("a nested function must fail the deep check at the assigning line");
    assert!(
        error.to_string().contains("function"),
        "the bridge error must name the offending type: {error}"
    );
}

#[test]
fn var_guard_error_is_catchable_at_the_assigning_line() {
    // A pcall around the write catches the guard's error, proving the failure
    // is raised by that statement rather than later at serialization. The
    // caught value is mlua's error userdata, so stringify before matching.
    let out = run(
        "local ok, err = pcall(function() var.f = function() end end)\n\
         assert(not ok, 'the write must fail')\n\
         assert(tostring(err):match('must be JSON data'), tostring(err))\n\
         var.kept = 'yes'\n\
         return var.kept",
        "",
    )
    .expect("the caught guard error must not fail the chunk");
    assert_eq!(out.returned.as_deref(), Some("yes"));
    assert_eq!(out.var.get("kept").and_then(|v| v.as_str()), Some("yes"));
}

#[test]
fn var_guard_rejects_incremental_nested_function_writes_at_the_assigning_line() {
    let out = run(
        "var.t = {}\n\
         local ok, err = pcall(function() var.t.f = function() end end)\n\
         assert(not ok, 'the nested write must fail')\n\
         assert(tostring(err):match('var.t.f must be JSON data'), tostring(err))\n\
         var.t.kept = 'yes'\n\
         return var.t.kept",
        "",
    )
    .expect("the nested guard error must remain catchable");
    assert_eq!(out.returned.as_deref(), Some("yes"));
    assert_eq!(out.var, json!({ "t": { "kept": "yes" } }));
}

#[test]
fn reassigning_the_var_global_fails_read_back() {
    // `var = 5` drops the guarded proxy from reach; read-back must reject it
    // rather than silently roll the pre-reassignment data forward.
    let error = run("var = 5", "").expect_err("reassigning the var global must fail at read-back");
    assert!(
        error.to_string().contains("`var` global was reassigned"),
        "the error must name the reassignment: {error}"
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
            .contains("tools.add alias \"web_search\" was not declared by tools.bind"),
        "the error must name the undeclared alias: {error}"
    );
}

#[test]
fn add_without_declarations_fails_in_a_prologue_without_a_shared_library() {
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('web_search')"),
        &NullObserver::default(),
        "Test",
    )
    .expect_err("an undeclared alias must fail loudly");
    assert!(
        error.to_string().contains("not declared by tools.bind"),
        "the error must report the missing declaration: {error}"
    );
    vm.teardown(&NullObserver::default(), "Test");
}

#[test]
fn add_with_empty_frozen_bindings_fails_as_undeclared() {
    let shared = program("function helper() return 'no declarations' end");
    let resolver = |description: &str| -> Result<ToolId> {
        panic!("a declaration-free program must not resolve {description:?}")
    };
    let bindings = execute_live_tool_binds(
        &shared,
        &resolver,
        EXECUTION,
        &NullObserver::default(),
        "Prompt",
    )
    .expect("a bind-free H1 program must execute");
    assert!(bindings.bindings().is_empty());
    let mut vm = section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Test")
        .expect("empty captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let error = run_scalar(
        &vm,
        &program("tools.add('web_search')"),
        &NullObserver::default(),
        "Test",
    )
    .expect_err("an undeclared alias must fail loudly");
    assert!(
        error.to_string().contains("not declared by tools.bind"),
        "the error must report the missing declaration: {error}"
    );
    vm.teardown(&NullObserver::default(), "Test");
}

#[test]
fn add_with_an_override_argument_records_the_model_description() {
    let bindings = fixture_bindings("tools.bind('search', 'search the web')");
    let mut vm = section_vm_with_bindings(&bindings, EXECUTION, &NullObserver::default(), "Test")
        .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    run_scalar(
        &vm,
        &program("tools.add('search', 'Search the web for pages matching a query.')"),
        &NullObserver::default(),
        "Test",
    )
    .expect("a description passed to tools.add is the model-facing override");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("tool scope must snapshot");
    assert_eq!(
        scope[0].model_description(),
        Some("Search the web for pages matching a query."),
        "the add override must reach the scoped binding"
    );
    vm.teardown(&NullObserver::default(), "Test");
}

#[test]
fn a_section_vm_without_declarations_snapshots_to_an_empty_scope() {
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host values must inject");
    let (bindings, runtime) = vm.tool_bag_handles();
    let scope = current_tool_bindings(&bindings, &runtime).expect("an empty scope must snapshot");
    assert!(scope.is_empty());
    vm.teardown(&NullObserver::default(), "Test");
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
fn store_write_then_read_numbered_returns_numbered_content() {
    let out = run(
        "store.write('a.txt', 'first\\nsecond')\nreturn store.read_numbered('a.txt')",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| first\n2| second"));
}

#[test]
fn store_append_extends_the_file() {
    let out = run(
            "store.append('log.txt', 'one\\n')\nstore.append('log.txt', 'two')\nreturn store.read_numbered('log.txt')",
            "",
        )
        .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| one\n2| two"));
}

#[test]
fn store_str_replace_edits_in_place() {
    let out = run(
            "store.write('a.txt', 'the quick brown fox')\nstore.str_replace('a.txt', 'quick', 'slow')\nreturn store.read_numbered('a.txt')",
            "",
        )
        .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| the slow brown fox"));
}

#[test]
fn store_delete_then_read_raises() {
    let err = run(
        "store.write('a.txt', 'gone soon')\nstore.delete('a.txt')\nreturn store.read('a.txt')",
        "",
    )
    .expect_err("reading a deleted file must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("file not found"),
        "the Lua error must carry the store message, got: {msg}"
    );
}

#[test]
fn store_inject_is_absent() {
    let out = run("return tostring(store.inject)", "").unwrap();
    assert_eq!(
        out.returned.as_deref(),
        Some("nil"),
        "store.inject was removed; indexing it must yield nil"
    );
    assert!(
        run("store.inject('a.txt')", "").is_err(),
        "calling the removed store.inject must raise"
    );
}

#[test]
fn store_read_lines_is_absent() {
    let out = run("return tostring(store.read_lines)", "").unwrap();
    assert_eq!(
        out.returned.as_deref(),
        Some("nil"),
        "store.read_lines was removed; indexing it must yield nil"
    );
    assert!(
        run("store.read_lines('a.txt')", "").is_err(),
        "calling the removed store.read_lines must raise"
    );
}

#[test]
fn store_read_with_start_only_reads_to_eof() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 2)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("two\nthree"));
}

#[test]
fn store_read_with_start_and_end_slices_inclusively() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 2, 2)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("two"));
}

#[test]
fn store_read_clamps_end_to_the_last_line() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 2, 99)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("two\nthree"));
}

#[test]
fn store_read_beyond_eof_returns_empty() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 99)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some(""));
}

#[test]
fn store_read_start_below_one_raises() {
    for source in [
        "store.write('a.txt', 'one')\nreturn store.read('a.txt', 0)",
        "store.write('a.txt', 'one')\nreturn store.read('a.txt', -1)",
    ] {
        let err = run(source, "").expect_err("a start below 1 must raise");
        let msg = lua_error_message(&err);
        assert!(
            msg.contains("invalid line range"),
            "the Lua error must carry the range message, got: {msg}"
        );
    }
}

#[test]
fn store_read_end_before_start_raises() {
    let err = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read('a.txt', 3, 2)",
        "",
    )
    .expect_err("an end before start must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("invalid line range"),
        "the Lua error must carry the range message, got: {msg}"
    );
}

#[test]
fn store_read_end_without_start_raises() {
    let err = run(
        "store.write('a.txt', 'one')\nreturn store.read('a.txt', nil, 1)",
        "",
    )
    .expect_err("an end without a start must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("invalid line range"),
        "the Lua error must carry the range message, got: {msg}"
    );
}

#[test]
fn store_read_numbered_without_bounds_numbers_from_one() {
    let store = StoreRef::memory();
    let out = run_with(
        "store.write('a.txt', 'first\\nsecond')\nreturn store.read_numbered('a.txt')",
        &store,
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("1| first\n2| second"));
}

#[test]
fn store_read_numbered_numbers_a_slice_absolutely() {
    let store = StoreRef::memory();
    let mut body = String::new();
    for n in 1..=85 {
        use std::fmt::Write as _;
        let _ = writeln!(body, "line{n}");
    }
    store.write("a.txt", &body).expect("write");
    let out = run_with("return store.read_numbered('a.txt', 84, 85)", &store).unwrap();
    assert_eq!(out.returned.as_deref(), Some("84| line84\n85| line85"));
}

#[test]
fn store_read_numbered_pads_across_the_hundred_boundary() {
    let store = StoreRef::memory();
    let mut body = String::new();
    for n in 1..=100 {
        use std::fmt::Write as _;
        let _ = writeln!(body, "line{n}");
    }
    store.write("a.txt", &body).expect("write");
    let out = run_with("return store.read_numbered('a.txt', 99, 100)", &store).unwrap();
    assert_eq!(out.returned.as_deref(), Some(" 99| line99\n100| line100"));
}

#[test]
fn store_read_numbered_clamps_end_to_the_last_line() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read_numbered('a.txt', 2, 99)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some("2| two\n3| three"));
}

#[test]
fn store_read_numbered_beyond_eof_returns_empty() {
    let out = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read_numbered('a.txt', 99)",
        "",
    )
    .unwrap();
    assert_eq!(out.returned.as_deref(), Some(""));
}

#[test]
fn store_read_numbered_start_below_one_raises() {
    for source in [
        "store.write('a.txt', 'one')\nreturn store.read_numbered('a.txt', 0)",
        "store.write('a.txt', 'one')\nreturn store.read_numbered('a.txt', -1)",
    ] {
        let err = run(source, "").expect_err("a start below 1 must raise");
        let msg = lua_error_message(&err);
        assert!(
            msg.contains("invalid line range"),
            "the Lua error must carry the range message, got: {msg}"
        );
    }
}

#[test]
fn store_read_numbered_end_before_start_raises() {
    let err = run(
        "store.write('a.txt', 'one\\ntwo\\nthree')\nreturn store.read_numbered('a.txt', 3, 2)",
        "",
    )
    .expect_err("an end before start must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("invalid line range"),
        "the Lua error must carry the range message, got: {msg}"
    );
}

#[test]
fn store_read_numbered_end_without_start_raises() {
    let err = run(
        "store.write('a.txt', 'one')\nreturn store.read_numbered('a.txt', nil, 1)",
        "",
    )
    .expect_err("an end without a start must raise");
    let msg = lua_error_message(&err);
    assert!(
        msg.contains("invalid line range"),
        "the Lua error must carry the range message, got: {msg}"
    );
}

#[test]
fn installed_store_read_honors_line_bounds() {
    let store = StoreRef::memory();
    store
        .write("a.txt", "one\ntwo\nthree\n")
        .expect("the memory store can prepare a file");
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");
    vm.inject_host("", &json!({}), &store, None)
        .expect("host values must inject");
    let observer: Arc<dyn Observer> = Arc::new(NullObserver::default());
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");

    let sliced = run_scalar(
        &vm,
        &program("return store.read('a.txt', 2, 2)"),
        &NullObserver::default(),
        "Test",
    )
    .expect("a bounded read must run");
    assert_eq!(sliced.as_deref(), Some("two"));

    let err = run_scalar(
        &vm,
        &program("return store.read('a.txt', 0)"),
        &NullObserver::default(),
        "Test",
    )
    .expect_err("a start below 1 must raise");
    assert!(
        err.to_string().contains("invalid line range"),
        "the error must carry the range message, got: {err}"
    );
}

#[test]
fn installed_store_read_numbered_honors_line_bounds() {
    let store = StoreRef::memory();
    store
        .write("a.txt", "one\ntwo\nthree\n")
        .expect("the memory store can prepare a file");
    let mut vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");
    vm.inject_host("", &json!({}), &store, None)
        .expect("host values must inject");
    let observer: Arc<dyn Observer> = Arc::new(NullObserver::default());
    vm.install_host_apis(&observer, "Test")
        .expect("host APIs must install");

    let numbered = run_scalar(
        &vm,
        &program("return store.read_numbered('a.txt', 2, 3)"),
        &NullObserver::default(),
        "Test",
    )
    .expect("a bounded numbered read must run");
    assert_eq!(numbered.as_deref(), Some("2| two\n3| three"));

    let whole = run_scalar(
        &vm,
        &program("return store.read_numbered('a.txt')"),
        &NullObserver::default(),
        "Test",
    )
    .expect("an unbounded numbered read must run");
    assert_eq!(whole.as_deref(), Some("1| one\n2| two\n3| three"));

    let err = run_scalar(
        &vm,
        &program("return store.read_numbered('a.txt', 0)"),
        &NullObserver::default(),
        "Test",
    )
    .expect_err("a start below 1 must raise");
    assert!(
        err.to_string().contains("invalid line range"),
        "the error must carry the range message, got: {err}"
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
    let msg = lua_error_message(&err);
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
        store.read("shared.txt").expect("read"),
        "from lua",
        "a Lua write must land in the shared store"
    );
}

#[test]
fn store_reports_are_ordered_exact_and_payload_free_on_failure() {
    let recorder = Arc::new(Recorder::default());
    let observer: Arc<dyn Observer> = recorder.clone();
    let store = StoreRef::memory();
    let source = "store.write('secret/path.txt', 'private contents')\n\
                      store.read('secret/path.txt')\n\
                      store.str_replace('secret/path.txt', 'missing secret', 'replacement')";
    let error = run_chunk(
        source,
        "private input",
        &json!({ "id": 1, "when": "t" }),
        &store,
        EXECUTION,
        &observer,
        "Gather",
    )
    .expect_err("the missing anchor must fail");
    assert!(matches!(error, Error::Lua(_) | Error::LuaRuntime { .. }));

    let observations = recorder.observations();
    assert_eq!(
        observations,
        vec![
            ("Gather".to_string(), detail::STORE_WRITE_SUCCEEDED.clone()),
            ("Gather".to_string(), detail::STORE_READ_SUCCEEDED.clone()),
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
            source: "store.read('a.txt')",
            success: detail::STORE_READ_SUCCEEDED,
            failure: detail::STORE_READ_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.read('a.txt', 1, 1)",
            success: detail::STORE_READ_SUCCEEDED,
            failure: detail::STORE_READ_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.read_numbered('a.txt')",
            success: detail::STORE_READ_NUMBERED_SUCCEEDED,
            failure: detail::STORE_READ_NUMBERED_FAILED,
            prepare: existing,
        },
        Case {
            source: "store.read_numbered('a.txt', 1, 1)",
            success: detail::STORE_READ_NUMBERED_SUCCEEDED,
            failure: detail::STORE_READ_NUMBERED_FAILED,
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
        let recorder = Arc::new(Recorder::default());
        let observer: Arc<dyn Observer> = recorder.clone();
        run_chunk(
            case.source,
            "",
            &json!({}),
            &store,
            EXECUTION,
            &observer,
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
        let recorder = Arc::new(Recorder::default());
        let observer: Arc<dyn Observer> = recorder.clone();
        let error = run_chunk(
            case.source,
            "",
            &json!({}),
            &store,
            EXECUTION,
            &observer,
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
    let recorder = Arc::new(BoundaryRecorder {
        store: store.clone(),
        snapshots: Mutex::new(Vec::new()),
    });
    let observer: Arc<dyn Observer> = recorder.clone();

    run_chunk(
        "store.write('first.txt', '')\nstore.write('second.txt', '')",
        "",
        &json!({}),
        &store,
        EXECUTION,
        &observer,
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
    let vm = SectionVm::new(&test_nonce(), EXECUTION, &NullObserver::default(), "Test")
        .expect("VM must build");
    vm.replay_shared(&shared, &NullObserver::default(), "Test")
        .expect("the shared library must call untrusted during load");
    vm.teardown(&NullObserver::default(), "Test");
}

#[test]
fn untrusted_global_rejects_a_non_string_argument() {
    let error = run("return untrusted({})", "").expect_err("a table is not a string");
    assert!(
        matches!(error, Error::Lua(_) | Error::LuaRuntime { .. }),
        "a non-string argument must surface as a Lua error, got {error:?}"
    );
}
