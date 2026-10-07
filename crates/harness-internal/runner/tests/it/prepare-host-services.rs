//! The run's services at preparation: a declared Plugin that needs a
//! service serves a call with the provider the run's map holds, and a run
//! whose services lack it is refused naming the service.

use super::*;

use promptforge_plugin::{ServiceId, ServiceKey};

/// The test-only service the fixture needs.
const GREETING: ServiceKey<str> = ServiceKey::new("tests/greeting");

const NEEDS: &[ServiceId] = &[GREETING.id()];

/// The fixture Plugin `greeter`: one tool, `greeter/greet`, answering
/// with the run's greeting.
const GREETER: Package = Package {
    name: "tests/greeter",
    prelude: None,
    needs: NEEDS,
    construct: construct_greeter,
};

/// A prompt declaring the greeter Plugin and returning its greeting.
const GREETS: &str = "---\nname: greets\ndescription: d\npromptforge: 0\n\
    plugins:\n  - greeter\n---\n\n# Title\n\n## Only\n\n\
    ```lua\nreturn tools.call('greeter/greet')\n```\n";

struct Greeter {
    tools: Vec<ToolDescriptor>,
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the Package construct signature fixes the return type"
)]
fn construct_greeter(
    name: &PluginId,
    _config: Value,
    _services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    let greet = ToolDescriptor::new(
        ToolId::parse(&format!("{name}/greet")).unwrap(),
        "greet",
        "Answer with the run's greeting.",
        json!({ "type": "object", "properties": {} }),
    );
    Ok(Arc::new(Greeter { tools: vec![greet] }))
}

impl Plugin for Greeter {
    fn tools(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        _args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let greeting = cx
                .service(&GREETING)
                .ok_or_else(|| ToolError::message("no greeting in this run"))?;
            Ok(ToolOutput::trusted(greeting.to_string()))
        })
    }
}

/// Prepares [`GREETS`] with `run` as the run's services.
async fn prepare_greeter(
    run: HostServices,
) -> (Arc<MemoryRecorder>, Result<Prepared, PrepareError>) {
    let recorder = recorder();
    let mut services = services(&recorder, installing(GREETER));
    services.services = run;
    let prepared = prepare(GREETS, "", services).await;
    (recorder, prepared)
}

#[tokio::test]
async fn a_plugin_serves_a_call_with_the_service_the_runs_map_provides() {
    let mut run = HostServices::new();
    run.provide(&GREETING, Arc::from("hello")).unwrap();
    let (recorder, prepared) = prepare_greeter(run).await;
    let prepared = prepared.expect("the run's service meets the need");
    let outcome = drive_run(
        prepared.run,
        prepared.performers,
        recorder.clone(),
        prepared.run_id,
        CancelHandle::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        completed(outcome),
        "hello",
        "the call read the run's provider"
    );
}

#[tokio::test]
async fn a_plugin_whose_service_the_run_lacks_is_refused_naming_the_service() {
    let (recorder, prepared) = prepare_greeter(HostServices::new()).await;
    let Err(PrepareError::Refused { run_id, error }) = prepared else {
        panic!("a required Plugin without its service refuses the run");
    };
    assert!(
        error
            .to_string()
            .contains("- greeter needs tests/greeting, and the environment provides none"),
        "the notice names the Plugin and the missing service: {error}"
    );
    let effects = recorder
        .records(run_id)
        .into_iter()
        .filter(|record| record.kind == RecordKind::Effect)
        .count();
    assert_eq!(effects, 0, "the Plugin was never called");
}
