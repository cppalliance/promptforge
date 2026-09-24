//! The task-operation request parsers: `spawn`'s target, seeds, var
//! snapshot, origin, and fanout mark; the `timer` leaf request's
//! author-supplied `seconds` and its domain checks; and the loop shim's
//! `drain_task_notices` unit request.

use super::*;

#[test]
fn spawn_parses_target_seeds_var_and_origin() {
    let lua = Lua::new();
    let table = request_table(&lua, "spawn");
    table.raw_set("target", "## Child").expect("raw_set");
    table.raw_set("input", "override").expect("raw_set");
    let item = lua.create_table().expect("table creation cannot fail");
    item.raw_set("name", "alpha").expect("raw_set");
    table.raw_set("item", item).expect("raw_set");
    table.raw_set("index", 3).expect("raw_set");
    table.raw_set("origin", "author").expect("raw_set");
    set_var_snapshot(&lua, &table);
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Spawn {
            target,
            input,
            item,
            index,
            var,
            origin,
            fanout,
        } => {
            assert_eq!(target, "## Child");
            assert_eq!(input.as_deref(), Some("override"));
            assert_eq!(item, Some(json!({ "name": "alpha" })));
            assert_eq!(index, Some(3));
            assert_eq!(var, json!({ "k": 1 }));
            assert_eq!(origin, TaskOrigin::Author);
            assert!(!fanout, "an absent mark is a plain `tasks.spawn`");
        }
        other => panic!("expected a spawn request, got {other:?}"),
    }
}

#[test]
fn spawn_reads_the_fanout_mark_and_rejects_a_non_boolean_one() {
    // The mark is shim-produced: `true` from the fanout shim, absent from
    // `tasks.spawn`; any other shape is a hand-built yield.
    let lua = Lua::new();
    let table = request_table(&lua, "spawn");
    table.raw_set("target", "### Worker").expect("raw_set");
    table.raw_set("origin", "author").expect("raw_set");
    table.raw_set("fanout", true).expect("raw_set");
    set_var_snapshot(&lua, &table);
    match expect_request(Request::from_yield(&lua, &Value::Table(table))) {
        Request::Spawn { fanout, .. } => assert!(fanout, "the fanout shim's mark is read"),
        other => panic!("expected a spawn request, got {other:?}"),
    }

    let table = request_table(&lua, "spawn");
    table.raw_set("target", "### Worker").expect("raw_set");
    table.raw_set("origin", "author").expect("raw_set");
    table.raw_set("fanout", "yes").expect("raw_set");
    set_var_snapshot(&lua, &table);
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn spawn_without_options_parses_every_seed_as_absent() {
    let lua = Lua::new();
    let table = request_table(&lua, "spawn");
    table.raw_set("target", "## Child").expect("raw_set");
    table.raw_set("origin", "author").expect("raw_set");
    set_var_snapshot(&lua, &table);
    match expect_request(Request::from_yield(&lua, &Value::Table(table))) {
        Request::Spawn {
            input, item, index, ..
        } => {
            assert_eq!(input, None);
            assert_eq!(item, None);
            assert_eq!(index, None);
        }
        other => panic!("expected a spawn request, got {other:?}"),
    }
}

#[test]
fn spawn_seed_shape_errors_are_the_calls_error() {
    // `item` and `index` are author options: a wrong shape returns as
    // the call's answer so `tasks.spawn` raises it at the call site.
    let lua = Lua::new();
    let table = request_table(&lua, "spawn");
    table.raw_set("target", "## Child").expect("raw_set");
    table.raw_set("origin", "author").expect("raw_set");
    let function = lua
        .create_function(|_, ()| Ok(()))
        .expect("function creation cannot fail");
    table.raw_set("item", function).expect("raw_set");
    set_var_snapshot(&lua, &table);
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::Spawn(Err(Error::Lua(message)))) => {
            assert_eq!(message, "item must be JSON data, got function");
        }
        other => panic!("expected the item call error, got {other:?}"),
    }

    let table = request_table(&lua, "spawn");
    table.raw_set("target", "## Child").expect("raw_set");
    table.raw_set("origin", "author").expect("raw_set");
    table.raw_set("index", -1).expect("raw_set");
    set_var_snapshot(&lua, &table);
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::Spawn(Err(Error::Lua(message)))) => {
            assert_eq!(message, "index must be a non-negative integer, got -1");
        }
        other => panic!("expected the index call error, got {other:?}"),
    }
}

