//! Every shipped prompt under the workspace `prompts/` tree parses offline,
//! and so does every fixture the Engine keeps as a positive example; the
//! shipped research prompt runs end to end over its web tools.

use std::error::Error;
use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer, EffectRecord, ToolCaller};
use promptforge::model::{
    Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode, ToolCall,
};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};
use promptforge::vfs::perform_vfs_op;
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
use serde_json::json;

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

/// The `valid/` and `execution/` fixture directories are positive examples,
/// so a fixture no test happens to load still has to track the language
/// (`invalid/` is excluded by construction: those are meant to fail).
#[test]
fn every_positive_fixture_parses_offline() {
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../promptforge-internal/engine/tests/prompts");
    every_prompt_under_parses(&fixtures.join("valid"));
    every_prompt_under_parses(&fixtures.join("execution"));
}

/// A web tool descriptor under `id`, taking one string `query`.
fn web_tool(id: &str) -> Result<ToolDescriptor, Box<dyn Error>> {
    Ok(ToolDescriptor::new(
        ToolId::parse(id)?,
        "A web tool.",
        json!({ "type": "object", "properties": { "query": { "type": "string" } } }),
    ))
}

#[test]
fn the_shipped_research_prompt_offers_its_web_tools_by_id_and_runs_end_to_end()
-> Result<(), Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts/research-person.md");
    let prompt = Prompt::parse(&fs::read_to_string(path)?, SHIPPED_PARSE).0?;
    let model = ModelDescriptor::new(
        ModelId::gateway("canned")?,
        "Calls one search, then summarizes",
        NonZeroU32::new(8_192).ok_or("a context window is never zero")?,
        ThinkingMode::Never,
    );
    let catalog = ToolCatalog::new(&[web_tool("web/fetch")?, web_tool("web/search")?])?;
    let ctx = RunContext::new("research", 7, Timestamp::UNIX_EPOCH).model(model);
    let (ctx, requirements) = Environment::new().tools(catalog).prepare(&prompt, ctx);
    if let Some(refusal) = requirements.refusal() {
        return Err(refusal.into());
    }
    let mut run = Run::new(Arc::new(prompt), "Ada Lovelace", ctx);
    let (mut advertised, mut calls) = (Vec::new(), Vec::new());
    let result = loop {
        match run.step() {
            Step::Pending { effects, .. } => {
                for (id, _provenance, effect) in effects {
                    match effect.record() {
                        EffectRecord::Chat { tools, .. } => advertised.push(tools),
                        EffectRecord::ToolCall {
                            tool,
                            alias,
                            origin,
                            ..
                        } => calls.push((tool.to_string(), alias, origin.caller)),
                        _ => {}
                    }
                    let answer = match effect {
                        Effect::Chat { .. } => {
                            let reply = if advertised.len() == 1 {
                                let call = ToolCall::from_parts(
                                    "call_1",
                                    "web_search",
                                    json!({ "query": "Ada Lovelace" }),
                                )?;
                                CompletionResult::ToolCalls(vec![call])
                            } else {
                                CompletionResult::Text("a factual summary".to_owned())
                            };
                            EffectAnswer::Chat(
                                Completion::from_result(reply, "canned").map(Box::new),
                            )
                        }
                        Effect::ToolCall { .. } => {
                            EffectAnswer::ToolCall(Ok(ToolOutput::untrusted("search results")))
                        }
                        Effect::Vfs { access, op } => {
                            EffectAnswer::Vfs(perform_vfs_op(&access, op))
                        }
                        Effect::Timer { .. } => EffectAnswer::Dropped,
                    };
                    run.resume(id, answer);
                }
            }
            Step::Done { result, .. } => break result,
        }
    };
    let RunResult::Ok(text) = result else {
        return Err(format!("the research prompt did not finish: {result:?}").into());
    };
    assert_eq!(text, "a factual summary");
    let wire_names = vec!["web_search".to_owned(), "web_fetch".to_owned()];
    assert_eq!(
        advertised,
        [wire_names.clone(), wire_names],
        "both rounds advertise the offered tools under their wire names, in offer order"
    );
    assert_eq!(
        calls,
        [(
            "web/search".to_owned(),
            "web_search".to_owned(),
            ToolCaller::Model
        )],
        "the model's call by wire name reaches the tool its id names"
    );
    Ok(())
}
