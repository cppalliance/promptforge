//! A fixture Plugin crate's whole Plugin side: a `PACKAGE` label, a
//! `construct` that reads a Host-wide service and names its tool under the
//! name the Host chose, and an `impl Plugin` whose `call` reads a per-run
//! service through the context it is lent.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use promptforge_plugin::testing::TestCall;
use promptforge_plugin::{
    HostServices, OutputTrust, Package, Plugin, PluginFuture, PluginId, ServiceId, ServiceKey,
    ToolContext, ToolDescriptor, ToolError, ToolId, ToolOutput,
};
use serde_json::{Value, json};

/// The Host-wide service `construct` reads.
const GREETING: ServiceKey<str> = ServiceKey::new("acme/greeting");

/// The per-run service a call reads.
const VISITOR: ServiceKey<str> = ServiceKey::new("acme/visitor");

const NEEDS: &[ServiceId] = &[VISITOR.id()];

const PACKAGE: Package = Package::new("acme/greeter", construct)
    .prelude("local plugin = ...\n")
    .needs(NEEDS);

/// The one object every run shares.
struct Greeter {
    greeting: Arc<str>,
    tools: Vec<ToolDescriptor>,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the Package construct signature fixes the argument types"
)]
fn construct(
    name: &PluginId,
    config: Value,
    services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    if !config.is_null() {
        return Err(ToolError::message("greeter takes no configuration"));
    }
    let greeting = services.get(&GREETING).ok_or_else(|| {
        ToolError::message("greeter needs acme/greeting, and this Host provides none")
    })?;
    let id = ToolId::parse(&format!("{name}/greet"))
        .map_err(|e| ToolError::with_source("greeter could not name its tool", e))?;
    let tools = vec![ToolDescriptor::new(
        id,
        "Greets this run's visitor.",
        json!({"type": "object", "properties": {"mark": {"type": "string"}}}),
    )];
    Ok(Arc::new(Greeter { greeting, tools }))
}

impl Plugin for Greeter {
    fn tools(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            match cx.tool().name() {
                "greet" => {
                    let visitor = cx
                        .service(&VISITOR)
                        .ok_or_else(|| ToolError::message("no visitor is in this run"))?;
                    let mark = args.get("mark").and_then(Value::as_str).unwrap_or(".");
                    Ok(ToolOutput::untrusted(format!(
                        "{}, {visitor}{mark}",
                        self.greeting
                    )))
                }
                other => Err(ToolError::message(format!(
                    "greeter has no tool named {other}"
                ))),
            }
        })
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

fn host_wide() -> HostServices {
    let mut services = HostServices::new();
    services
        .provide(&GREETING, Arc::from("Hello"))
        .expect("a valid, new id is accepted");
    services
}

fn installed_as(name: &str) -> Arc<dyn Plugin> {
    let name = PluginId::parse(name).expect("a one-segment Plugin name parses");
    let Ok(plugin) = (PACKAGE.construct)(&name, Value::Null, &host_wide()) else {
        panic!("construct succeeds when its Host-wide service is provided");
    };
    plugin
}

#[test]
fn construct_names_the_tools_under_the_name_the_host_chose() {
    let tools = installed_as("hi").tools();
    let ids: Vec<String> = tools.iter().map(|tool| tool.id.to_string()).collect();
    assert_eq!(ids, ["hi/greet"]);
}

#[test]
fn construct_fails_with_a_tool_error_when_a_host_wide_service_is_missing() {
    let name = PluginId::parse("greeter").expect("a one-segment Plugin name parses");
    let Err(error) = (PACKAGE.construct)(&name, Value::Null, &HostServices::new()) else {
        panic!("construct fails without its Host-wide service");
    };
    assert_eq!(
        error.to_string(),
        "greeter needs acme/greeting, and this Host provides none"
    );
}

#[test]
fn a_call_through_dyn_plugin_answers_from_the_context_it_is_lent() {
    let plugin = installed_as("greeter");
    let tool = plugin.tools()[0].id.clone();
    let mut run = HostServices::new();
    run.provide(&VISITOR, Arc::from("Ada"))
        .expect("a valid, new id is accepted");
    let call = TestCall::new(tool).with_services(run);
    let output = block_on(plugin.call(call.context(), json!({"mark": "!"})))
        .expect("the greet call succeeds");
    assert_eq!(output.text(), "Hello, Ada!");
    assert_eq!(output.trust(), OutputTrust::Untrusted);
}

#[test]
fn a_call_without_the_per_run_service_it_needs_fails() {
    let plugin = installed_as("greeter");
    let call = TestCall::new(plugin.tools()[0].id.clone());
    let error = block_on(plugin.call(call.context(), json!({})))
        .expect_err("the greet call fails without a visitor");
    assert_eq!(error.to_string(), "no visitor is in this run");
}

#[test]
fn a_package_from_new_has_no_prelude_and_no_needs_until_set() {
    const BARE: Package = Package::new("acme/bare", construct);
    const FULL: Package = Package::new("acme/full", construct)
        .prelude("local plugin = ...\n")
        .needs(NEEDS);

    assert_eq!(BARE.name, "acme/bare");
    assert_eq!(BARE.prelude, None);
    assert!(BARE.needs.is_empty());
    assert_eq!(FULL.name, "acme/full");
    assert_eq!(FULL.prelude, Some("local plugin = ...\n"));
    assert_eq!(FULL.needs, [VISITOR.id()]);
    let name = PluginId::parse("hi").expect("a one-segment Plugin name parses");
    for label in [BARE, FULL] {
        let Ok(plugin) = (label.construct)(&name, Value::Null, &host_wide()) else {
            panic!(
                "{}: construct succeeds with its Host-wide service",
                label.name
            );
        };
        let ids: Vec<String> = plugin.tools().iter().map(|t| t.id.to_string()).collect();
        assert_eq!(ids, ["hi/greet"], "{}", label.name);
    }
}

#[test]
fn a_package_shows_its_name_prelude_presence_and_needs_in_debug_output() {
    let shown = format!("{PACKAGE:?}");
    assert!(shown.contains("acme/greeter"), "{shown}");
    assert!(shown.contains("prelude: true"), "{shown}");
    assert!(shown.contains("acme/visitor"), "{shown}");
    assert!(
        !shown.contains("local plugin"),
        "the prelude's text is not shown: {shown}"
    );
}
