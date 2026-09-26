//! Answer-to-envelope rendering for the `chat` answer: a reply round, a
//! tool-calls round, an overflow (with its compactor tag), an empty round
//! (with its detail and finish reason), and the typed error, each read
//! back through Lua with absent fields as true nil.

use super::*;

#[test]
fn an_ok_chat_reply_answer_resumes_as_a_table_with_nil_tool_calls() {
    use promptforge_types::metrics::{ClientTiming, Usage};

    let lua = Lua::new();
    let result = ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: Some("hello there".to_owned()),
        empty_detail: None,
        tool_calls: None,
        finish_reason: Some("stop".to_owned()),
        model: "fixture-model".to_owned(),
        metrics: Some(CallMetrics {
            usage: Some(Usage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 10,
                cached_tokens: None,
                reasoning_tokens: None,
            }),
            llama: None,
            vllm: None,
            client: Some(ClientTiming {
                ttft_ms: Some(9.5),
                mean_itl_ms: None,
                e2e_ms: 41.5,
            }),
        }),
        turn: 1,
    };
    let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    // The loop shim branches on presence: absent fields must read back as
    // true Lua nil, never a serde null sentinel.
    let (ok, reply, tools_nil, finish, model, total, llama_nil, e2e, overflow): (
        bool,
        String,
        bool,
        String,
        String,
        i64,
        bool,
        f64,
        bool,
    ) = lua
        .load(
            "local ok, r = ...; \
             return ok, r.reply, r.tool_calls == nil, r.finish_reason, r.model, \
             r.metrics.usage.total_tokens, r.metrics.llama == nil, r.metrics.client.e2e_ms, \
             r.overflow",
        )
        .call(envelope)
        .expect("the result table reads back through Lua");
    assert!(ok);
    assert!(!overflow, "a completed round renders overflow as false");
    assert_eq!(reply, "hello there");
    assert!(
        tools_nil,
        "an absent tool_calls must be nil, not a null sentinel"
    );
    assert_eq!(finish, "stop");
    assert_eq!(model, "fixture-model");
    assert_eq!(total, 10);
    assert!(llama_nil, "an absent metrics section must be nil");
    assert!((e2e - 41.5).abs() < f64::EPSILON);
}

#[test]
fn an_ok_chat_tool_calls_answer_resumes_with_presence_and_arguments() {
    let lua = Lua::new();
    let result = ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: None,
        empty_detail: None,
        tool_calls: Some(vec![
            ToolCallEvent {
                id: "call_1".to_owned(),
                name: "echo".to_owned(),
                arguments: json!({ "value": "hi" }),
            },
            ToolCallEvent {
                id: "call_2".to_owned(),
                name: "search".to_owned(),
                arguments: json!({ "query": "rust" }),
            },
        ]),
        finish_reason: Some("tool_calls".to_owned()),
        model: "fixture-model".to_owned(),
        metrics: None,
        turn: 1,
    };
    let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, reply_nil, len, id, name, value, second, metrics_nil): (
        bool,
        bool,
        i64,
        String,
        String,
        String,
        String,
        bool,
    ) = lua
        .load(
            "local ok, r = ...; \
             return ok, r.reply == nil, #r.tool_calls, r.tool_calls[1].id, \
             r.tool_calls[1].name, r.tool_calls[1].arguments.value, \
             r.tool_calls[2].arguments.query, r.metrics == nil",
        )
        .call(envelope)
        .expect("the result table reads back through Lua");
    assert!(ok);
    assert!(reply_nil, "a tool-calls round has no reply");
    assert_eq!(len, 2);
    assert_eq!(id, "call_1");
    assert_eq!(name, "echo");
    assert_eq!(value, "hi");
    assert_eq!(second, "rust");
    assert!(metrics_nil);
}

#[test]
fn a_chat_tool_calls_answer_resumes_the_turn_it_was_reported_under() {
    // The loop shim passes `turn` back with each call so the call reports
    // the round that requested it, not the counter when it runs.
    let lua = Lua::new();
    let result = ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: None,
        empty_detail: None,
        tool_calls: Some(vec![ToolCallEvent {
            id: "call_1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({}),
        }]),
        finish_reason: Some("tool_calls".to_owned()),
        model: "fixture-model".to_owned(),
        metrics: None,
        turn: 4,
    };
    let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, turn, is_integer): (bool, i64, bool) = lua
        .load("local ok, r = ...; return ok, r.turn, math.type(r.turn) == 'integer'")
        .call(envelope)
        .expect("the result table reads back through Lua");
    assert!(ok);
    assert_eq!(turn, 4);
    assert!(is_integer, "the turn resumes as a Lua integer");
}

#[test]
fn an_overflow_chat_answer_resumes_with_overflow_true_and_nothing_else() {
    // The request was refused as too large before or by the provider: no
    // round ran, so the shim branches on `overflow` and calls the compactor
    // without ever reading a reply or tool calls.
    let lua = Lua::new();
    let result = ChatResult {
        overflow: true,
        overflow_reason: Some(crate::OverflowReason::Precheck),
        reply: None,
        empty_detail: None,
        tool_calls: None,
        finish_reason: None,
        model: String::new(),
        metrics: None,
        turn: 1,
    };
    let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, overflow, reply_nil, calls_nil, finish_nil, model): (
        bool,
        bool,
        bool,
        bool,
        bool,
        String,
    ) = lua
        .load(
            "local ok, r = ...; \
             return ok, r.overflow, r.reply == nil, r.tool_calls == nil, \
             r.finish_reason == nil, r.model",
        )
        .call(envelope)
        .expect("the result table reads back through Lua");
    assert!(
        ok,
        "an overflow is a successful answer, not a failure envelope"
    );
    assert!(overflow, "the overflow flag must read back as true");
    assert!(reply_nil, "an overflow leaves reply nil");
    assert!(calls_nil, "an overflow leaves tool_calls nil");
    assert!(finish_nil, "an overflow leaves finish_reason nil");
    assert_eq!(model, "");
}

