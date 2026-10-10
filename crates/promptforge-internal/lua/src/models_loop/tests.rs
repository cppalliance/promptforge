//! Tests for the `models.loop` adapter over a fresh VM: Lua's type names,
//! both entries' argument checks, the envelope failure, the chat answer's
//! reader, and each step's action.

use mlua::{Function, Lua, Value};
use promptforge_model_client::model::{ModelBinding, ModelInvocation};
use promptforge_types::detail::model_id_from_validated;
use promptforge_types::metrics::ToolCallEvent;
use serde_json::json;

use super::machine::Then;
use super::{act, chat_result, envelope_failure, loop_begin, lua_type_name};
use crate::compactors::OverflowReason;
use crate::error::Error;
use crate::error_value::{ErrorField, ErrorKind, Raised, error_table, raised_from};
use crate::hardening::InstructionBudget;
use crate::messages::MessageList;
use crate::models::LuaModelHandle;
use crate::protocol::{Answer, ChatResult, LocalToolOutcome, Request, YieldParse};

/// A requested call named `echo`, its arguments naming its id.
fn call(id: &str) -> ToolCallEvent {
    ToolCallEvent {
        id: id.to_owned(),
        name: "echo".to_owned(),
        arguments: json!({ "id": id }),
        tool: None,
    }
}

/// A served round with no product, under turn 7.
fn round() -> ChatResult {
    ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: None,
        empty_detail: None,
        tool_calls: None,
        finish_reason: None,
        model: String::new(),
        metrics: None,
        turn: 7,
    }
}

/// A model handle's userdata over a stub binding.
fn handle(lua: &Lua) -> Value {
    let binding = ModelBinding::new(
        "fast",
        "a test model",
        model_id_from_validated("gateway", "m1"),
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking: None,
        },
        std::num::NonZeroU32::MIN,
    );
    Value::UserData(
        lua.create_userdata(LuaModelHandle::from_binding(&binding))
            .unwrap(),
    )
}

#[test]
fn lua_type_name_is_luas_type_with_an_integer_named_integer() {
    let lua = Lua::new();
    let lua_type: Function = lua.load("return type").eval().unwrap();
    let values = [
        Value::Nil,
        Value::Boolean(true),
        Value::Integer(1),
        Value::Number(1.5),
        Value::String(lua.create_string("s").unwrap()),
        Value::Table(lua.create_table().unwrap()),
        Value::Function(lua_type.clone()),
        Value::Thread(lua.create_thread(lua_type.clone()).unwrap()),
        handle(&lua),
        Value::NULL,
        Value::Error(Box::new(mlua::Error::runtime("boom"))),
    ];
    for value in values {
        let expected = match value {
            Value::Integer(_) => "integer".to_owned(),
            _ => lua_type.call::<String>(value.clone()).unwrap(),
        };
        assert_eq!(lua_type_name(&value), expected, "{value:?}");
    }
}

#[test]
fn both_entries_check_their_arguments_in_order_with_the_shims_texts() {
    let lua = Lua::new();
    let compactors = lua.create_table().unwrap();
    let begin = loop_begin(&lua, 2, compactors, &InstructionBudget::default()).unwrap();
    let list = lua.create_userdata(MessageList::default()).unwrap();
    let outcomes: Vec<String> = lua
        .load(
            "local begin, h, list, null = ...
             local function probe(...)
               local step, tag, value = begin(...)
               return type(step) .. '|' .. tag .. '|' .. tostring(value.op or value)
             end
             return {
               probe(false, h, list), probe(false, list, nil, nil), probe(false, list, 5, nil),
               probe(false, 1, 5), probe(false, 1, null), probe(false, 1), probe(false, list),
               probe(true, list), probe(true, h, list, nil, nil), probe(true, h, 1, 1.5),
               probe(true, h), probe(true, h, list, nil),
             }",
        )
        .call((begin, handle(&lua), list, Value::NULL))
        .unwrap();
    let not_a_list = "nil|raise|models.loop needs a messages.new() list; build one with \
                      messages.new() and :user, :append, or :replace";
    let started = "function|yield|chat";
    assert_eq!(
        outcomes,
        [
            "nil|raise|models.loop takes (messages, compactor?); call \
             handle:loop(messages, compactor?) to run on a model handle",
            "nil|raise|models.loop takes (messages, compactor?)",
            "nil|raise|models.loop takes (messages, compactor?)",
            "nil|raise|compactor must be a function, got integer",
            "nil|raise|compactor must be a function, got userdata",
            not_a_list,
            started,
            "nil|raise|call loop on a model handle with a colon: \
             handle:loop(messages, compactor?)",
            "nil|raise|handle:loop takes (messages, compactor?)",
            "nil|raise|compactor must be a function, got number",
            not_a_list,
            started,
        ]
    );
}

#[test]
fn envelope_failure_keeps_an_error_table_and_wraps_anything_else() {
    let lua = Lua::new();
    let table = error_table(&lua, &Error::Interrupted).unwrap();
    let kept = envelope_failure(&lua, Value::Table(table.clone())).unwrap();
    assert_eq!(kept.to_pointer(), table.to_pointer());
    let boom = Value::String(lua.create_string("boom").unwrap());
    let wrapped = envelope_failure(&lua, boom).unwrap();
    let raised = raised_from(&lua, &wrapped)
        .unwrap()
        .expect("an error table");
    assert_eq!(
        (raised.kind, raised.message.as_str()),
        (ErrorKind::Lua, "boom")
    );
}

