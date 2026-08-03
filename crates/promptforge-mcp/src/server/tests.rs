//! Handler tests.
//!
//! None of these needs a gateway: every fixture prompt's Lua block returns a
//! value, which finishes the run before any model call is made. A turn against
//! the model is the subject of a later step, and only that step needs a backend
//! to talk to.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode, JsonObject};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::{PromptForgeServer, UNCOUNTED_TURNS};
use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::config::Config;

/// A prompt that returns its input without calling a model.
fn echo_prompt(name: &str, description: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\nversion: 3\npromptforge: 1\n---\n\n\
         ## Main\n\n```lua\nreturn args\n```\n"
    )
}

/// A prompt whose Lua block does not compile, so the run starts and fails.
fn broken_lua_prompt(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Fails on entry\nversion: 1\npromptforge: 1\n---\n\n\
         ## Main\n\n```lua\nreturn (\n```\n"
    )
}

/// Writes `contents` under `root`.
fn write(root: &Path, relative: &str, contents: &str) {
    fs::write(root.join(relative), contents).expect("write the fixture prompt");
}

/// A server over a prompts directory holding one direct prompt (`echo`), two
/// listed ones (`greet`, `summarize`), and one whose Lua will not compile.
fn server() -> (TempDir, PromptForgeServer) {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    let root = dir.path();
    write(root, "echo.md", &echo_prompt("echo", "Echo the input back"));
    write(root, "greet.md", &echo_prompt("greet", "Greet a person"));
    write(
        root,
        "summarize.md",
        &echo_prompt("summarize", "Summarize a document"),
    );
    write(root, "explode.md", &broken_lua_prompt("explode"));

    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n\
         [catalog]\ninclude = [\"*.md\"]\ndefault_expose = \"list\"\n\n\
         [prompts.echo]\nexpose = \"tool\"\n",
        root.display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let server = PromptForgeServer::new(Arc::new(config), Arc::new(CatalogHandle::new(catalog)));
    (dir, server)
}

/// A server over a prompts directory whose every prompt has its own tool, so
/// the catalog publishes neither listing tool.
fn all_direct_server() -> (TempDir, PromptForgeServer) {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    let root = dir.path();
    write(root, "echo.md", &echo_prompt("echo", "Echo the input back"));

    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n\
         [catalog]\ninclude = [\"*.md\"]\ndefault_expose = \"tool\"\n",
        root.display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let server = PromptForgeServer::new(Arc::new(config), Arc::new(CatalogHandle::new(catalog)));
    (dir, server)
}

/// A `tools/call` request for `name` with the given arguments.
fn call(name: &'static str, arguments: Value) -> CallToolRequestParams {
    let arguments: JsonObject = match arguments {
        Value::Object(map) => map,
        other => panic!("arguments must be an object, got {other}"),
    };
    CallToolRequestParams::new(name).with_arguments(arguments)
}

/// The text of a result's single content block.
fn text_of(result: &CallToolResult) -> String {
    let [block] = result.content.as_slice() else {
        panic!("expected exactly one content block")
    };
    block
        .as_text()
        .expect("the content block should be text")
        .text
        .clone()
}

/// A result's `structuredContent`.
fn structured_of(result: &CallToolResult) -> Value {
    result
        .structured_content
        .clone()
        .expect("every run result carries structured content")
}

#[tokio::test]
async fn a_direct_tool_runs_its_prompt_and_reports_the_value_twice() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call("echo", json!({ "args": "hello" })))
        .await
        .expect("a direct call is not a protocol error");

    assert_eq!(result.is_error, Some(false));
    assert_eq!(text_of(&result), "hello");
    let structured = structured_of(&result);
    assert_eq!(structured["value"], json!("hello"));
    assert_eq!(structured["status"], json!("completed"));
    assert_eq!(structured["prompt"], json!("echo"));
    assert_eq!(structured["version"], json!(3));
    assert!(
        structured["run_id"]
            .as_str()
            .is_some_and(|id| id.len() == 32),
        "a run carries an identifier: {structured}"
    );
}

