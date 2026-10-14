//! The shipped research prompt runs end to end over its web tools.

use std::fs;
use std::path::Path;

use promptforge::effect::{EffectRecord, ToolCaller};
use promptforge::tools::ToolCatalog;
use serde_json::json;

use super::support::{Scenario, TestResult, Tool, calls, descriptor, text};

#[test]
fn the_shipped_research_prompt_offers_its_web_tools_by_id_and_runs_end_to_end() -> TestResult {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts/research-person.md");
    let query = json!({ "type": "object", "properties": { "query": { "type": "string" } } });
    let catalog = ToolCatalog::new(&[
        descriptor("web/fetch", query.clone())?,
        descriptor("web/search", query)?,
    ])?;
    let search = calls("web_search", json!({ "query": "Ada Lovelace" }))?;
    let trace = Scenario::tool(
        Tool::untrusted("web/search", "search results"),
        [search, text("a factual summary")],
    )
    .args("Ada Lovelace")
    .context(move |ctx| ctx.tools(catalog.clone()))
    .run(&fs::read_to_string(path)?)?;
    assert_eq!(trace.text()?, "a factual summary");

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
