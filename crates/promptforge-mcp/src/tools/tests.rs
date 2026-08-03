//! Unit tests for the tool surface.
//!
//! The first is a golden: it asserts the whole serialized `tools/list` payload
//! for a mixed catalog, so a reworded description or a changed schema shows up
//! as a failing test rather than as a silent change to what a model reads.

use std::path::PathBuf;

use promptforge_core::parser::Prompt;

use super::{Shape, built_in_tools, tool_definitions};
use crate::catalog::{Catalog, Entry};
use crate::config::Expose;

/// A parsed prompt that runs offline: one section whose Lua returns at once.
fn prompt(name: &str, description: &str) -> Prompt {
    let source = format!(
        "---\nname: {name}\ndescription: {description}\nversion: 1\npromptforge: 1\n---\n\n\
         # Title\n\n## Main\n\n```lua\nreturn args\n```\n"
    );
    Prompt::parse(&source).expect("the fixture prompt parses")
}

fn entry(name: &str, description: &str, expose: Expose) -> Entry {
    Entry::healthy(
        PathBuf::from(format!("{name}.md")),
        expose,
        prompt(name, description),
    )
}

/// Two direct prompts and one listed one.
fn mixed() -> Catalog {
    Catalog::new(vec![
        entry("echo", "Echo the input back", Expose::Tool),
        entry("greet", "Greet one person by name", Expose::Tool),
        entry(
            "staker",
            "Build a stakeholder position report",
            Expose::List,
        ),
    ])
}

fn names(catalog: &Catalog) -> Vec<String> {
    tool_definitions(catalog)
        .iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

#[test]
#[cfg(feature = "picker")]
fn mixed_catalog_publishes_the_golden_tool_list() {
    let published =
        serde_json::to_string_pretty(&tool_definitions(&mixed())).expect("tools serialize");
    assert_eq!(
        published,
        include_str!("tests/golden-tools-list.json").trim_end()
    );
}

#[test]
fn a_direct_prompt_carries_its_frontmatter_name_and_description() {
    let tools = tool_definitions(&mixed());
    let echo = &tools[0];
    assert_eq!(echo.name, "echo");
    let description = echo
        .description
        .as_deref()
        .expect("a healthy prompt carries a description");
    assert!(
        description.starts_with("Echo the input back"),
        "{description}"
    );
    assert!(description.ends_with(super::PROMPT_VALUE), "{description}");
}

#[test]
fn every_tool_returning_a_prompt_value_says_what_that_value_is() {
    let tools = tool_definitions(&mixed());
    for name in ["echo", "greet", "run_prompt", "check_run"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("the mixed catalog publishes this tool");
        let description = tool
            .description
            .as_deref()
            .expect("the tool carries a description");
        assert!(description.contains(super::PROMPT_VALUE), "{name}");
    }
}

#[test]
fn an_all_direct_catalog_keeps_only_the_collector() {
    let catalog = Catalog::new(vec![
        entry("echo", "Echo the input back", Expose::Tool),
        entry("greet", "Greet one person by name", Expose::Tool),
    ]);
    assert_eq!(names(&catalog), ["echo", "greet", "check_run"]);
}

#[test]
fn an_all_listed_catalog_publishes_no_per_prompt_tool() {
    let catalog = Catalog::new(vec![
        entry("echo", "Echo the input back", Expose::List),
        entry(
            "staker",
            "Build a stakeholder position report",
            Expose::List,
        ),
    ]);
    let published = names(&catalog);
    assert!(!published.iter().any(|name| name == "echo"));
    assert!(!published.iter().any(|name| name == "staker"));
    assert!(published.contains(&"list_prompts".to_owned()));
    assert!(published.contains(&"run_prompt".to_owned()));
    assert!(published.contains(&"check_run".to_owned()));
}

#[test]
fn a_broken_direct_prompt_keeps_its_tool_and_says_why_it_cannot_run() {
    let catalog = Catalog::new(vec![Entry::broken(
        "echo".to_owned(),
        PathBuf::from("echo.md"),
        Expose::Tool,
        "frontmatter is missing description",
    )]);
    let tools = tool_definitions(&catalog);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(
        tools[0].description.as_deref(),
        Some("unavailable: frontmatter is missing description")
    );
}

#[test]
fn the_retrieval_tool_needs_both_a_listed_prompt_and_the_picker_feature() {
    let listed = built_in_tools(Shape {
        listed: true,
        direct: false,
    });
    let has_need_prompt = listed.iter().any(|tool| tool.name == "need_prompt");
    assert_eq!(has_need_prompt, cfg!(feature = "picker"));

    let direct_only = built_in_tools(Shape {
        listed: false,
        direct: true,
    });
    assert!(!direct_only.iter().any(|tool| tool.name == "need_prompt"));
}

#[test]
#[cfg(feature = "picker")]
fn the_retrieval_tool_states_the_register_in_both_places_a_client_may_read() {
    let listed = built_in_tools(Shape {
        listed: true,
        direct: false,
    });
    let need = listed
        .iter()
        .find(|tool| tool.name == "need_prompt")
        .expect("a listed catalog publishes the retrieval tool");
    let schema = serde_json::to_value(&*need.input_schema).expect("the schema serializes");
    let parameter = schema["properties"]["capability"]["description"]
        .as_str()
        .expect("the parameter carries its own description")
        .to_owned();

    // A client may surface the tool's description or the parameter's, so the
    // instruction and both examples have to be readable from either alone.
    for text in [
        need.description
            .as_deref()
            .expect("the tool carries a description"),
        &parameter,
    ] {
        assert!(
            text.contains("the way a tool author would document it"),
            "{text}"
        );
        assert!(
            text.contains("Build a stakeholder position report for one entity."),
            "{text}"
        );
        assert!(text.contains("Herb Sutter"), "{text}");
    }
}

#[test]
fn every_prompt_tool_takes_one_optional_string_named_args() {
    let tools = tool_definitions(&mixed());
    let schema = serde_json::to_value(&*tools[0].input_schema).expect("the schema serializes");
    assert_eq!(
        schema,
        serde_json::json!({
            "type": "object",
            "properties": { "args": { "type": "string" } }
        })
    );
}
