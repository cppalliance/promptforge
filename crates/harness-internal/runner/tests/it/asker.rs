//! The fixture ask Plugin the Harness suites install in place of a real
//! user-input Plugin, which Harness crates may not name: an ask tool that
//! survives stops, a prelude defining `input.ask()`, a need for the run's
//! broker, and a broker that fails with a `ToolError`.

use std::sync::Arc;

use promptforge_plugin::{
    HostServices, Package, Plugin, PluginFuture, PluginId, ServiceId, ServiceKey, ToolContext,
    ToolDescriptor, ToolError, ToolId, ToolOutput,
};
use serde_json::{Value, json};

/// The label: installed with no name, the Plugin is `user-input`.
pub(crate) const ASKER: Package = Package {
    name: "tests/user-input",
    prelude: Some(PRELUDE),
    needs: NEEDS,
    construct,
};

/// The key the run's broker is provided under.
pub(crate) const BROKER: ServiceKey<dyn AskBroker> = ServiceKey::new("tests/input-broker");

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

/// Run services holding `broker` under [`BROKER`].
pub(crate) fn broker_services(broker: Arc<dyn AskBroker>) -> HostServices {
    let mut services = HostServices::new();
    services.provide(&BROKER, broker).unwrap();
    services
}

struct Asker {
    tools: Vec<ToolDescriptor>,
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the Package construct signature fixes the return type"
)]
fn construct(
    name: &PluginId,
    _config: Value,
    _services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    let ask = ToolDescriptor::new(
        ToolId::parse(&format!("{name}/ask")).unwrap(),
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
