//! Unit tests for the tool surface.
//!
//! The first is a golden: it asserts the whole serialized `tools/list` payload,
//! so a reworded description or a changed schema shows up as a failing test
//! rather than as a silent change to what a model reads. The rest hold the line
//! that decides this surface - the list is the built-ins, it never carries a
//! prompt, and its text reads as a command interpreter rather than as a
//! capability offered for selection.

use std::path::PathBuf;
use std::sync::Arc;

use promptforge_core::model::ModelCatalog;
use promptforge_core::observe::NullObserver;
use promptforge_core::parser::Prompt;

use super::{prompt_value, publishes_built_in, reserved_names, tool_definitions};
use crate::catalog::{Catalog, CatalogHandle, Entry};
use crate::config::Config;
use crate::server::{PreparedTools, PromptForgeServer};

/// The four names this server publishes, in `tools/list` order.
const BUILT_INS: [&str; 4] = ["list_prompts", "run_prompt", "need_prompt", "check_run"];

/// A parsed prompt that runs offline: one section whose Lua returns at once.
fn prompt(name: &str, description: &str) -> (String, Prompt) {
    let source = format!(
        "---\nname: {name}\ndescription: {description}\npromptforge: 1\n---\n\n\
         # Title\n\n## Main\n\n```lua\nreturn args\n```\n"
    );
    let prompt = Prompt::parse(&source, "test-run", &NullObserver::default())
        .expect("the fixture prompt parses");
    (source, prompt)
}

fn entry(name: &str, description: &str) -> Entry {
    let (source, prompt) = prompt(name, description);
    Entry::healthy(PathBuf::from(format!("{name}.md")), source, prompt)
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

/// The configuration and prepared tools every fixture server shares. Built once
/// per test: neither depends on the catalog, and the listing reads neither.
fn fixture_server_parts() -> (Arc<Config>, Arc<PreparedTools>) {
    let config = Config::from_toml_str(
        "[server]\napi_key = \"t\"\n\n[gateway]\nurl = \"http://127.0.0.1:8081/v1/\"\napi_key = \"gw\"\n",
    )
    .expect("the fixture configuration parses");
    let tools = Arc::new(
        PreparedTools::new(&config.gateway, &config.tools, ModelCatalog::empty())
            .expect("prepare fixture tools"),
    );
    (Arc::new(config), tools)
}

/// A server that publishes `catalog`, sharing `config` and `tools`.
fn server_over(
    catalog: Catalog,
    config: &Arc<Config>,
    tools: &Arc<PreparedTools>,
) -> PromptForgeServer {
    PromptForgeServer::new(
        Arc::clone(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::clone(tools),
    )
}

/// The built-in names the real listing handler publishes for `server`.
fn published_by(server: &PromptForgeServer) -> Vec<String> {
    server
        .list_page(None)
        .expect("the listing succeeds")
        .tools
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
#[cfg(not(feature = "picker"))]
fn the_published_tool_list_is_the_no_picker_golden_one() {
    // A build without `picker` publishes three built-ins, not four: the
    // resolver is gated out. Its own golden pins that surface so the
    // feature-off contract cannot silently regress.
    let published = serde_json::to_string_pretty(&tool_definitions()).expect("tools serialize");
    assert_eq!(
        published,
        include_str!("tests/golden-tools-list-no-picker.json").trim_end()
    );
}

#[test]
fn every_published_schema_rejects_unknown_properties() {
    // A strict object schema: an argument the tool never declared is a caller
    // defect the boundary refuses, not one it silently drops.
    for tool in tool_definitions() {
        let schema = serde_json::to_value(&*tool.input_schema).expect("the schema serializes");
        assert_eq!(
            schema["additionalProperties"],
            serde_json::json!(false),
            "{} must reject unknown properties",
            tool.name
        );
    }
}

#[test]
fn the_reserved_names_are_exactly_the_built_in_names() {
    // The one set both listing/dispatch and catalog resolution read. A built-in
    // added to the definitions but not reserved would let a prompt take its
    // name and shadow it; a name reserved but not a built-in would refuse a
    // prompt for no live tool. Pinning the two equal forbids either drift, so a
    // prompt named exactly as any built-in always collides and is refused.
    let mut reserved: Vec<&str> = reserved_names().collect();
    reserved.sort_unstable();
    let mut built_ins = BUILT_INS.to_vec();
    built_ins.sort_unstable();
    assert_eq!(
        reserved, built_ins,
        "the reserved set and the built-in name set must be identical"
    );
    // Each reserved name is dispatchable exactly when this build publishes it.
    for name in reserved_names() {
        assert_eq!(
            publishes_built_in(name),
            names().iter().any(|published| published == name),
            "{name}"
        );
    }
}

#[test]
fn the_tool_surface_never_varies_with_the_catalog() {
    // `tools/list` serves the fixed built-in set and reads no catalog: a prompt
    // is reached by naming it to run_prompt, never by becoming a tool of its
    // own. Prove that through the real listing code path -
    // `PromptForgeServer::list_page`, which `ServerHandler::list_tools`
    // delegates to - by driving two materially different catalogs and asserting
    // the published tools are byte-identical either way. One catalog is plain;
    // the other's own entries take built-in names, including a broken entry
    // named exactly like a built-in, which is the case that would leak through
    // if the listing consulted the catalog at all.
    let (config, tools) = fixture_server_parts();
    let plain = server_over(catalog(), &config, &tools);
    let collides = server_over(
        Catalog::new(vec![
            entry("list_prompts", "A prompt named exactly like a built-in"),
            Entry::broken(
                "run_prompt".to_owned(),
                PathBuf::from("run_prompt.md"),
                "frontmatter is missing description",
            ),
            entry("staker", "Build a stakeholder position report"),
        ]),
        &config,
        &tools,
    );

    let expected: Vec<String> = BUILT_INS
        .into_iter()
        .filter(|name| *name != "need_prompt" || cfg!(feature = "picker"))
        .map(str::to_owned)
        .collect();

    let from_plain = published_by(&plain);
    let from_collides = published_by(&collides);
    assert_eq!(
        from_plain, expected,
        "the real listing over a plain catalog is exactly the built-ins"
    );
    assert_eq!(
        from_collides, expected,
        "and is identical over a catalog whose own entries take built-in names"
    );
    assert_eq!(
        from_plain, from_collides,
        "two materially different catalogs publish the same built-in tools"
    );
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
