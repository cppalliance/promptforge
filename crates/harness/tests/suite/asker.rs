//! The fixture ask Plugin a suite Host installs in place of a real
//! user-input Plugin, which Harness crates may not name: an ask tool that
//! survives stops, a prelude defining `input.ask()`, a need for the run's
//! broker, and a broker that fails with a `ToolError`.

use std::sync::Arc;

use harness::plugin::{HostContext, HostServices};
use promptforge_plugin::{
    Package, Plugin, PluginFuture, PluginId, ServiceId, ServiceKey, ToolContext, ToolDescriptor,
    ToolError, ToolId, ToolOutput,
};
use serde_json::{Value, json};

/// The label: installed with no name, the Plugin is `user-input`.
const ASKER: Package = Package {
    name: "tests/user-input",
    prelude: Some(PRELUDE),
    needs: NEEDS,
    construct,
};

/// The key the run's broker is provided under.
const BROKER: ServiceKey<dyn AskBroker> = ServiceKey::new("tests/input-broker");

const NEEDS: &[ServiceId] = &[BROKER.id()];

const PRELUDE: &str = r#"local plugin = ...
input = {}
function input.ask(...)
  if select('#', ...) > 0 then
    error("input.ask takes no arguments", 2)
  end
  return tools.call(plugin .. "/ask")
end
"#;

/// Delivers the operator's answer to the fixture's ask tool.
#[async_trait::async_trait]
pub(crate) trait AskBroker: Send + Sync {
    async fn wait(&self) -> Result<String, ToolError>;
}

/// A Host with the fixture installed as `user-input`.
pub(crate) fn asker_host() -> Arc<HostContext> {
    let mut host = HostContext::new(HostServices::new());
    if let Err(error) = host.install(ASKER, None, Value::Null) {
        panic!("an empty Host installs the fixture: {error}");
    }
    Arc::new(host)
}

/// Run services holding `broker` as the run's input broker.
pub(crate) fn broker_services(broker: Arc<dyn AskBroker>) -> HostServices {
    let mut services = HostServices::new();
    if let Err(error) = services.provide(&BROKER, broker) {
        panic!("an empty map takes the broker: {error}");
    }
    services
}

struct Asker {
    tools: Vec<ToolDescriptor>,
}

fn construct(
    name: &PluginId,
    _config: Value,
    _services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    let id = ToolId::parse(&format!("{name}/ask"))
        .map_err(|e| ToolError::with_source("the fixture could not name its ask tool", e))?;
    let ask = ToolDescriptor::new(
        id,
        "ask",
        "Wait for the operator's next message.",
        json!({ "type": "object", "properties": {} }),
    )
    .survives_stop(true);
    Ok(Arc::new(Asker { tools: vec![ask] }))
}

impl Plugin for Asker {
    fn tools(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        _args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let broker = cx
                .service(&BROKER)
                .ok_or_else(|| ToolError::message("no operator is connected to this run"))?;
            broker.wait().await.map(ToolOutput::trusted)
        })
    }
}
