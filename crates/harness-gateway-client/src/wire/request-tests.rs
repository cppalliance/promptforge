//! Tests for the request body: the OpenAI function-call shape of a replayed
//! tool turn, message order, tool wrapping, and the option fields.
//!
//! A `ToolSchema` and an assistant tool-call message have no public
//! constructor, so [`tool_round`] takes both from a facade `Run` that went
//! through one tool round.

use std::fmt::Write as _;
use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::model::{
    Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode, ToolCall,
};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
use serde_json::json;

use super::*;

/// What a run's second `Chat` effect sent, after one tool round.
struct Round {
    messages: Vec<Message>,
    tools: Vec<ToolSchema>,
    options: CompletionOptions,
}

/// One tool a round offers: its name, description, and parameters schema.
type Offered = (&'static str, &'static str, Value);

/// Drives a facade `Run` through one tool round and returns what its second
/// `Chat` effect sent.
///
/// The section adds every tool in `offered` and runs `models.loop` over one
/// user message. The first `Chat` effect is answered with a call to the
/// first tool, `call_1` with `{"value":"one"}`; the tool call with the
/// trusted output `echoed: one`; and the second `Chat` effect, recorded
/// here, with the text `done`.
fn tool_round(offered: &[Offered]) -> Round {
    let (mut slots, mut adds) = (String::new(), String::new());
    for (name, _, _) in offered {
        writeln!(slots, "  {name}: example/test/{name}").expect("a String takes every write");
        writeln!(adds, "tools.add('{name}')").expect("a String takes every write");
    }
    let source = format!(
        "---\nname: round\ndescription: One tool round.\npromptforge: 0\n\
         models:\n  writer: {{}}\ntools:\n{slots}---\n\n# Round\n\n## Ask\n\n\
         ```lua\nmodels.use('writer')\n{adds}local msgs = messages.new()\n\
         msgs:user('echo once')\nmodels.loop(msgs)\nreturn msgs[#msgs].content\n```\n"
    );
    let (parsed, _parse_events) = Prompt::parse(&source, "round");
    let prompt = parsed.expect("the round prompt parses");
    let descriptors: Vec<ToolDescriptor> = offered
        .iter()
        .map(|(name, description, parameters)| {
            let id = ToolId::parse(&format!("example/test/{name}")).expect("a valid tool id");
            ToolDescriptor::new(id, *description, parameters.clone())
        })
        .collect();
    let model = ModelDescriptor::new(
        ModelId::gateway("m").expect("a valid model id"),
        "Plays the model from a script",
        NonZeroU32::new(8_192).expect("8192 is non-zero"),
        ThinkingMode::Never,
    );
    let ctx = RunContext::new("round", 7, Timestamp::UNIX_EPOCH).model(model);
    let catalog = ToolCatalog::new(&descriptors).expect("the offered tools are distinct");
    let (ctx, requirements) = Environment::new().tools(catalog).prepare(&prompt, ctx);
    assert!(requirements.refusal().is_none(), "the round prepares");
    let mut run = Run::new(Arc::new(prompt), "", ctx);
    let mut rounds = Vec::new();
    let result = loop {
        let effects = match run.step() {
            Step::Pending { effects, .. } => effects,
            Step::Done { result, .. } => break result,
        };
        for (id, _provenance, effect) in effects {
            let answer = match effect {
                Effect::Chat {
                    messages,
                    tools,
                    options,
                    ..
                } => {
                    let reply = if rounds.is_empty() {
                        let call =
                            ToolCall::from_parts("call_1", offered[0].0, json!({ "value": "one" }))
                                .expect("a whole call");
                        CompletionResult::ToolCalls(vec![call])
                    } else {
                        CompletionResult::Text("done".to_owned())
                    };
                    rounds.push(Round {
                        messages,
                        tools,
                        options,
                    });
                    EffectAnswer::Chat(Completion::from_result(reply, "m").map(Box::new))
                }
                Effect::ToolCall { .. } => {
                    EffectAnswer::ToolCall(Ok(ToolOutput::trusted("echoed: one")))
                }
                _ => EffectAnswer::Dropped,
            };
            run.resume(id, answer);
        }
    };
    assert!(
        matches!(&result, RunResult::Ok(text) if text == "done"),
        "the round runs to its terminal reply: {result:?}"
    );
    assert_eq!(rounds.len(), 2, "one tool round, then the terminal round");
    rounds.pop().expect("the terminal round was recorded")
}

