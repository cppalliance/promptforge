//! The `user-input` Plugin through its public label: what `construct`
//! builds under the name the Host chose, what the ask tool answers from
//! the run's broker, and the prelude a declaring prompt runs.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use plugin_user_input::{ASK, INPUT_BROKER, InputBroker, PACKAGE};
use promptforge_plugin::testing::TestCall;
use promptforge_plugin::{
    HostServices, OutputTrust, Plugin, PluginId, ToolError, ToolErrorKind, ToolId,
};
use serde_json::{Value, json};

/// A broker whose operator always types the same text.
struct Scripted(&'static str);

#[async_trait::async_trait]
impl InputBroker for Scripted {
    async fn wait(&self) -> Result<String, ToolError> {
        Ok(self.0.to_owned())
    }
}

/// A broker whose every wait fails with a hidden cause behind the
/// message.
struct Failing;

#[async_trait::async_trait]
impl InputBroker for Failing {
    async fn wait(&self) -> Result<String, ToolError> {
        Err(ToolError::with_source(
            "the operator's window closed",
            std::io::Error::other("socket reset"),
        ))
    }
}

/// Drives a future that never pends to its output.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
    }
}

/// The Plugin installed under `name` with `config`.
fn construct(name: &str, config: Value) -> Result<Arc<dyn Plugin>, ToolError> {
    let name = PluginId::parse(name).expect("a one-segment Plugin name parses");
    (PACKAGE.construct)(&name, config, &HostServices::new())
}

fn installed(name: &str) -> Arc<dyn Plugin> {
    let Ok(plugin) = construct(name, Value::Null) else {
        panic!("construct succeeds with no configuration");
    };
    plugin
}

/// Run services whose broker is `broker`.
fn with_broker(broker: impl InputBroker + 'static) -> HostServices {
    let mut services = HostServices::new();
    let broker: Arc<dyn InputBroker> = Arc::new(broker);
    services
        .provide(&INPUT_BROKER, broker)
        .expect("an empty map takes the broker");
    services
}

/// Asks once through the Plugin installed as `user-input`, with `services`
/// as the run's services.
fn ask(services: HostServices) -> Result<promptforge_plugin::ToolOutput, ToolError> {
    let plugin = installed("user-input");
    let call = TestCall::new(plugin.tools()[0].id.clone()).with_services(services);
    block_on(plugin.call(call.context(), json!({})))
}

#[test]
fn the_package_is_promptforge_user_input_and_needs_the_input_broker() {
    assert_eq!(PACKAGE.name, "promptforge/user-input");
    assert_eq!(PACKAGE.needs, [INPUT_BROKER.id()]);
    assert_eq!(PACKAGE.needs[0].to_string(), "promptforge/input-broker");
    assert_eq!(ASK, "ask");
}

#[test]
fn construct_names_the_ask_tool_under_the_installed_name_and_marks_it_to_survive_a_stop() {
    let tools = installed("operator").tools();
    assert_eq!(tools.len(), 1, "one tool: {tools:?}");
    let ask = &tools[0];
    assert_eq!(
        ask.id,
        ToolId::parse("operator/ask").expect("the id parses")
    );
    assert_eq!(
        ask.description,
        "Wait for the operator's next message and return its text."
    );
    assert_eq!(
        ask.parameters_schema,
        json!({ "type": "object", "properties": {} })
    );
    assert!(ask.survives_stop, "a stop leaves the question open");
    assert!(!ask.structured_output, "the answer is plain text");
}

#[test]
fn construct_accepts_no_configuration_or_an_empty_object_and_refuses_anything_else() {
    assert!(construct("user-input", json!({})).is_ok());
    let Err(error) = construct("user-input", json!({ "greeting": "hi" })) else {
        panic!("a configuration with settings is refused");
    };
    assert_eq!(error.to_string(), "user-input takes no configuration");
}

#[test]
fn the_ask_tool_returns_the_brokers_text_trusted_and_byte_exact() {
    let typed = "  two lines\nwith\ttabs and trailing space  ";
    let output = ask(with_broker(Scripted(typed))).expect("the broker answers");
    assert_eq!(output.text(), typed, "no trimming or re-encoding");
    assert_eq!(output.trust(), OutputTrust::Trusted);
}

#[test]
fn a_broker_failure_is_the_ask_calls_error_unchanged() {
    let error = ask(with_broker(Failing)).expect_err("a failed wait fails the call");
    assert_eq!(error.to_string(), "the operator's window closed");
    assert_eq!(error.kind(), ToolErrorKind::Backend);
    let cause = std::error::Error::source(&error).map(ToString::to_string);
    assert_eq!(cause.as_deref(), Some("socket reset"));
}

#[test]
fn a_call_without_a_broker_fails_saying_no_operator_is_connected() {
    let error = ask(HostServices::new()).expect_err("no broker, no answer");
    assert_eq!(error.to_string(), "no operator is connected to this run");
}

#[test]
fn the_prelude_reads_the_installed_name_and_asks_through_it() {
    let prelude = PACKAGE.prelude.expect("the Plugin has a prelude");
    assert!(prelude.starts_with("local plugin = ...\n"), "{prelude}");
    assert!(
        prelude.contains("tools.call(plugin .. \"/ask\")"),
        "{prelude}"
    );
    assert!(
        prelude.contains("error(\"input.ask takes no arguments\", 2)"),
        "{prelude}"
    );
    assert!(
        !prelude.contains("connected"),
        "input.connected is gone: {prelude}"
    );
}
