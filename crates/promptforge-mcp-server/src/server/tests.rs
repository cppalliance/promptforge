//! Handler tests: routing, argument shapes, and what one call answers with.
//!
//! Admission, the reply deadline, and collecting a run by id are in [`runs`],
//! which shares these fixtures.
//!
//! Almost none of these needs a gateway: every fixture prompt's Lua preamble
//! returns a value, which finishes the run before any model call is made. The
//! exception is the turn count, which is a statement about model round trips
//! and so needs a backend to take one against.

mod runs;

use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use axum::Json;
use axum::Router;
use axum::routing::post;
use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode, JsonObject};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::{PreparedTools, PromptForgeServer};
use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::config::Config;
use crate::progress::McpObserver;
use crate::result::NO_TURNS;
use crate::retrieval::Retrieval;
use promptforge_core::observe::detail;

fn prepared(config: &Config) -> Arc<PreparedTools> {
    static SEED: OnceLock<PreparedTools> = OnceLock::new();
    let seed = SEED
        .get_or_init(|| PreparedTools::new(&config.gateway).expect("prepare fixture tool model"));
    Arc::new(
        seed.rebuild(&config.gateway)
            .expect("index fixture live tools"),
    )
}

/// A prompt that returns its input without calling a model.
fn echo_prompt(name: &str, description: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\nversion: 3\npromptforge: 1\n---\n\n\
         # Test prompt\n\n## Main\n\n```lua\nreturn args\n```\n"
    )
}

/// A prompt whose valid Lua returns an unsupported value at execution.
fn failing_lua_prompt(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Fails on entry\nversion: 1\npromptforge: 1\n---\n\n\
         # Test prompt\n\n## Main\n\n```lua\nreturn {{}}\n```\n"
    )
}

fn capability_prompt(name: &str, capability: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Capability binding fixture\nversion: 1\npromptforge: 1\n---\n\n\
         # Capability prompt\n\n```lua prompt\ntools.need(\"fetch\", \"{capability}\")\n```\n\n\
         ## Main\n\n```lua\ntools.add(\"fetch\")\n```\n\n```lua\nreturn \"bound\"\n```\n"
    )
}

/// Writes `contents` under `root`.
fn write(root: &Path, relative: &str, contents: &str) {
    fs::write(root.join(relative), contents).expect("write the fixture prompt");
}

/// A server over a prompts directory holding three prompts that run offline
/// (`echo`, `greet`, `summarize`) and one whose Lua fails during execution.
fn server() -> (TempDir, PromptForgeServer) {
    server_with("")
}

/// The same server, with `server_lines` added to its `[server]` table.
fn server_with(server_lines: &str) -> (TempDir, PromptForgeServer) {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    let root = dir.path();
    write(root, "echo.md", &echo_prompt("echo", "Echo the input back"));
    write(root, "greet.md", &echo_prompt("greet", "Greet a person"));
    write(
        root,
        "summarize.md",
        &echo_prompt("summarize", "Summarize a document"),
    );
    write(root, "explode.md", &failing_lua_prompt("explode"));

    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n{server_lines}\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n\
         [catalog]\ninclude = [\"*.md\"]\n",
        root.display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::new(Retrieval::idle()),
        tools,
    );
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
async fn the_runner_runs_the_named_prompt_and_reports_the_value_twice() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "echo", "args": "hello" }),
        ))
        .await
        .expect("running a named prompt is not a protocol error");

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
async fn the_runner_reuses_its_returned_run_id_for_parse_bind_and_execution() {
    let (_dir, server) = server();
    let catalog = server.catalog.load();
    let entry = catalog
        .find("echo")
        .expect("the runner lifecycle fixture must exist");
    let observer = Arc::new(McpObserver::silent());

    let result = super::runner::run_recorded(
        server.config.as_ref(),
        &server.registry,
        Arc::clone(&server.tools),
        entry,
        "hello",
        Arc::clone(&observer),
    )
    .await
    .expect("the recorded runner lifecycle must not be a protocol error");

    let structured = structured_of(&result);
    let run_id = structured["run_id"]
        .as_str()
        .expect("the runner must return its run id");
    let records = observer.records();
    assert!(!records.is_empty());
    assert!(
        records.iter().all(|(execution, _, _)| execution == run_id),
        "parse, bind, and execution must reuse the returned run id: {records:#?}"
    );
    let details = records
        .iter()
        .map(|(_, _, detail)| detail.as_str())
        .collect::<Vec<_>>();
    for expected in [
        detail::PARSE_STARTED,
        detail::PARSE_SUCCEEDED,
        detail::TOOL_BINDING_STARTED,
        detail::TOOL_BINDING_SUCCEEDED,
        detail::RUN_STARTED,
        detail::RUN_SUCCEEDED,
    ] {
        assert!(
            details.contains(&expected),
            "the MCP runner lifecycle must include {expected:?}: {records:#?}"
        );
    }
}

