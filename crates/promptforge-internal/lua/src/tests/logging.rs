//! The `log` global: event correlation and order across chunks and Engine
//! operations, its argument and budget checks, and its effect-free null
//! observer.

use super::*;

const EXECUTION: &str = "lua-test";

#[test]
fn logs_are_correlated_and_ordered_across_chunks() {
    let recorder = Arc::new(Recorder::default());
    let bindings = fixture_set(&[("search", "search the web")], &[]);
    let mut vm = section_vm_with_bindings(&bindings, recorder.emitter(), "Gather")
        .expect("the section VM builds over the offering");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    let observer = recorder.emitter().clone();
    vm.install_engine_globals(&observer, "Gather")
        .expect("Engine globals must install");
    run_scalar(
        &vm,
        &program("log('prologue checkpoint')"),
        recorder.emitter(),
        "Gather",
    )
    .expect("first chunk log must succeed");
    run_scalar(
        &vm,
        &program("log('epilog checkpoint')"),
        recorder.emitter(),
        "Gather",
    )
    .expect("second chunk log must succeed");
    vm.teardown(recorder.emitter(), "Gather");

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
fn compatibility_chunk_logs_interleave_with_engine_operations() {
    let recorder = Arc::new(Recorder::for_execution("compatibility-run"));
    let observer = recorder.emitter().clone();
    run_chunk(
        "log('before write')\n\
             store.write('state.txt', 'value')\n\
             log('after write')",
        "",
        &json!({}),
        &fresh_access(),
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
                detail::VFS_WRITE_SUCCEEDED.clone(),
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
        let observer = recorder.emitter().clone();
        let error = run_chunk(
            source,
            "",
            &json!({}),
            &fresh_access(),
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
    let observer = recorder.emitter().clone();
    run_chunk(
        &source,
        "",
        &json!({}),
        &fresh_access(),
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
    // Many small events must not emit unbounded total log bytes.
    // With a 4-event budget the byte budget is 4 * 256 = 1024 bytes; three
    // 400-byte messages (200 two-byte chars each) exceed it on the third
    // call, while only three of the four events have been spent - so the
    // BYTE ceiling, not the event ceiling, is what refuses the call.
    let mut vm = SectionVm::new(&test_nonce(), &null_emitter(), "Budget").expect("VM builds");
    vm.apply_lua_limits(DEFAULT_LUA_MEMORY_BYTES, 4)
        .expect("limits apply");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values inject");
    let recorder = Arc::new(Recorder::default());
    let observer = recorder.emitter().clone();
    vm.install_engine_globals(&observer, "Budget")
        .expect("Engine globals must install");
    let program = program(
        "log(string.rep('é', 200))\n\
             log(string.rep('é', 200))\n\
             log(string.rep('é', 200))\n\
             return 'unreached'",
    );
    let error = run_scalar(&vm, &program, recorder.emitter(), "Budget")
        .expect_err("the cumulative byte budget must refuse the third message");
    // The refusal is the stable typed quota error, not an opaque
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
    vm.teardown(&null_emitter(), "Budget");
}

#[test]
fn logging_does_not_change_results_or_store_effects_with_null_observer() {
    let source = "log('checkpoint')\n\
                      var.answer = args\n\
                      store.write('answer.txt', args)\n\
                      return var.answer";
    let recorded_access = fresh_access();
    let recorded_store = store_view(&recorded_access);
    let recorder = Arc::new(Recorder::default());
    let observer = recorder.emitter().clone();
    let observed_outcome = run_chunk(
        source,
        "same",
        &json!({}),
        &recorded_access,
        &observer,
        "Equivalence",
    )
    .expect("recorded execution must succeed");
    let null_access = fresh_access();
    let null_store = store_view(&null_access);
    let silent = run_chunk(
        source,
        "same",
        &json!({}),
        &null_access,
        &null_emitter(),
        "Equivalence",
    )
    .expect("silent execution must succeed");

    assert_eq!(observed_outcome.returned, silent.returned);
    assert_eq!(observed_outcome.var, silent.var);
    assert_eq!(
        recorded_store
            .read_string("answer.txt")
            .expect("recorded write must persist"),
        null_store
            .read_string("answer.txt")
            .expect("silent write must persist")
    );
}

#[test]
fn installed_log_persists_across_chunks() {
    // `log` is installed once per section by `install_engine_globals`, so a saved
    // reference stays live for every later chunk in the same VM.
    let recorder = Arc::new(Recorder::default());
    let observer = recorder.emitter().clone();
    let mut vm =
        SectionVm::new(&test_nonce(), &null_emitter(), "Section").expect("VM must construct");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("values must inject");
    vm.install_engine_globals(&observer, "Section")
        .expect("Engine globals must install");
    run_scalar(
        &vm,
        &program("saved_log = log; log('first chunk')"),
        recorder.emitter(),
        "Section",
    )
    .expect("first chunk log must succeed");
    run_scalar(
        &vm,
        &program("saved_log('retained call')"),
        recorder.emitter(),
        "Section",
    )
    .expect("a retained log reference stays live for the section's lifecycle");
    vm.teardown(recorder.emitter(), "Section");

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
            let observer = recorder.emitter_for(execution);
            run_chunk(
                "log('first'); log('second')",
                "",
                &json!({}),
                &fresh_access(),
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
