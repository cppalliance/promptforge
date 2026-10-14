//! The shipped research prompt runs end to end over its web tools.

use std::fs;
use std::path::Path;

use promptforge::RunResult;
use promptforge::effect::{EffectRecord, ToolCaller};
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use serde_json::json;

use super::support::{Scenario, TestResult, Tool, calls, text};

/// A web tool descriptor under `id`, taking one string `query`.
fn web_tool(id: &str) -> TestResult<ToolDescriptor> {
    Ok(ToolDescriptor::new(
        ToolId::parse(id)?,
        "A web tool.",
        json!({ "type": "object", "properties": { "query": { "type": "string" } } }),
    ))
}

#[test]
fn the_shipped_research_prompt_offers_its_web_tools_by_id_and_runs_end_to_end() -> TestResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts/research-person.md");
    let catalog = ToolCatalog::new(&[web_tool("web/fetch")?, web_tool("web/search")?])?;
    let search = calls("web_search", json!({ "query": "Ada Lovelace" }))?;
    let trace = Scenario::tool(
        Tool::untrusted("web/search", "search results"),
        [search, text("a factual summary")],
    )
    .args("Ada Lovelace")
    .context(move |ctx| ctx.tools(catalog.clone()))
    .run(&fs::read_to_string(path)?)?;
    let RunResult::Ok(summary) = trace.result else {
        return Err(format!("the research prompt did not finish: {:?}", trace.result).into());
    };
    assert_eq!(summary, "a factual summary");

    let (mut advertised, mut tool_calls) = (Vec::new(), Vec::new());
    for logged in &trace.effects {
        match &logged.effect {
            EffectRecord::Chat { tools, .. } => advertised.push(tools.clone()),
            EffectRecord::ToolCall {
                tool,
                alias,
                origin,
                ..
            } => tool_calls.push((tool.to_string(), alias.clone(), origin.caller)),
            _ => {}
        }
    }
    let wire_names = vec!["web_search".to_owned(), "web_fetch".to_owned()];
    assert_eq!(
        advertised,
        [wire_names.clone(), wire_names],
        "both rounds advertise the offered tools under their wire names, in offer order"
    );
    assert_eq!(
        tool_calls,
        [(
            "web/search".to_owned(),
            "web_search".to_owned(),
            ToolCaller::Model
        )],
        "the model's call by wire name reaches the tool its id names"
    );
    Ok(())
}
