//! The context a tool call lends its Plugin: the Plugin writes the run's
//! files through `cx.access()`, the calling section reads what it wrote
//! on its next step, and `cx.origin()` names the section's script as the
//! caller.

use std::sync::Arc;

use harness_runner::effect_loop::drive_run;
use harness_runner::prepare::prepare;
use harness_runner::recorder::{MemoryRecorder, RunOutcome};
use promptforge::cancel::CancelHandle;
use promptforge_plugin::{
    HostServices, Package, Plugin, PluginFuture, PluginId, ToolContext, ToolDescriptor, ToolError,
    ToolId, ToolOutput,
};
use serde_json::{Value, json};

use crate::prepare::{installing, services};

/// What the fixture tool writes to `/from-tool.md`.
const WRITTEN: &str = "written through the context";

/// A prompt binding `record` to the fixture tool, calling it from the
/// section's script, and returning its answer beside the file it wrote.
const CALLS_RECORD: &str = "---\nname: calls-record\ndescription: d\npromptforge: 0\n\
    plugins:\n  - context\ntools:\n  record: context/record\n---\n\n\
    # Title\n\n## Only\n\n```lua\nlocal caller = tools.call('record')\n\
    return caller .. '|' .. store.read('from-tool.md')\n```\n";

/// The fixture Plugin `context`: one tool, `context/record`, which writes
/// [`WRITTEN`] to `/from-tool.md` through the access its call lends it
/// and answers with the caller its origin names.
const CONTEXT: Package = Package::new("tests/context", construct);

struct Record {
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
    let record = ToolDescriptor::new(
        ToolId::parse(&format!("{name}/record")).unwrap(),
        "Write a file through the call's access and name the caller.",
        json!({ "type": "object", "properties": {} }),
    );
    Ok(Arc::new(Record {
        tools: vec![record],
    }))
}

impl Plugin for Record {
    fn tools(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        _args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            cx.access()
                .write("/from-tool.md", WRITTEN.as_bytes())
                .map_err(|error| ToolError::message(format!("record: {error}")))?;
            Ok(ToolOutput::trusted(format!("{:?}", cx.origin().caller)))
        })
    }
}

#[tokio::test]
async fn a_tool_writes_through_its_context_and_the_calling_script_reads_it_back() {
    let recorder = Arc::new(MemoryRecorder::new());
    let prepared = prepare(CALLS_RECORD, "", services(&recorder, installing(CONTEXT)))
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