#[test]
fn spawn_with_an_unknown_or_missing_origin_is_a_malformed_yield() {
    // The origin is shim-produced, never an author argument: a wrong value
    // is a hand-built yield, not a call error.
    let lua = Lua::new();
    for origin in [Value::Nil, Value::Integer(1)] {
        let table = request_table(&lua, "spawn");
        table.raw_set("target", "## Child").expect("raw_set");
        table.raw_set("origin", origin).expect("raw_set");
        set_var_snapshot(&lua, &table);
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }
    let table = request_table(&lua, "spawn");
    table.raw_set("target", "## Child").expect("raw_set");
    table.raw_set("origin", "operator").expect("raw_set");
    set_var_snapshot(&lua, &table);
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn a_spawn_with_a_non_string_target_keeps_the_resolve_error() {
    let lua = Lua::new();
    let table = request_table(&lua, "spawn");
    table.raw_set("target", 42).expect("raw_set");
    table.raw_set("origin", "author").expect("raw_set");
    set_var_snapshot(&lua, &table);
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::Spawn(Err(Error::LuaRuntime { message, .. }))) => {
            assert!(
                message.contains("section target must be a string, got integer"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected the resolve_section_target call error, got {other:?}"),
    }
}

#[test]
fn a_drain_task_notices_yield_parses_to_the_request() {
    let lua = Lua::new();
    let table = request_table(&lua, "drain_task_notices");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    assert!(
        matches!(request, Request::DrainTaskNotices),
        "a drain_task_notices yield is the unit request, got {request:?}"
    );
}

#[test]
fn timer_parses_a_non_negative_finite_seconds_value() {
    let lua = Lua::new();
    for (value, expected) in [
        (Value::Number(1.5), 1.5),
        (Value::Integer(30), 30.0),
        (Value::Number(0.0), 0.0),
    ] {
        let table = request_table(&lua, "timer");
        table.raw_set("seconds", value).expect("raw_set");
        match expect_request(Request::from_yield(&lua, &Value::Table(table))) {
            Request::Timer { seconds } => assert!(
                (seconds - expected).abs() < f64::EPSILON,
                "expected {expected}, got {seconds}"
            ),
            other => panic!("expected a timer request, got {other:?}"),
        }
    }
}

#[test]
fn timer_seconds_out_of_domain_are_the_calls_error() {
    // `seconds` is the author's `opts.timeout`: a negative, non-finite,
    // out-of-range, or non-numeric value returns as the call's answer
    // so the wait shim raises it at the call site and starts no timer.
    let lua = Lua::new();
    let cases: [(Value, &str); 5] = [
        (Value::Number(-1.0), "-1"),
        (Value::Number(f64::NAN), "NaN"),
        (Value::Number(f64::INFINITY), "inf"),
        // Past `Duration`'s u64 seconds; Display renders the plain digits.
        (Value::Number(1e20), "100000000000000000000"),
        (Value::Nil, "nil"),
    ];
    for (value, needle) in cases {
        let table = request_table(&lua, "timer");
        table.raw_set("seconds", value).expect("raw_set");
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::Timer(Err(Error::Lua(message)))) => {
                assert!(
                    message.contains("timeout") && message.contains(needle),
                    "the message names the option and the value: {message}"
                );
            }
            other => panic!("expected the timeout call error for {needle}, got {other:?}"),
        }
    }
    let table = request_table(&lua, "timer");
    table.raw_set("seconds", "soon").expect("raw_set");
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::Timer(Err(Error::Lua(message)))) => {
            assert_eq!(message, "timeout must be a number, got string");
        }
        other => panic!("expected the timeout type error, got {other:?}"),
    }
}

#[test]
fn task_events_parses_the_task_and_the_optional_last_bound() {
    let lua = Lua::new();
    let table = request_table(&lua, "task_events");
    table.raw_set("task", "0.2").expect("raw_set");
    match expect_request(Request::from_yield(&lua, &Value::Table(table))) {
        Request::TaskEvents { task, last } => {
            assert_eq!(task, "0.2".parse::<TaskId>().expect("a task id parses"));
            assert_eq!(last, None, "an absent `last` reads from the start");
        }
        other => panic!("expected a task_events request, got {other:?}"),
    }
    for (value, expected) in [(Value::Integer(7), 7), (Value::Number(3.0), 3)] {
        let table = request_table(&lua, "task_events");
        table.raw_set("task", "0.2").expect("raw_set");
        table.raw_set("last", value).expect("raw_set");
        match expect_request(Request::from_yield(&lua, &Value::Table(table))) {
            Request::TaskEvents { last, .. } => assert_eq!(last, Some(expected)),
            other => panic!("expected a task_events request, got {other:?}"),
        }
    }
}

#[test]
fn task_events_last_out_of_domain_is_the_calls_error() {
    // `last` is the author's `opts.last`: a negative, fractional, or
    // non-numeric value returns as the call's answer so the shim raises
    // it at the call site; a malformed id is the id's own error.
    let lua = Lua::new();
    for (value, needle) in [
        (Value::Integer(-1), "-1"),
        (Value::Number(1.5), "1.5"),
        (Value::Number(f64::from(u32::MAX) + 1.0), "4294967296"),
        (Value::Boolean(true), "boolean"),
    ] {
        let table = request_table(&lua, "task_events");
        table.raw_set("task", "0.2").expect("raw_set");
        table.raw_set("last", value).expect("raw_set");
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::TaskEvents(Err(Error::Lua(message)))) => assert!(
                message.starts_with("last must be") && message.contains(needle),
                "the message names the option and the value: {message}"
            ),
            other => panic!("expected the `last` call error for {needle}, got {other:?}"),
        }
    }
    let table = request_table(&lua, "task_events");
    table.raw_set("task", "nope").expect("raw_set");
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::TaskEvents(Err(Error::Lua(message)))) => {
            assert!(message.contains("is not a task id"), "got {message}");
        }
        other => panic!("expected the task id call error, got {other:?}"),
    }
}

#[test]
fn a_timer_answer_resumes_the_task_id_as_its_path_text() {
    let lua = Lua::new();
    let task: TaskId = "0.3".parse().expect("a task id parses");
    let answer: Answer<Error> = Answer::Timer(Ok(task));
    let (envelope, retained) = answer.into_envelope(&lua).expect("envelope renders");
    assert!(retained.is_none());
    let (ok, value) = echo_through_lua(&lua, envelope);
    assert!(ok);
    assert_eq!(
        value,
        Value::String(lua.create_string("0.3").expect("string"))
    );
}
