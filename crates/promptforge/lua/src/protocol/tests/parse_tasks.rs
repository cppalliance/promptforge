//! The task-operation request parsers: the `timer` leaf request's
//! author-supplied `seconds` and its domain checks.

use super::*;

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
    // out-of-range, or non-numeric value rides back as the call's answer
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