#[tokio::test]
async fn the_runner_binds_and_executes_a_bound_prompt_against_the_shared_registry() {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    write(
        dir.path(),
        "bound.md",
        &capability_prompt(
            "bound",
            "Fetch a web page and return its main content as markdown.",
        ),
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
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::new(Retrieval::idle()),
        tools,
    );

    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "bound" })))
        .await
        .expect("binding and running are not protocol errors");

    assert_eq!(result.is_error, Some(false));
    assert_eq!(text_of(&result), "bound");
    assert_eq!(structured_of(&result)["status"], json!("completed"));
}

#[tokio::test]
async fn an_unbindable_capability_fails_before_admission_and_execution() {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    write(
        dir.path(),
        "absent.md",
        &capability_prompt(
            "absent",
            "Control a deep-space telescope's cryogenic mirror actuators.",
        ),
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
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::new(Retrieval::idle()),
        tools,
    );
    let mut slots = Vec::new();
    for _ in 0..4 {
        slots.push(
            server
                .registry
                .admit()
                .await
                .expect("all fixture run slots start free"),
        );
    }

    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "absent" })))
        .await
        .expect("an absent capability is a run failure, not a protocol error");

    assert_eq!(result.is_error, Some(true));
    assert_eq!(structured_of(&result)["status"], json!("failed"));
    assert!(
        text_of(&result).contains("no tool matches capability"),
        "{}",
        text_of(&result)
    );
    drop(slots);
}

#[tokio::test]
async fn a_missing_args_argument_is_the_empty_string() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "echo" })))
        .await
        .expect("omitting args is legal");
    assert_eq!(text_of(&result), "");
}

#[tokio::test]
async fn run_prompt_reaches_any_prompt_in_the_catalog() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "greet", "args": "Ada" }),
        ))
        .await
        .expect("running a named prompt is not a protocol error");
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
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::new(Retrieval::idle()),
        tools,
    );

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
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "echo", "args": ["a"] }),
        ))
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
async fn a_prompt_called_under_its_own_name_is_a_protocol_error() {
    let (_dir, server) = server();

    // No prompt is published as a tool, so its name is not a method here: the
    // only way in is to name it to the runner.
    for name in ["echo", "greet", "summarize", "explode", "nonesuch"] {
        let Err(unroutable) = server.dispatch(CallToolRequestParams::new(name)).await else {
            panic!("{name} is not a tool this server publishes")
        };
        assert_eq!(unroutable.code, ErrorCode::METHOD_NOT_FOUND, "{name}");
    }
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
    assert_eq!(prompts[2]["version"], json!(3));
    assert!(
        prompts.iter().all(|entry| entry.get("direct").is_none()),
        "no prompt has a tool of its own to report"
    );
}