#[test]
fn an_empty_reply_chat_answer_resumes_with_nil_reply_and_its_finish_reason() {
    // An empty reply is a completed round with `reply` absent: the shim
    // reads nil (never an empty string) and applies the exit rules against
    // `finish_reason`.
    let lua = Lua::new();
    let result = ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: None,
        empty_detail: Some("empty model reply".to_owned()),
        tool_calls: None,
        finish_reason: Some("stop".to_owned()),
        model: "fixture-model".to_owned(),
        metrics: None,
        turn: 1,
    };
    let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, overflow, reply_nil, calls_nil, finish, model): (
        bool,
        bool,
        bool,
        bool,
        String,
        String,
    ) = lua
        .load(
            "local ok, r = ...; \
             return ok, r.overflow, r.reply == nil, r.tool_calls == nil, \
             r.finish_reason, r.model",
        )
        .call(envelope)
        .expect("the result table reads back through Lua");
    assert!(ok);
    assert!(
        !overflow,
        "an empty reply is a completed round, not an overflow"
    );
    assert!(
        reply_nil,
        "the absent reply must be nil, not an empty string"
    );
    assert!(calls_nil);
    assert_eq!(finish, "stop");
    assert_eq!(model, "fixture-model");
}

#[test]
fn an_empty_reply_string_chat_answer_also_resumes_with_nil_reply() {
    // The render drops an empty `reply` string, so a producer that hands
    // over `Some("")` instead of the documented absent field still resumes
    // the shim with nil: presence-branching never sees an empty string.
    let lua = Lua::new();
    let result = ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: Some(String::new()),
        empty_detail: None,
        tool_calls: None,
        finish_reason: Some("stop".to_owned()),
        model: "fixture-model".to_owned(),
        metrics: None,
        turn: 1,
    };
    let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, reply_nil, finish): (bool, bool, String) = lua
        .load("local ok, r = ...; return ok, r.reply == nil, r.finish_reason")
        .call(envelope)
        .expect("the result table reads back through Lua");
    assert!(ok);
    assert!(
        reply_nil,
        "an empty reply string must resume as nil, not as an empty string"
    );
    assert_eq!(finish, "stop");
}

#[test]
fn an_err_chat_answer_round_trips_and_retains_the_typed_error() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::Chat(Err(Error::Interrupted))
        .into_envelope(&lua)
        .expect("the envelope renders");
    match retained {
        Some(Error::Interrupted) => {}
        other => panic!("expected the retained Interrupted error, got {other:?}"),
    }
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(!ok);
    let (kind, message) = failure_parts(&lua, result);
    assert_eq!(kind, "cancelled");
    assert_eq!(message, "interrupted by Ctrl-C");
}

#[test]
fn an_overflow_chat_answer_resumes_the_flag_and_the_compactor_tag() {
    // The loop shim hands `overflow_reason` to the compactor as its tag, so
    // it must resume as the reason's exact tag string beside the flag.
    let lua = Lua::new();
    for (reason, tag) in [
        (crate::OverflowReason::Precheck, "precheck"),
        (crate::OverflowReason::Provider, "provider"),
    ] {
        let result = ChatResult {
            overflow: true,
            overflow_reason: Some(reason),
            reply: None,
            empty_detail: None,
            tool_calls: None,
            finish_reason: None,
            model: String::new(),
            metrics: None,
            turn: 1,
        };
        let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, overflow, resumed_tag, reply_nil): (bool, bool, String, bool) = lua
            .load("local ok, r = ...; return ok, r.overflow, r.overflow_reason, r.reply == nil")
            .call(envelope)
            .expect("the result table reads back through Lua");
        assert!(ok);
        assert!(overflow, "the flag resumes set");
        assert_eq!(
            resumed_tag, tag,
            "the reason resumes as the compactor's tag"
        );
        assert!(reply_nil, "no round ran");
    }
}

#[test]
fn an_empty_round_chat_answer_resumes_its_detail_beside_the_absent_reply() {
    // The exit rules raise `empty_detail` as the empty_model_reply message,
    // so the client's phrase must resume verbatim while `reply` stays nil.
    let lua = Lua::new();
    let result = ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: None,
        empty_detail: Some(
            "empty model reply: reasoning content was present but ignored".to_owned(),
        ),
        tool_calls: None,
        finish_reason: Some("stop".to_owned()),
        model: String::new(),
        metrics: None,
        turn: 1,
    };
    let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, reply_nil, detail, overflow_reason_nil): (bool, bool, String, bool) = lua
        .load(
            "local ok, r = ...; return ok, r.reply == nil, r.empty_detail, r.overflow_reason == nil",
        )
        .call(envelope)
        .expect("the result table reads back through Lua");
    assert!(ok);
    assert!(reply_nil, "an empty round leaves reply nil");
    assert_eq!(
        detail,
        "empty model reply: reasoning content was present but ignored"
    );
    assert!(overflow_reason_nil, "a served round names no overflow gate");
}