#[tokio::test]
async fn a_missing_args_argument_is_the_empty_string() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call("echo", json!({})))
        .await
        .expect("omitting args is legal");
    assert_eq!(text_of(&result), "");
}

#[tokio::test]
async fn run_prompt_reaches_a_listed_prompt() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "greet", "args": "Ada" }),
        ))
        .await
        .expect("running a listed prompt is not a protocol error");
    assert_eq!(text_of(&result), "Ada");
    assert_eq!(structured_of(&result)["prompt"], json!("greet"));
}

#[tokio::test]
async fn a_name_differing_in_case_or_separator_still_resolves() {
    let (_dir, server) = server();
    for spelling in ["GREET", "Greet", "greet"] {
        let result = server
            .dispatch(call(
                "run_prompt",
                json!({ "prompt": spelling, "args": "Ada" }),
            ))
            .await
            .expect("a lenient spelling is not a protocol error");
        assert_eq!(
            structured_of(&result)["prompt"],
            json!("greet"),
            "{spelling} should resolve to greet"
        );
    }
}

#[tokio::test]
async fn a_hyphen_resolves_to_the_underscore_it_stands_for() {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    write(
        dir.path(),
        "research.md",
        &echo_prompt("research_person", "Research one person"),
    );
    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n[catalog]\ninclude = [\"*.md\"]\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let server = PromptForgeServer::new(Arc::new(config), Arc::new(CatalogHandle::new(catalog)));

    let result = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "Research-Person", "args": "x" }),
        ))
        .await
        .expect("a hyphenated spelling is not a protocol error");
    assert_eq!(structured_of(&result)["prompt"], json!("research_person"));
}

#[tokio::test]
async fn an_unresolvable_name_lists_the_catalog_nearest_first_and_runs_nothing() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "greeet" })))
        .await
        .expect("a bad name is an answer, not a protocol error");

    assert_eq!(result.is_error, Some(true));
    assert!(
        result.structured_content.is_none(),
        "nothing ran, so there is no run to report"
    );
    let text = text_of(&result);
    assert!(text.contains("greeet"), "the miss is quoted back: {text}");
    let listed = text
        .split_once("closest first: ")
        .map(|(_, rest)| rest.trim_end_matches('.').to_owned())
        .expect("the message lists the catalog");
    let names: Vec<&str> = listed.split(", ").collect();
    assert_eq!(names[0], "greet", "the near miss comes first: {listed}");
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        ["echo", "explode", "greet", "summarize"],
        "every enabled name is offered: {listed}"
    );
}

#[tokio::test]
async fn a_malformed_argument_shape_is_a_protocol_error() {
    let (_dir, server) = server();

    let Err(missing) = server.dispatch(call("run_prompt", json!({}))).await else {
        panic!("a missing prompt name is the client's bug")
    };
    assert_eq!(missing.code, ErrorCode::INVALID_PARAMS);

    let Err(mistyped) = server
        .dispatch(call("run_prompt", json!({ "prompt": 7 })))
        .await
    else {
        panic!("a non-string prompt name is the client's bug")
    };
    assert_eq!(mistyped.code, ErrorCode::INVALID_PARAMS);

    let Err(bad_args) = server
        .dispatch(call("echo", json!({ "args": ["a"] })))
        .await
    else {
        panic!("a non-string args is the client's bug")
    };
    assert_eq!(bad_args.code, ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn a_prompt_that_fails_reports_the_failure_as_a_result() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "explode" })))
        .await
        .expect("a failing run is an answer, not a protocol error");

    assert_eq!(result.is_error, Some(true));
    let structured = structured_of(&result);
    assert_eq!(structured["status"], json!("failed"));
    assert!(structured["value"].is_null());
    assert!(
        !text_of(&result).is_empty(),
        "the failure text carries the error"
    );
}

