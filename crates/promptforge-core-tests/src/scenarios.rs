//! Real-model PromptForge text and aliased tool-call scenarios.

use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, ensure};
use promptforge_core::Error;
use promptforge_core::client::GatewayClient;
use promptforge_core::execute::{ResolutionContext, RunOptions, run};
use promptforge_core::observe::{Observer, detail};
use promptforge_core::parser::Prompt;
use promptforge_core::store::StoreRef;
use promptforge_core::tools::{Tool, ToolId};
use promptforge_tool_picker::{
    Catalog, Config, ToolDescriptor, ToolId as PickerToolId, ToolPicker,
};
use serde_json::{Value, json};

const TEXT_EXECUTION: &str = "real-model-text";
const TOOL_EXECUTION: &str = "real-model-tool-call";
const TOOL_CAPABILITY: &str = "Return one deterministic fixture value for one supplied string.";
const TOOL_INPUT: &str = "promptforge-probe";
const TOOL_RESULT_MARKER: &str = "PF_TOOL_RESULT_42";
const TOOL_RESULT: &str = "The function completed. Its exact result is PF_TOOL_RESULT_42. Your final answer must be exactly PF_TOOL_FINAL: PF_TOOL_RESULT_42.";
const TEXT_EPILOG: &str = "TEXT_EPILOG|";
const TOOL_EPILOG: &str = "TOOL_EPILOG|";

const REAL_TEXT: &str = include_str!("../prompts/execution/real-text.md");
const REAL_TOOL_CALL: &str = include_str!("../prompts/execution/real-tool-call.md");

type Record = (String, String, String);

/// Runs both explicit scenarios against one ready local server.
pub(crate) async fn run_all(base_url: &str, api_key: &str, model_alias: &str) -> Result<()> {
    let _ = model_alias;
    run_text(base_url, api_key).await?;
    run_tool_call(base_url, api_key).await
}

#[derive(Debug, Default)]
struct Recorder(Mutex<Vec<Record>>);

impl Recorder {
    fn records(&self) -> Vec<Record> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn detail_count(&self, execution: &str, expected: &str) -> usize {
        self.records()
            .iter()
            .filter(|(record_execution, _, found)| {
                record_execution == execution && found == expected
            })
            .count()
    }
}

impl Observer for Recorder {
    fn observe(&self, execution: &str, section: &str, detail: &str) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((execution.to_owned(), section.to_owned(), detail.to_owned()));
    }
}

#[derive(Debug, Default)]
struct StringFixtureTool {
    calls: Mutex<Vec<Value>>,
}

impl StringFixtureTool {
    fn calls(&self) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl Tool for StringFixtureTool {
    fn id(&self) -> ToolId {
        ToolId::new("real-model-fixtures", "string_fixture")
    }

    fn wire_name(&self) -> &'static str {
        "canonical_string_fixture"
    }

    fn description(&self) -> &'static str {
        TOOL_CAPABILITY
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "value": {
                    "type": "string",
                    "description": "The one string to look up."
                }
            },
            "required": ["value"],
            "additionalProperties": false
        })
    }

    async fn call(&self, arguments: Value) -> promptforge_core::Result<String> {
        let valid = arguments.as_object().is_some_and(|object| {
            object.len() == 1 && object.get("value").and_then(Value::as_str) == Some(TOOL_INPUT)
        });
        if !valid {
            return Err(Error::Lua(format!(
                "real-model fixture received schema-invalid arguments: {arguments}"
            )));
        }
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(arguments);
        Ok(TOOL_RESULT.to_owned())
    }
}

async fn run_text(base_url: &str, api_key: &str) -> Result<()> {
    let observer = Recorder::default();
    let prompt = Prompt::parse(REAL_TEXT, TEXT_EXECUTION, &observer)
        .context("parse execution/real-text.md")?;
    let picker = ToolPicker::build(Catalog::default(), Config::default())
        .context("build empty tool picker")?;
    let models = promptforge_core::model::pinned_qwen_dev_catalog("writer");
    let client = GatewayClient::new(base_url, api_key);
    let result = run(
        &prompt,
        "",
        ResolutionContext {
            picker: &picker,
            models: &models,
        },
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: TEXT_EXECUTION,
            observer: &observer,
            client: Some(client),
            debug: None,
        },
    )
    .await
    .context("run execution/real-text.md")?;

    let reply = result
        .strip_prefix(TEXT_EPILOG)
        .context("real-text result did not prove epilog visibility")?;
    ensure!(!reply.trim().is_empty(), "real-text model reply was empty");
    ensure!(
        reply.contains("PF_TEXT_OK"),
        "real-text reply omitted its requested behavioral marker: {reply:?}"
    );
    ensure!(
        observer.detail_count(TEXT_EXECUTION, detail::MODEL_TURN_COMPLETED) == 1,
        "real-text scenario did not complete in exactly one model turn"
    );
    println!("real-text scenario passed");
    Ok(())
}

async fn run_tool_call(base_url: &str, api_key: &str) -> Result<()> {
    let observer = Recorder::default();
    let tool = Arc::new(StringFixtureTool::default());
    let prompt = Prompt::parse(REAL_TOOL_CALL, TOOL_EXECUTION, &observer)
        .context("parse execution/real-tool-call.md")?;
    let schema = tool.parameters_schema();
    let picker = ToolPicker::build(
        Catalog::new(vec![ToolDescriptor::new(
            PickerToolId::new("real-model-fixtures", "string_fixture"),
            tool.description(),
            schema,
        )]),
        Config::default()
            .with_similarity_floor(0.0)
            .expect("zero is a valid threshold")
            .with_margin(0.0)
            .expect("zero is a valid margin"),
    )
    .context("build deterministic one-tool fixture picker")?;
    let tools: [Arc<dyn Tool>; 1] = [Arc::clone(&tool) as Arc<dyn Tool>];
    let models = promptforge_core::model::pinned_qwen_dev_catalog("writer");

    let client = GatewayClient::new(base_url, api_key);
    let result = run(
        &prompt,
        "",
        ResolutionContext {
            picker: &picker,
            models: &models,
        },
        &tools,
        &StoreRef::memory(),
        RunOptions {
            execution: TOOL_EXECUTION,
            observer: &observer,
            client: Some(client),
            debug: None,
        },
    )
    .await
    .context("run execution/real-tool-call.md")?;

    let reply = result
        .strip_prefix(TOOL_EPILOG)
        .context("real-tool-call result did not prove epilog visibility")?;
    ensure!(
        !reply.trim().is_empty(),
        "tool continuation reply was empty"
    );
    ensure!(
        reply.contains("PF_TOOL_FINAL") && reply.contains(TOOL_RESULT_MARKER),
        "tool continuation did not produce the requested final answer: {reply:?}"
    );
    let calls = tool.calls();
    ensure!(
        calls == [json!({"value": TOOL_INPUT})],
        "expected one schema-valid aliased call, got {calls:?}; final reply was {reply:?}"
    );
    ensure!(
        observer.detail_count(TOOL_EXECUTION, detail::TOOL_CALL_SUCCEEDED) == 1,
        "tool scenario did not dispatch exactly one successful aliased call"
    );
    ensure!(
        observer.detail_count(TOOL_EXECUTION, detail::MODEL_TURN_COMPLETED) == 2,
        "tool scenario did not honor its two-turn budget"
    );
    println!("real-tool-call scenario passed");
    Ok(())
}
