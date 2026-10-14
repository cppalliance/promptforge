//! Every shipped prompt under the workspace `prompts/` tree parses offline;
//! the shipped research prompt runs end to end over its web tools.

use std::fs;
use std::path::{Path, PathBuf};

use promptforge::effect::{EffectRecord, ToolCaller};
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use promptforge::{Prompt, RunResult};
use serde_json::json;

use super::support::{Scenario, TestResult, Tool, calls, text};

const SHIPPED_PARSE: &str = "fixture-shipped-prompts";

fn collect_markdown(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read repository prompt directory") {
        let path = entry.expect("read repository prompt entry").path();
        if path.is_dir() {
            collect_markdown(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
}

fn every_prompt_under_parses(directory: &Path) {
    let mut files = Vec::new();
    collect_markdown(directory, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "{} holds no prompts to check",
        directory.display()
    );

    for path in files {
        let source = fs::read_to_string(&path).expect("read prompt fixture");
        Prompt::parse(&source, SHIPPED_PARSE)
            .0
            .unwrap_or_else(|error| panic!("{} must parse: {error}", path.display()));
    }
}

#[test]
fn every_shipped_prompt_parses_offline() {
    every_prompt_under_parses(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts"));
}

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