/// The `echo` tool with a one-string-field schema.
fn echo() -> Offered {
    (
        "echo",
        "Echo a value.",
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
        }),
    )
}

/// Checks every `messages[].tool_calls[]` entry against the OpenAI
/// function-call schema a strict endpoint enforces, returning the
/// offending path for the first violation.
fn check_openai_tool_calls(body: &Value) -> Result<(), String> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "request body had no messages array".to_owned())?;
    for (message_index, message) in messages.iter().enumerate() {
        let Some(calls) = message.get("tool_calls").filter(|calls| !calls.is_null()) else {
            continue;
        };
        let calls = calls.as_array().ok_or_else(|| {
            format!("messages[{message_index}].tool_calls was present but not an array")
        })?;
        for (call_index, call) in calls.iter().enumerate() {
            let path = format!("messages[{message_index}].tool_calls[{call_index}]");
            if !call.is_object() {
                return Err(format!("{path} was not an object"));
            }
            if call.get("type") != Some(&Value::String("function".to_owned())) {
                return Err(format!("{path}.type must be the string \"function\""));
            }
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{path} had no string id"))?;
            if id.trim().is_empty() {
                return Err(format!("{path}.id was blank"));
            }
            let function = call
                .get("function")
                .filter(|function| function.is_object())
                .ok_or_else(|| format!("{path}.function was not an object"))?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{path}.function had no string name"))?;
            if name.trim().is_empty() {
                return Err(format!("{path}.function.name was blank"));
            }
            let raw = function
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("{path}.function.arguments was not a JSON-encoded string")
                })?;
            let decoded = serde_json::from_str::<Value>(raw).map_err(|error| {
                format!("{path}.function.arguments was not valid JSON: {error}")
            })?;
            if !decoded.is_object() {
                return Err(format!(
                    "{path}.function.arguments did not decode to a JSON object"
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn a_replayed_tool_call_turn_serializes_in_the_openai_function_shape() {
    let round = tool_round(&[echo()]);
    let body = build_request_body(&round.messages, None, &round.options);
    assert_eq!(check_openai_tool_calls(&body), Ok(()), "{body}");
    let call = &body["messages"][1]["tool_calls"][0];
    assert_eq!(call["id"], "call_1");
    assert_eq!(call["type"], "function");
    assert_eq!(call["function"]["name"], "echo");
    assert_eq!(call["function"]["arguments"], "{\"value\":\"one\"}");
    assert_eq!(body["messages"][2]["role"], "tool");
    assert_eq!(body["messages"][2]["tool_call_id"], "call_1");
    assert_eq!(body["messages"][2]["content"], "echoed: one");
}

#[test]
fn the_openai_check_names_each_way_a_replayed_call_breaks_the_schema() {
    let round = tool_round(&[echo()]);
    let replayed = build_request_body(&round.messages, None, &round.options);
    let cases = [
        (
            json!({ "id": "call_1", "function": { "name": "echo", "arguments": "{}" } }),
            ".type must be the string \"function\"",
        ),
        (
            json!({ "id": " ", "type": "function", "function": { "name": "echo", "arguments": "{}" } }),
            ".id was blank",
        ),
        (
            json!({ "type": "function", "function": { "name": "echo", "arguments": "{}" } }),
            " had no string id",
        ),
        (
            json!({ "id": "call_1", "type": "function", "name": "echo", "arguments": "{}" }),
            ".function was not an object",
        ),
        (
            json!({ "id": "call_1", "type": "function", "function": { "name": "", "arguments": "{}" } }),
            ".function.name was blank",
        ),
        (
            json!({ "id": "call_1", "type": "function", "function": { "name": "echo", "arguments": { "value": "one" } } }),
            ".function.arguments was not a JSON-encoded string",
        ),
        (
            json!({ "id": "call_1", "type": "function", "function": { "name": "echo", "arguments": "{oops" } }),
            ".function.arguments was not valid JSON",
        ),
        (
            json!({ "id": "call_1", "type": "function", "function": { "name": "echo", "arguments": "[1]" } }),
            ".function.arguments did not decode to a JSON object",
        ),
    ];
    for (call, expected) in cases {
        let mut body = replayed.clone();
        body["messages"][1]["tool_calls"][0] = call;
        let violation = check_openai_tool_calls(&body).expect_err("the call breaks the schema");
        assert!(
            violation.starts_with("messages[1].tool_calls[0]") && violation.contains(expected),
            "expected {expected:?}, got {violation:?}"
        );
    }
}

#[test]
fn messages_go_out_verbatim_in_order() {
    let messages = [
        Message::user("draft text"),
        Message::assistant("done"),
        Message::tool("call_1", "echoed: one"),
    ];
    let body = build_request_body(&messages, None, &CompletionOptions::new("m"));
    assert_eq!(
        body["messages"],
        json!([
            { "role": "user", "content": "draft text" },
            { "role": "assistant", "content": "done" },
            { "role": "tool", "content": "echoed: one", "tool_call_id": "call_1" },
        ])
    );
}

#[test]
fn each_tool_is_wrapped_as_a_function_with_its_schema_in_order() {
    let (_, _, parameters) = echo();
    let round = tool_round(&[
        echo(),
        ("grab", "Grab a value", json!({ "type": "object" })),
    ]);
    let body = build_request_body(
        &[Message::user("hi")],
        Some(&round.tools),
        &CompletionOptions::new("m"),
    );
    assert_eq!(
        body["tools"],
        json!([
            {
                "type": "function",
                "function": {
                    "name": "echo",
                    "description": "Echo a value.",
                    "parameters": parameters,
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "grab",
                    "description": "Grab a value",
                    "parameters": { "type": "object" },
                },
            },
        ])
    );
}

#[test]
fn unset_options_send_no_option_fields() {
    let body = build_request_body(
        &[Message::user("hi")],
        None,
        &CompletionOptions::new("analyst"),
    );
    assert_eq!(body["model"], "analyst");
    for field in ["temperature", "max_tokens", "chat_template_kwargs"] {
        assert!(body.get(field).is_none(), "{field} is absent: {body}");
    }
}

#[test]
fn the_body_always_streams_and_asks_for_usage() {
    let body = build_request_body(
        &[Message::user("hi")],
        None,
        &CompletionOptions::new("analyst"),
    );
    assert_eq!(body["model"], "analyst");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert!(body.get("tools").is_none(), "no tools field without tools");
    assert!(body.get("temperature").is_none());
}

#[test]
fn options_and_tools_reach_the_body() {
    let options = CompletionOptions::new("analyst")
        .with_temperature(0.0)
        .expect("0.0 is valid")
        .with_max_tokens(NonZeroU32::new(128).expect("128 is non-zero"))
        .with_thinking(false);
    let round = tool_round(&[("echo", "Echo.", json!({ "type": "object" }))]);
    let body = build_request_body(&[Message::user("hi")], Some(&round.tools), &options);
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["max_tokens"], 128);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "echo");
}

#[test]
fn an_empty_tool_list_sends_no_tools_field() {
    let body = build_request_body(
        &[Message::user("hi")],
        Some(&[]),
        &CompletionOptions::new("m"),
    );
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
}