#[test]
fn the_chat_reader_reads_back_every_field_the_loop_reads_from_the_rendered_table() {
    let lua = Lua::new();
    let cases = [
        ChatResult {
            overflow: true,
            overflow_reason: Some(OverflowReason::Provider),
            ..round()
        },
        ChatResult {
            reply: Some("done".to_owned()),
            finish_reason: Some("stop".to_owned()),
            ..round()
        },
        ChatResult {
            empty_detail: Some("the reply was empty".to_owned()),
            finish_reason: Some("length".to_owned()),
            ..round()
        },
        ChatResult {
            tool_calls: Some(vec![call("c1"), call("c2")]),
            finish_reason: Some("tool_calls".to_owned()),
            ..round()
        },
    ];
    for expected in cases {
        let served = ChatResult {
            model: "served-model".to_owned(),
            ..expected.clone()
        };
        let (envelope, _) = Answer::<Error>::Chat(Ok(Box::new(served)))
            .into_envelope(&lua)
            .unwrap();
        let table = envelope.into_iter().nth(1).expect("the result table");
        assert_eq!(chat_result(&lua, table).unwrap(), expected);
    }
}

/// `act`'s tag and values for `then`.
fn action(lua: &Lua, then: Then<Value>) -> (String, Vec<Value>) {
    let mut values = act(lua, then).unwrap().into_vec().into_iter();
    let tag: String = lua.unpack(values.next().expect("a tag")).unwrap();
    (tag, values.collect())
}

/// The request `act` yields for `then`, as the protocol parses it.
fn parsed(lua: &Lua, then: Then<Value>) -> Request {
    let (tag, values) = action(lua, then);
    assert_eq!((tag.as_str(), values.len()), ("yield", 1));
    match Request::from_yield(lua, &values[0]) {
        YieldParse::Request(request) => request,
        other => panic!("the yield parses, got {other:?}"),
    }
}

#[test]
fn act_yields_each_request_as_the_protocol_parses_it() {
    let lua = Lua::new();
    let tool_call = Then::ToolCall {
        call: call("c1"),
        turn: 7,
    };
    let Request::ToolCall {
        alias,
        args,
        call_id,
        turn,
    } = parsed(&lua, tool_call)
    else {
        panic!("a tool_call request");
    };
    assert_eq!(
        (alias.as_str(), call_id.as_deref(), turn),
        ("echo", Some("c1"), Some(7))
    );
    assert_eq!(args, json!({ "id": "c1" }));
    let returned = Value::String(lua.create_string("text").unwrap());
    let outcome = |then| match parsed(&lua, then) {
        Request::LocalToolDone { outcome } => outcome,
        other => panic!("a local_tool_done request, got {other:?}"),
    };
    let done = outcome(Then::Report(Some(returned)));
    assert!(matches!(done, LocalToolOutcome::Returned(text) if text == "text"));
    assert!(matches!(
        outcome(Then::Report(None)),
        LocalToolOutcome::Raised
    ));
    let list = Value::UserData(lua.create_userdata(MessageList::default()).unwrap());
    let receiver = handle(&lua);
    for (handle, expected) in [(Some(receiver.clone()), receiver), (None, Value::Nil)] {
        let messages = list.clone();
        let (tag, values) = action(&lua, Then::Chat { messages, handle });
        let Value::Table(chat) = &values[0] else {
            panic!("a request table");
        };
        assert_eq!(tag, "yield");
        assert_eq!(chat.raw_get::<String>("op").unwrap(), "chat");
        assert_eq!(chat.raw_get::<Value>("messages").unwrap(), list);
        assert_eq!(chat.raw_get::<Value>("handle").unwrap(), expected);
    }
}

#[test]
fn act_names_each_call_return_and_raise_with_its_values() {
    let lua = Lua::new();
    let value = |text: &str| Value::String(lua.create_string(text).unwrap());
    let tagged = |tag: &str, values: Vec<Value>| (tag.to_owned(), values);
    let reason = Some(OverflowReason::Precheck);
    let compact = Then::Compact {
        compactor: value("f"),
        reason,
    };
    assert_eq!(
        action(&lua, compact),
        tagged("compactor", vec![value("f"), value("precheck")])
    );
    let compact = Then::Compact {
        compactor: value("f"),
        reason: None,
    };
    assert_eq!(
        action(&lua, compact),
        tagged("compactor", vec![value("f"), Value::Nil])
    );
    let handler = Then::Handle {
        handler: value("h"),
        args: value("a"),
    };
    assert_eq!(
        action(&lua, handler),
        tagged("handler", vec![value("h"), value("a")])
    );
    assert_eq!(action(&lua, Then::Return), tagged("return", Vec::new()));
    assert_eq!(
        action(&lua, Then::Raise(value("x"))),
        tagged("raise", vec![value("x")])
    );
    let plain = Then::RaiseNormalized(value("plain"));
    assert_eq!(action(&lua, plain), tagged("raise", vec![value("plain")]));
    let typed = mlua::Error::external(Error::ContextExhausted {
        reason: OverflowReason::Precheck,
    });
    let new = Raised {
        kind: ErrorKind::EmptyModelReply,
        message: "empty".to_owned(),
        fields: [(
            "finish_reason".to_owned(),
            ErrorField::String("stop".to_owned()),
        )]
        .into(),
    };
    let raised = [
        (
            Then::RaiseNormalized(Value::Error(Box::new(typed))),
            "context_exhausted",
            "precheck",
        ),
        (Then::RaiseNew(new), "empty_model_reply", "stop"),
    ];
    for (then, kind, field) in raised {
        let (tag, values) = action(&lua, then);
        let Value::Table(table) = &values[0] else {
            panic!("an error table");
        };
        let read = |name| table.raw_get::<String>(name).unwrap();
        let field_name = if kind == "context_exhausted" {
            "reason"
        } else {
            "finish_reason"
        };
        assert_eq!((tag.as_str(), read("kind").as_str()), ("raise", kind));
        assert_eq!(read(field_name), field);
    }
}
