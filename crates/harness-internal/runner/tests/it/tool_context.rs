//! The context a tool call lends its tool: the tool writes the run's
//! files through `cx.access()`, the calling section reads what it wrote
//! on its next step, and `cx.origin()` names the section's script as the
//! caller.

use std::sync::Arc;

use harness_capabilities::{
    Capability, CapabilityError, CapabilityId, CapabilityRegistry, Contribution, RunServices, Tool,
    ToolContext,
};
use harness_runner::effect_loop::drive_run;
use harness_runner::prepare::prepare;
use harness_runner::recorder::{MemoryRecorder, RunOutcome};
use promptforge::cancel::CancelHandle;
use promptforge::tools::{ToolError, ToolId, ToolOutput};
use serde_json::{Value, json};

use crate::prepare::services;

/// What the fixture tool writes to `/from-tool.md`.
const WRITTEN: &str = "written through the context";

/// A prompt binding `record` to the fixture tool, calling it from the
/// section's script, and returning its answer beside the file it wrote.
const CALLS_RECORD: &str = "---\nname: calls-record\ndescription: d\npromptforge: 0\n\
    capabilities:\n  - tests/context\ntools:\n  record: tests/context/record\n---\n\n\
    # Title\n\n## Only\n\n```lua\nlocal caller = tools.call('record')\n\
    return caller .. '|' .. store.read('from-tool.md')\n```\n";

/// A fixture tool that writes [`WRITTEN`] to `/from-tool.md` through the
/// access its call lends it, and answers with the caller its origin names.
struct Record {
    id: ToolId,
}

#[async_trait::async_trait]
impl Tool for Record {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn wire_name(&self) -> &str {
        self.id.name()
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "Write a file through the call's access and name the caller."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(&self, cx: ToolContext<'_>, _args: Value) -> Result<ToolOutput, ToolError> {
        cx.access()
            .write("/from-tool.md", WRITTEN.as_bytes())
            .map_err(|error| ToolError::message(format!("record: {error}")))?;
        Ok(ToolOutput::trusted(format!("{:?}", cx.origin().caller)))
    }
}

/// A fixture capability contributing the record tool.
struct Context {
    id: CapabilityId,
}

impl Capability for Context {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "The tool context fixture."
    }

    fn create(&self, _services: &RunServices) -> Result<Contribution, CapabilityError> {
        Ok(Contribution {
            tools: vec![Arc::new(Record {
                id: ToolId::parse("tests/context/record").unwrap(),
            })],
            prelude: None,
        })
    }
}

/// A registry holding the fixture capability.
fn context_registry() -> Arc<CapabilityRegistry> {
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(Context {
            id: CapabilityId::parse("tests/context").unwrap(),
        }))
        .unwrap();
    Arc::new(registry)
}

#[tokio::test]
async fn a_tool_writes_through_its_context_and_the_calling_script_reads_it_back() {
    let recorder = Arc::new(MemoryRecorder::new());
    let prepared = prepare(
        CALLS_RECORD,
        "",
        services(&recorder, Some(context_registry())),
    )
    .await
    .unwrap();
    let outcome = drive_run(
        prepared.run,
        prepared.performers,
        recorder.clone(),
        prepared.run_id,
        CancelHandle::new(),
    )
    .await
    .unwrap();
    let RunOutcome::Completed { final_text } = outcome else {
        panic!("the run completes: {outcome:?}");
    };
    assert_eq!(
        final_text,
        format!("Script|{WRITTEN}"),
        "the tool's origin names the script, and the section reads the file the tool wrote"
    );
}
