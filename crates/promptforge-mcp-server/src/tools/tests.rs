//! Unit tests for the tool surface.
//!
//! The first is a golden: it asserts the whole serialized `tools/list` payload,
//! so a reworded description or a changed schema shows up as a failing test
//! rather than as a silent change to what a model reads. The rest hold the line
//! that decides this surface - the list is the built-ins, it never carries a
//! prompt, and its text reads as a command interpreter rather than as a
//! capability offered for selection.

use std::path::PathBuf;

use promptforge_core::observe::NullObserver;
use promptforge_core::parser::Prompt;

use super::{prompt_value, publishes_built_in, tool_definitions};
use crate::catalog::{Catalog, Entry};

/// The four names this server publishes, in `tools/list` order.
const BUILT_INS: [&str; 4] = ["list_prompts", "run_prompt", "need_prompt", "check_run"];

/// A parsed prompt that runs offline: one section whose Lua returns at once.
fn prompt(name: &str, description: &str) -> Prompt {
    let source = format!(
        "---\nname: {name}\ndescription: {description}\nversion: 1\npromptforge: 1\n---\n\n\
         # Title\n\n## Main\n\n```lua\nreturn args\n```\n"
    );
    Prompt::parse(&source, &NullObserver).expect("the fixture prompt parses")
}

fn entry(name: &str, description: &str) -> Entry {
    Entry::healthy(
        PathBuf::from(format!("{name}.md")),
        prompt(name, description),
    )
}

/// Three prompts, which is three more than the tool list ever reports.
fn catalog() -> Catalog {
    Catalog::new(vec![
        entry("echo", "Echo the input back"),
        entry("greet", "Greet one person by name"),
        entry("staker", "Build a stakeholder position report"),
    ])
}

fn names() -> Vec<String> {
    tool_definitions()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

#[test]
#[cfg(feature = "picker")]
fn the_published_tool_list_is_the_golden_one() {
    let published = serde_json::to_string_pretty(&tool_definitions()).expect("tools serialize");
    assert_eq!(
        published,
        include_str!("tests/golden-tools-list.json").trim_end()
    );
}

#[test]
fn a_catalog_of_three_prompts_publishes_only_the_built_ins() {
    assert_eq!(catalog().len(), 3);
    let published = names();
    let expected: Vec<&str> = BUILT_INS
        .into_iter()
        .filter(|name| *name != "need_prompt" || cfg!(feature = "picker"))
        .collect();
    assert_eq!(published, expected);
}

#[test]
fn no_prompt_name_is_ever_a_tool_name() {
    // Every catalog shape the resolver can produce: healthy prompts, a broken
    // one, and a prompt named the way a built-in is described.
    let catalogs = [
        catalog(),
        Catalog::new(vec![Entry::broken(
            "echo".to_owned(),
            PathBuf::from("echo.md"),
            "frontmatter is missing description",
        )]),
        Catalog::new(vec![entry("run_the_prompts", "List and run everything")]),
    ];
    for catalog in &catalogs {
        for entry in catalog.entries() {
            assert!(
                !names().iter().any(|name| name == entry.name()),
                "{} reached tools/list",
                entry.name()
            );
        }
    }
}

#[test]
fn the_two_tools_returning_a_prompt_value_say_what_that_value_is() {
    let tools = tool_definitions();
    for name in ["run_prompt", "check_run"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("this server publishes the tool");
        let description = tool
            .description
            .as_deref()
            .expect("the tool carries a description");
        assert!(description.contains(prompt_value!()), "{name}");
    }
}

#[test]
fn every_description_reads_as_a_command_rather_than_as_an_offer() {
    // Decision 6: the text says what this server executes and that a caller
    // names what to run. Trigger phrasing, a claim on a situation, or an
    // invitation to go looking is what this rules out.
    let forbidden = [
        "use this when",
        "use when",
        "whenever you",
        "if you need",
        "you should",
        "helpful",
        "useful",
        "anytime",
        "proactively",
    ];
    let mut texts: Vec<String> = vec![crate::server::INSTRUCTIONS.to_owned()];
    for tool in tool_definitions() {
        texts.push(
            tool.description
                .as_deref()
                .expect("every published tool carries a description")
                .to_owned(),
        );
        texts.push(serde_json::to_string(&*tool.input_schema).expect("the schema serializes"));
    }
    for text in &texts {
        let lowered = text.to_lowercase();
        for phrase in forbidden {
            assert!(!lowered.contains(phrase), "{phrase:?} appears in: {text}");
        }
    }
}

#[test]
fn the_session_instructions_say_a_caller_names_what_to_run() {
    let instructions = crate::server::INSTRUCTIONS;
    assert!(
        instructions.contains("executes PromptForge prompts"),
        "{instructions}"
    );
    assert!(instructions.contains("names one"), "{instructions}");
    assert!(instructions.contains(prompt_value!()), "{instructions}");
}

#[test]
fn the_dispatcher_publishes_exactly_what_the_listing_does() {
    for name in BUILT_INS {
        assert_eq!(
            publishes_built_in(name),
            names().iter().any(|published| published == name),
            "{name}"
        );
    }
    assert!(!publishes_built_in("echo"));
    assert_eq!(publishes_built_in("need_prompt"), cfg!(feature = "picker"));
}

#[test]
#[cfg(feature = "picker")]
fn the_resolver_states_the_register_in_both_places_a_client_may_read() {
    let tools = tool_definitions();
    let need = tools
        .iter()
        .find(|tool| tool.name == "need_prompt")
        .expect("the picker build publishes the resolver");
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
fn the_runner_takes_a_required_prompt_name_and_one_optional_string() {
    let tools = tool_definitions();
    let runner = tools
        .iter()
        .find(|tool| tool.name == "run_prompt")
        .expect("the runner is always published");
    let schema = serde_json::to_value(&*runner.input_schema).expect("the schema serializes");
    assert_eq!(schema["required"], serde_json::json!(["prompt"]));
    assert_eq!(schema["properties"]["prompt"]["type"], "string");
    assert_eq!(schema["properties"]["args"]["type"], "string");
}
