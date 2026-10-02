//! Tests for the `promptforge/user-input` capability: its ask tool with
//! and without a broker, the prelude it writes, and the service it needs.

use std::sync::Arc;

use promptforge::cancel::CancelHandle;
use promptforge::capabilities::CapabilityId;
use promptforge::tools::{OutputTrust, ToolErrorKind, ToolId};
use serde_json::json;

use super::{FALLBACK, INPUT_BROKER, USER_INPUT_ASK_TOOL, UserInput};
use crate::{Capability, InputBroker, InputError, RunServices, Tool};

/// A broker whose operator always types the same text.
struct Scripted(&'static str);

#[async_trait::async_trait]
impl InputBroker for Scripted {
    async fn wait(&self) -> Result<String, InputError> {
        Ok(self.0.to_owned())
    }
}

/// A broker whose every wait fails with a hidden cause behind the
/// message.
struct Failing;

#[async_trait::async_trait]
impl InputBroker for Failing {
    async fn wait(&self) -> Result<String, InputError> {
        Err(InputError::with_source(
            "the operator's window closed",
            std::io::Error::other("socket reset"),
        ))
    }
}

/// A broker whose every wait fails with a message and no cause.
struct Withdrawn;

#[async_trait::async_trait]
impl InputBroker for Withdrawn {
    async fn wait(&self) -> Result<String, InputError> {
        Err(InputError::message("the host withdrew the wait"))
    }
}

/// Run services with no broker.
fn headless() -> RunServices {
    RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new())
}

/// Run services whose broker is `broker`.
fn with_broker(broker: impl InputBroker + 'static) -> RunServices {
    let mut services = headless();
    services.insert_input_broker(Arc::new(broker));
    services
}

/// The one tool the capability contributes under `services`.
fn ask_tool(services: &RunServices) -> Arc<dyn Tool> {
    let contribution = UserInput::new()
        .create(services)
        .expect("the capability activates");
    assert_eq!(contribution.tools.len(), 1, "one tool is contributed");
    Arc::clone(&contribution.tools[0])
}

/// The prelude the capability writes under `services`.
fn prelude(services: &RunServices) -> String {
    UserInput::new()
        .create(services)
        .expect("the capability activates")
        .prelude
        .expect("the capability contributes a prelude")
}

#[test]
fn the_capability_is_promptforge_user_input_and_needs_the_input_broker() {
    let capability = UserInput::new();
    assert_eq!(capability.id().to_string(), "promptforge/user-input");
    assert_eq!(capability.needs(), [INPUT_BROKER.id()]);
    assert_eq!(
        capability.needs()[0].to_string(),
        "promptforge/input-broker"
    );
    assert!(capability.conflicts().is_empty());
}

#[test]
fn the_input_broker_literal_parses_as_a_capability_id() {
    let literal = INPUT_BROKER.id().to_string();
    assert_eq!(literal, "promptforge/input-broker");
    CapabilityId::parse(&literal).expect("the input broker's literal is a namespace/name id");
}

#[test]
fn the_ask_tool_sits_under_its_full_id_with_an_empty_schema_and_plain_output() {
    let tool = ask_tool(&headless());
    assert_eq!(USER_INPUT_ASK_TOOL, "promptforge/user-input/ask");
    assert_eq!(
        tool.id(),
        ToolId::parse(USER_INPUT_ASK_TOOL).expect("the full id parses")
    );
    assert!(UserInput::new().id().contains(&tool.id()));
    assert_eq!(tool.wire_name(), "ask");
    assert_eq!(
        tool.description(),
        "Wait for the operator's next message and return its text."
    );
    assert_eq!(
        tool.parameters_schema(),
        json!({ "type": "object", "properties": {} })
    );
    assert!(!tool.structured_output(), "the answer is plain text");
}

#[tokio::test]
async fn the_ask_tool_returns_the_brokers_text_trusted_and_byte_exact() {
    let typed = "  two lines\nwith\ttabs and trailing space  ";
    let tool = ask_tool(&with_broker(Scripted(typed)));
    let output = tool.call(json!({})).await.expect("the broker answers");
    assert_eq!(output.text(), typed, "no trimming or re-encoding");
    assert_eq!(output.trust(), OutputTrust::Trusted);
}

#[tokio::test]
async fn an_operator_who_types_the_fallback_sentence_gets_it_back_as_their_text() {
    let tool = ask_tool(&with_broker(Scripted(FALLBACK)));
    let output = tool.call(json!({})).await.expect("the broker answers");
    assert_eq!(output.text(), FALLBACK);
    assert_eq!(output.trust(), OutputTrust::Trusted);
}

#[tokio::test]
async fn a_broker_failure_becomes_a_tool_error_with_the_brokers_message_and_cause() {
    let tool = ask_tool(&with_broker(Failing));
    let error = tool
        .call(json!({}))
        .await
        .expect_err("a failed wait fails the call");
    assert_eq!(error.to_string(), "the operator's window closed");
    assert_eq!(error.kind(), ToolErrorKind::Backend);
    let mut causes = Vec::new();
    let mut cause = std::error::Error::source(&error);
    while let Some(current) = cause {
        causes.push(current.to_string());
        cause = current.source();
    }
    assert!(
        causes.iter().any(|text| text == "socket reset"),
        "the broker's hidden cause stays behind Error::source: {causes:?}"
    );
}

#[tokio::test]
async fn a_message_only_broker_failure_stays_message_only() {
    let tool = ask_tool(&with_broker(Withdrawn));
    let error = tool
        .call(json!({}))
        .await
        .expect_err("a failed wait fails the call");
    assert_eq!(error.to_string(), "the host withdrew the wait");
    assert_eq!(error.kind(), ToolErrorKind::Backend);
    assert!(std::error::Error::source(&error).is_none());
}

#[tokio::test]
async fn without_a_broker_the_ask_tool_returns_the_fallback_sentence_trusted() {
    let tool = ask_tool(&headless());
    let output = tool
        .call(json!({}))
        .await
        .expect("a host with nobody to ask still answers");
    assert_eq!(
        output.text(),
        "User input is unavailable in this host; continue without it."
    );
    assert_eq!(output.trust(), OutputTrust::Trusted);
}

#[test]
fn the_prelude_writes_connected_as_true_with_a_broker_and_false_without() {
    let connected = prelude(&with_broker(Scripted("unused")));
    let disconnected = prelude(&headless());
    assert!(
        connected.contains("local connected = true\n"),
        "{connected}"
    );
    assert!(
        disconnected.contains("local connected = false\n"),
        "{disconnected}"
    );
    assert_eq!(
        connected.replace("local connected = true\n", "local connected = false\n"),
        disconnected,
        "the two preludes differ only in the connected line"
    );
}

#[test]
fn the_prelude_defines_input_and_calls_the_ask_tool_by_its_full_id() {
    let source = prelude(&headless());
    assert!(source.starts_with("input = {}\n"), "{source}");
    assert!(
        source.contains("tools.call(\"promptforge/user-input/ask\")"),
        "{source}"
    );
    assert!(
        source.contains("error(\"input.ask takes no arguments\", 2)"),
        "{source}"
    );
    assert!(
        !source.contains("user_input"),
        "no user_input global: {source}"
    );
}
