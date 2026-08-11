//! Turning run outcomes and call arguments into MCP results.
//!
//! Where a failure lands is decided by who can fix it: a malformed argument
//! shape is the client's own bug and comes back as `-32602`, while anything the
//! calling model can correct comes back as an `Ok` result with `isError` set.
//! These helpers draw that line in one place, so every built-in answers the same
//! way.

use std::time::Duration;

use rmcp::model::{CallToolResult, ContentBlock, ErrorData, JsonObject};
use serde_json::Value;

use crate::result::{RunResult, RunStatus};

/// What a caller collecting an id nobody holds is told: that the id is unknown
/// and how long a finished run stays collectable, so a model that polled too
/// late learns why rather than reading it as a fault.
pub(super) fn unknown_run(run_id: &str, retained: Duration) -> String {
    format!(
        "no run {run_id}. A run is collectable while it is going and for {} after it finishes; anything older has been evicted.",
        humantime::format_duration(retained)
    )
}

/// A run reported as a tool result: the value or the error verbatim in the text
/// block, the whole record in `structuredContent`, and `isError` set when the
/// run failed.
///
/// # Errors
/// Returns `-32603` if the record cannot be serialized.
pub(super) fn run_result(run: &RunResult) -> Result<CallToolResult, ErrorData> {
    let text = run.text();
    let failed = matches!(run.status(), RunStatus::Failed);
    let structured = serde_json::to_value(run.to_wire())
        .map_err(|e| ErrorData::internal_error(format!("render the run result: {e}"), None))?;
    let content = vec![ContentBlock::text(text)];
    let mut result = if failed {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    };
    result.structured_content = Some(structured);
    Ok(result)
}

/// A result the calling model is meant to read and act on.
pub(super) fn text_error(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

/// A required string argument.
///
/// # Errors
/// Returns `-32602` when the argument is absent or is not a string, since the
/// tool's schema declared both.
pub(super) fn required_string(
    arguments: Option<&JsonObject>,
    key: &str,
) -> Result<String, ErrorData> {
    match arguments.and_then(|arguments| arguments.get(key)) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(ErrorData::invalid_params(
            format!("{key} must be a string"),
            None,
        )),
        None => Err(ErrorData::invalid_params(
            format!("{key} is required"),
            None,
        )),
    }
}

/// An optional string argument; an absent field is the empty string.
///
/// An absent field and an explicit `null` are not the same thing: absent means
/// "use the default", which is the empty string, while `null` is a value the
/// schema declared a string, so it is the client's bug rather than a default
/// coerced silently out of a null.
///
/// # Errors
/// Returns `-32602` when the argument is present but is not a string, an
/// explicit `null` included.
pub(super) fn optional_string(
    arguments: Option<&JsonObject>,
    key: &str,
) -> Result<String, ErrorData> {
    match arguments.and_then(|arguments| arguments.get(key)) {
        None => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(ErrorData::invalid_params(
            format!("{key} must be a string"),
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::{ErrorCode, JsonObject};
    use serde_json::{Value, json};

    use super::optional_string;

    fn object(value: Value) -> JsonObject {
        match value {
            Value::Object(map) => map,
            other => panic!("arguments must be an object, got {other}"),
        }
    }

    #[test]
    fn an_absent_optional_string_defaults_but_an_explicit_null_is_a_client_bug() {
        let present = object(json!({ "args": "x" }));
        assert_eq!(
            optional_string(Some(&present), "args").expect("a string is taken as itself"),
            "x"
        );

        // Both an absent field and no arguments at all mean "use the default",
        // which is the empty string.
        let absent = object(json!({}));
        assert_eq!(
            optional_string(Some(&absent), "args").expect("an absent field is the default"),
            ""
        );
        assert_eq!(
            optional_string(None, "args").expect("no arguments is the default"),
            ""
        );

        // An explicit null is a value the schema declared a string, so it is
        // refused rather than coerced silently to the empty string.
        let null = object(json!({ "args": Value::Null }));
        let error = optional_string(Some(&null), "args")
            .expect_err("an explicit null is not the string the schema declared");
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    }
}
