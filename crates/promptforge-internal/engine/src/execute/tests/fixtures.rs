//! Fixture tools the execution suites bind and call.

use super::*;

// Wrap/nonce tests live in the untrusted module; these just confirm wiring.

/// The `value` schema both echo tools share.
fn value_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "value": { "type": "string" } },
        "required": ["value"]
    })
}

/// The schema of a tool that takes no arguments.
fn no_arguments() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// The fixture Plugin's `tools/echo`, which echoes its `value` argument.
#[track_caller]
pub(super) fn echo_tool() -> FnTool {
    FnTool::new("tools/echo", value_schema(), |args| {
        let value = require_string_arg(&args, "value")?;
        Ok(ToolOutput::trusted(format!("echoed: {value}")))
    })
    .description("Echo the value argument back to the caller.")
}

/// The fixture Plugin's `tools/untrusted_echo`, which echoes its `value`
/// argument as untrusted external data, so its output is guard-wrapped.
#[track_caller]
pub(super) fn untrusted_echo_tool() -> FnTool {
    FnTool::new("tools/untrusted_echo", value_schema(), |args| {
        let value = require_string_arg(&args, "value")?;
        Ok(ToolOutput::untrusted(format!("echoed: {value}")))
    })
    .description("Echo the value argument back as untrusted external data.")
}

/// Reads a required string argument from a fixture tool's call arguments.
///
/// The fixtures declare their arguments `required` in their JSON schema, so a
/// missing or non-string value is a malformed call, not something to paper over
/// with an empty string. Returning a concrete [`ToolError`] makes a malformed
/// fixture call fail loudly instead of silently succeeding on `""`.
fn require_string_arg<'a>(args: &'a Value, key: &str) -> std::result::Result<&'a str, ToolError> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| {
        ToolError::message(format!("fixture tool requires a string `{key}` argument"))
            .with_kind(ToolErrorKind::InvalidArguments)
    })
}

/// `tools/structured`, a structured tool returning `body` as its exact
/// output text, so a script `tools.call` resumes valid JSON as a Lua
/// table and garbage as the invalid-JSON tool error. An untrusted output
/// is nonce-wrapped before the structured classification, so even valid
/// JSON fails the parse: the ordering that restricts structured output to
/// trusted tools.
#[track_caller]
pub(super) fn structured_tool(body: &'static str, trusted: bool) -> FnTool {
    FnTool::new("tools/structured", no_arguments(), move |_| {
        Ok(if trusted {
            ToolOutput::trusted(body)
        } else {
            ToolOutput::untrusted(body)
        })
    })
    .description("Return a structured payload.")
    .structured()
}

/// `tools/failing`, whose every call fails, standing in for a tool that
/// hits a broken backend, so a test can observe what the loop reports on
/// its way out.
#[track_caller]
pub(super) fn failing_tool() -> FnTool {
    FnTool::new("tools/failing", no_arguments(), |_| {
        // Attach an inner cause so the executor's error can be checked for source
        // preservation (item 4): the tool error must not be flattened to a string.
        let cause = std::io::Error::other("upstream socket reset");
        Err(
            ToolError::with_source("the tool's own backend failed", cause)
                .with_kind(ToolErrorKind::Backend),
        )
    })
    .description("Always fail.")
}

/// `tools/<name>`, which answers its `value` argument with `called <name>
/// with <value>`, beside the count of its calls.
#[track_caller]
pub(super) fn scoped_tool(name: &str, description: &str) -> (FnTool, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let called = format!("called {name} with");
    let tool = FnTool::new(&format!("tools/{name}"), value_schema(), move |args| {
        counted.fetch_add(1, Ordering::SeqCst);
        let value = require_string_arg(&args, "value")?;
        Ok(ToolOutput::trusted(format!("{called} {value}")))
    })
    .description(description);
    (tool, calls)
}