#[tokio::test]
async fn a_tool_this_catalog_does_not_publish_is_a_protocol_error() {
    let (_dir, server) = server();

    // `greet` is listed, not direct, so it has no tool of its own.
    let Err(listed) = server.dispatch(call("greet", json!({}))).await else {
        panic!("a listed prompt has no direct tool")
    };
    assert_eq!(listed.code, ErrorCode::METHOD_NOT_FOUND);

    let Err(absent) = server.dispatch(call("nonesuch", json!({}))).await else {
        panic!("an unpublished name is unroutable")
    };
    assert_eq!(absent.code, ErrorCode::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn list_prompts_reports_every_enabled_prompt() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call("list_prompts", json!({})))
        .await
        .expect("listing is not a protocol error");

    let structured = structured_of(&result);
    let prompts = structured["prompts"]
        .as_array()
        .expect("the listing is an array")
        .clone();
    let names: Vec<&str> = prompts
        .iter()
        .map(|entry| entry["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, ["echo", "explode", "greet", "summarize"]);
    assert_eq!(prompts[0]["direct"], json!(true));
    assert_eq!(prompts[2]["direct"], json!(false));
    assert_eq!(prompts[2]["version"], json!(3));
}

#[tokio::test]
async fn the_built_ins_a_later_step_answers_say_so() {
    let (_dir, server) = server();
    let published: &[&str] = if cfg!(feature = "picker") {
        &["check_run", "need_prompt"]
    } else {
        &["check_run"]
    };
    for name in published {
        let request = CallToolRequestParams::new(*name);
        let result = server
            .dispatch(request)
            .await
            .expect("an unanswered built-in is a result, not a protocol error");
        assert_eq!(result.is_error, Some(true));
        assert!(
            text_of(&result).contains(name),
            "the message should name the tool"
        );
    }
}

#[tokio::test]
async fn a_built_in_this_catalog_does_not_publish_is_a_protocol_error() {
    let (_dir, server) = all_direct_server();

    // Nothing is listed, so the listing tools are absent from `tools/list` and
    // a call to either asks for something this server does not have.
    for name in ["list_prompts", "run_prompt", "need_prompt"] {
        let Err(unpublished) = server.dispatch(CallToolRequestParams::new(name)).await else {
            panic!("{name} is not published by an all-direct catalog")
        };
        assert_eq!(unpublished.code, ErrorCode::METHOD_NOT_FOUND, "{name}");
    }

    // The collector is published whenever anything is, since a direct call can
    // outlive its deadline too.
    let collector = server
        .dispatch(CallToolRequestParams::new("check_run"))
        .await
        .expect("check_run is published here");
    assert_eq!(collector.is_error, Some(true));
}

#[tokio::test]
#[cfg(not(feature = "picker"))]
async fn without_the_picker_the_retrieval_tool_is_not_a_method_at_all() {
    let (_dir, server) = server();
    let Err(absent) = server
        .dispatch(CallToolRequestParams::new("need_prompt"))
        .await
    else {
        panic!("need_prompt is unpublished without the picker feature")
    };
    assert_eq!(absent.code, ErrorCode::METHOD_NOT_FOUND);
}

/// Step 7 wires the counting observer in, and this assertion is what makes
/// forgetting it loud: the moment a run reports a turn it took, this fails and
/// the constant behind it has to go.
#[tokio::test]
async fn step_7_must_replace_the_uncounted_turn_total() {
    assert_eq!(UNCOUNTED_TURNS, 0);
    let (_dir, server) = server();
    let result = server
        .dispatch(call("echo", json!({ "args": "hello" })))
        .await
        .expect("a direct call is not a protocol error");
    assert_eq!(
        structured_of(&result)["turns"],
        json!(0),
        "nothing counts turns until step 7 replaces the null observer"
    );
}
