//! Fixture tools the execution suites bind and call.

use super::*;

// Wrap/nonce tests live in the untrusted module; these just confirm wiring.

/// A trivial tool that echoes back the `value` argument it is given.
pub(super) struct EchoTool;

#[async_trait::async_trait]
impl TestTool for EchoTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/echo").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Echo the value argument back to the caller."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        })
    }

    async fn call(&self, args: Value) -> std::result::Result<ToolOutput, ToolError> {
        let value = require_string_arg(&args, "value")?;
        Ok(ToolOutput::trusted(format!("echoed: {value}")))
    }
}

/// Reads a required string argument from a fixture tool's call arguments.
///
/// The fixtures declare their arguments `required` in their JSON schema, so a
/// missing or non-string value is a malformed call, not something to paper over
/// with an empty string. Returning a concrete [`ToolError`] makes a malformed
/// fixture call fail loudly instead of silently succeeding on `""`.
pub(super) fn require_string_arg<'a>(
    args: &'a Value,
    key: &str,
) -> std::result::Result<&'a str, ToolError> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| {
        ToolError::message(format!("fixture tool requires a string `{key}` argument"))
            .with_kind(ToolErrorKind::InvalidArguments)
    })
}

/// A tool whose output opts in to guard-wrapping, standing in for a tool
/// like `web_fetch` that returns attacker-controllable text.
pub(super) struct UntrustedEchoTool;

#[async_trait::async_trait]
impl TestTool for UntrustedEchoTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/untrusted_echo").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Echo the value argument back as untrusted external data."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        })
    }

    async fn call(&self, args: Value) -> std::result::Result<ToolOutput, ToolError> {
        let value = require_string_arg(&args, "value")?;
        Ok(ToolOutput::untrusted(format!("echoed: {value}")))
    }
}

/// A tool returning a JSON object as text, bound structured in scheduler
/// fixtures so a script `tools.call` resumes it as a Lua table.
pub(super) struct StructuredFixtureTool {
    /// The exact output text; valid JSON for the happy path, garbage for
    /// the invalid-JSON tool-error path.
    pub(super) body: &'static str,
    /// Whether the output is trusted. An untrusted output is nonce-wrapped
    /// before the structured classification, so even valid JSON fails the
    /// parse - the ordering that restricts structured output to trusted
    /// tools.
    pub(super) trusted: bool,
}

#[async_trait::async_trait]
impl TestTool for StructuredFixtureTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/structured").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "structured"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Return a structured payload."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(&self, _args: Value) -> std::result::Result<ToolOutput, ToolError> {
        Ok(if self.trusted {
            ToolOutput::trusted(self.body)
        } else {
            ToolOutput::untrusted(self.body)
        })
    }
}

/// A tool whose every call fails, standing in for a tool that hits a broken
/// backend, so a test can observe what the loop reports on its way out.
pub(super) struct FailingTool;

#[async_trait::async_trait]
impl TestTool for FailingTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/failing").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Always fail."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(&self, _args: Value) -> std::result::Result<ToolOutput, ToolError> {
        // Attach an inner cause so the executor's error can be checked for source
        // preservation (item 4): the tool error must not be flattened to a string.
        let cause = std::io::Error::other("upstream socket reset");
        Err(
            ToolError::with_source("the tool's own backend failed", cause)
                .with_kind(ToolErrorKind::Backend),
        )
    }
}

pub(super) struct ScopedFixtureTool {
    id: ToolId,
    wire_name: &'static str,
    description: &'static str,
    pub(super) calls: Arc<AtomicUsize>,
}

impl ScopedFixtureTool {
    pub(super) fn new(name: &str, wire_name: &'static str, description: &'static str) -> Self {
        Self {
            id: ToolId::parse(&format!("tests/tools/{name}")).expect("valid id"),
            wire_name,
            description,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl TestTool for ScopedFixtureTool {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn wire_name(&self) -> &str {
        self.wire_name
    }

    fn description(&self) -> &str {
        self.description
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"]
        })
    }

    async fn call(&self, args: Value) -> std::result::Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let value = require_string_arg(&args, "value")?;
        Ok(ToolOutput::trusted(format!(
            "called {} with {value}",
            self.id.name(),
        )))
    }
}