#[tokio::test]
async fn list_prompts_carries_the_problem_that_stops_a_prompt_running() {
    // The catalog is resolved under the reload's rule, where a prompt that
    // fails validation keeps its place carrying its error.
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    let root = dir.path();
    write(root, "good.md", &echo_prompt("good", "Runs"));
    write(
        root,
        "bad.md",
        "---\npromptforge: 1\nname: placeholder\n---\n\n## S\n\np\n",
    );
    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n[catalog]\ninclude = [\"*.md\"]\n",
        root.display()
    ))
    .expect("the fixture configuration parses");
    let catalog = Catalog::resolve(&config, OnBroken::Retain).expect("a reload keeps going");
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::new(Retrieval::idle()),
        tools,
    );

    let result = server
        .dispatch(call("list_prompts", json!({})))
        .await
        .expect("listing is not a protocol error");
    let structured = structured_of(&result);
    let prompts = structured["prompts"]
        .as_array()
        .expect("the listing is an array");
    let broken = prompts
        .iter()
        .find(|entry| entry["name"] == json!("bad"))
        .expect("the broken prompt keeps its place");
    assert!(
        broken["problem"]
            .as_str()
            .is_some_and(|problem| problem.contains("does not parse")),
        "{broken}"
    );
    let healthy = prompts
        .iter()
        .find(|entry| entry["name"] == json!("good"))
        .expect("the healthy prompt is listed");
    assert!(healthy["problem"].is_null());
}

#[tokio::test]
#[cfg(feature = "picker")]
async fn a_published_built_in_called_with_no_arguments_at_all_is_the_client_s_bug() {
    let (_dir, server) = server();
    let error = server
        .dispatch(CallToolRequestParams::new("need_prompt"))
        .await
        .expect_err("the schema declared a required argument");
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
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

/// Spawn a gateway that answers every request with the same assistant message,
/// so a prose section takes exactly one model round trip.
async fn spawn_text_gateway() -> SocketAddr {
    async fn completions(Json(_body): Json<Value>) -> Json<Value> {
        Json(json!({
            "choices": [{ "message": { "role": "assistant", "content": "spoken" } }]
        }))
    }

    let router = Router::new().route("/v1/chat/completions", post(completions));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let addr = listener.local_addr().expect("read the bound address");
    tokio::spawn(async move {
        let _served = axum::serve(listener, router).await;
    });
    addr
}

/// A server over one prompt whose single section is prose, pointed at
/// `gateway`.
fn speaking_server(gateway: SocketAddr) -> (TempDir, PromptForgeServer) {
    speaking_server_with(gateway, "")
}

/// The same server, with `server_lines` added to its `[server]` table.
fn speaking_server_with(gateway: SocketAddr, server_lines: &str) -> (TempDir, PromptForgeServer) {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    write(
        dir.path(),
        "speak.md",
        "---\nname: speak\ndescription: Say something\nversion: 1\npromptforge: 1\n---\n\n\
         # Test prompt\n\n## Only\n\nSay something.\n",
    );
    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n{server_lines}\n\n\
         [gateway]\nurl = \"http://{gateway}/v1\"\ntoken = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n\
         [catalog]\ninclude = [\"*.md\"]\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::new(Retrieval::idle()),
        tools,
    );
    (dir, server)
}

#[tokio::test]
async fn the_reported_turn_total_is_the_one_the_run_took() {
    // The observer is the only thing that knows: a turn happens inside the
    // executor, and the handler learns of it through the event stream.
    let gateway = spawn_text_gateway().await;
    let (_dir, server) = speaking_server(gateway);
    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "speak" })))
        .await
        .expect("running a named prompt is not a protocol error");

    let structured = structured_of(&result);
    assert_eq!(structured["status"], json!("completed"));
    assert_eq!(structured["value"], json!("spoken"));
    assert_eq!(structured["turns"], json!(1), "one prose section, one turn");
}

#[tokio::test]
#[cfg(not(feature = "picker"))]
async fn a_build_without_the_picker_has_no_need_prompt_at_all() {
    let (_dir, server) = server();
    let error = server
        .dispatch(call("need_prompt", json!({ "capability": "anything" })))
        .await
        .expect_err("a tool this build never advertised does not exist");
    assert_eq!(
        error.code,
        ErrorCode::METHOD_NOT_FOUND,
        "publication is the one line dispatch reads"
    );
}

#[tokio::test]
async fn a_run_that_never_started_reports_no_turns() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "echo", "args": "hello" }),
        ))
        .await
        .expect("running a named prompt is not a protocol error");
    assert_eq!(
        structured_of(&result)["turns"],
        json!(NO_TURNS),
        "a Lua-only prompt reaches no model"
    );
}
