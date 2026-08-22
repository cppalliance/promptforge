//! Real-model PromptForge text and aliased tool-call scenarios.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, ensure};
use promptforge_core::client::{GatewayClient, GatewayEndpoint, SecretString};
use promptforge_core::execute::{ResolutionContext, RunConfig, run};
use promptforge_core::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use promptforge_core::observe::{Observation, Observer};
use promptforge_core::parser::Prompt;
use promptforge_core::store::StoreRef;
use promptforge_core::tools::{Tool, ToolCatalog, ToolError, ToolId, ToolOutput};
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

/// Catalog entry for scenario fixtures: a large switchable-context model so
/// `models.bind` can filter and request thinking without a live `/v1/models`
/// fetch. Defined here (not in core) because it is only test scaffolding.
fn pinned_qwen_dev_catalog(model_alias: &str) -> Result<ModelCatalog> {
    let context = NonZeroU32::new(131_072).context("131072 is a non-zero context window")?;
    let id = ModelId::gateway(model_alias).context("pinned model alias must be valid")?;
    ModelCatalog::new([ModelDescriptor::new(
        id,
        "A careful analysis model suited to structured reasoning and long-context review",
        context,
        ThinkingMode::Switchable,
    )])
    .context("pinned catalog must have a single unique model")
}

/// One correlated observation: the execution id, section, and the typed event.
type Record = (String, String, Observation);

/// Runs both explicit scenarios against one ready local server.
///
/// `model_alias` is the gateway model identity from `/v1/models`; it becomes
/// each scenario catalog's model id so resolution requests the model the server
/// actually exposes rather than a prompt-local binding name.
pub(crate) async fn run_all(base_url: &str, api_key: &str, model_alias: &str) -> Result<()> {
    run_text(base_url, api_key, model_alias).await?;
    run_tool_call(base_url, api_key, model_alias).await
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

    fn detail_count(&self, execution: &str, expected: &Observation) -> usize {
        self.records()
            .iter()
            .filter(|(record_execution, _, found)| {
                record_execution == execution && found == expected
            })
            .count()
    }
}

impl Observer for Recorder {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((execution.to_owned(), section.to_owned(), event));
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
    #[expect(
        clippy::expect_used,
        reason = "the fixture id components are compile-time constants that satisfy ToolId's validation"
    )]
    fn id(&self) -> ToolId {
        ToolId::new("real-model-fixtures", "string_fixture")
            .expect("the fixture tool identity is valid")
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

    async fn call(&self, arguments: Value) -> std::result::Result<ToolOutput, ToolError> {
        let valid = arguments.as_object().is_some_and(|object| {
            object.len() == 1 && object.get("value").and_then(Value::as_str) == Some(TOOL_INPUT)
        });
        if !valid {
            return Err(ToolError::message(format!(
                "real-model fixture received schema-invalid arguments: {arguments}"
            )));
        }
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(arguments);
        Ok(ToolOutput::untrusted(TOOL_RESULT))
    }
}

async fn run_text(base_url: &str, api_key: &str, model_alias: &str) -> Result<()> {
    let observer = Arc::new(Recorder::default());
    let prompt = Prompt::parse(REAL_TEXT, TEXT_EXECUTION, observer.as_ref())
        .context("parse execution/real-text.md")?;
    let picker = ToolPicker::build(Catalog::default(), Config::default())
        .context("build empty tool picker")?;
    let models = pinned_qwen_dev_catalog(model_alias)?;
    let client = GatewayClient::new(
        GatewayEndpoint::new(base_url).context("gateway base URL must be valid")?,
        SecretString::new(api_key).context("gateway key must not be empty")?,
    );
    let result = run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models, &ToolCatalog::default()),
        &StoreRef::memory(),
        RunConfig::new(TEXT_EXECUTION)
            .observer(Arc::clone(&observer) as Arc<dyn Observer>)
            .client(client),
    )
    .await
    .context("run execution/real-text.md")?;

    let reply = result
        .strip_prefix(TEXT_EPILOG)
        .context("real-text result did not prove epilog visibility")?;
    ensure!(
        reply == "PF_TEXT_OK",
        "real-text reply must be exactly its behavioral marker, got: {reply:?}"
    );
    ensure!(
        observer.detail_count(TEXT_EXECUTION, &Observation::ModelTurnCompleted) == 1,
        "real-text scenario did not complete in exactly one model turn"
    );
    println!("real-text scenario passed");
    Ok(())
}

async fn run_tool_call(base_url: &str, api_key: &str, model_alias: &str) -> Result<()> {
    let observer = Arc::new(Recorder::default());
    let tool = Arc::new(StringFixtureTool::default());
    let prompt = Prompt::parse(REAL_TOOL_CALL, TOOL_EXECUTION, observer.as_ref())
        .context("parse execution/real-tool-call.md")?;
    let schema = tool.parameters_schema();
    let config = Config::default()
        .with_similarity_floor(0.0)
        .and_then(|config| config.with_margin(0.0))
        .context("build deterministic one-tool fixture config")?;
    let picker = ToolPicker::build(
        Catalog::new(vec![ToolDescriptor::new(
            PickerToolId::new("real-model-fixtures", "string_fixture"),
            tool.description(),
            schema,
        )]),
        config,
    )
    .context("build deterministic one-tool fixture picker")?;
    let tools: [Arc<dyn Tool>; 1] = [Arc::clone(&tool) as Arc<dyn Tool>];
    let tools = ToolCatalog::new(&tools).context("the fixture tool is unique")?;
    let models = pinned_qwen_dev_catalog(model_alias)?;

    let client = GatewayClient::new(
        GatewayEndpoint::new(base_url).context("gateway base URL must be valid")?,
        SecretString::new(api_key).context("gateway key must not be empty")?,
    );
    let result = run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models, &tools),
        &StoreRef::memory(),
        RunConfig::new(TOOL_EXECUTION)
            .observer(Arc::clone(&observer) as Arc<dyn Observer>)
            .client(client),
    )
    .await
    .context("run execution/real-tool-call.md")?;

    let reply = result
        .strip_prefix(TOOL_EPILOG)
        .context("real-tool-call result did not prove epilog visibility")?;
    let expected_final = format!("PF_TOOL_FINAL: {TOOL_RESULT_MARKER}");
    ensure!(
        reply == expected_final,
        "tool continuation reply must be exactly {expected_final:?}, got: {reply:?}"
    );
    let calls = tool.calls();
    ensure!(
        calls == [json!({"value": TOOL_INPUT})],
        "expected one schema-valid aliased call, got {calls:?}; final reply was {reply:?}"
    );
    ensure!(
        observer.detail_count(TOOL_EXECUTION, &Observation::ToolCallSucceeded) == 1,
        "tool scenario did not dispatch exactly one successful aliased call"
    );
    ensure!(
        observer.detail_count(TOOL_EXECUTION, &Observation::ModelTurnCompleted) == 2,
        "tool scenario did not honor its two-turn budget"
    );
    println!("real-tool-call scenario passed");
    Ok(())
}
