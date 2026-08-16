//! Routing, resolution, and argument-shape tests: what one `run_prompt` (and a
//! misrouted call) answers with, short of the admission and deadline machinery
//! in [`super::runs`].

use std::sync::Arc;

use promptforge_core::observe::Observation;
use rmcp::model::{CallToolRequestParams, ErrorCode};
use serde_json::json;

use crate::progress::McpObserver;

use super::{
    Catalog, CatalogHandle, Config, OnBroken, PromptForgeServer, call, capability_prompt,
    echo_prompt, prepared, server, structured_of, text_of, write,
};

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
    assert!(
        structured.get("version").is_none(),
        "retired author versions must not reach run JSON: {structured}"
    );
    assert!(
        structured["run_id"]
            .as_str()
            .is_some_and(|id| id.len() == 32),
        "a run carries an identifier: {structured}"
    );
}

#[tokio::test]
async fn a_call_with_no_progress_token_answers_the_same() {
    // `dispatch` is the untokened entry point the transport calls for a
    // `tools/call` that carried no `progressToken` - a request no `rmcp` client
    // can produce, since a client always attaches one. With no peer to report
    // to there is no channel and no pump task, and the caller must not be able
    // to tell that from the answer.
    let (_dir, server) = server();
    let result = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "echo", "args": "hi" }),
        ))
        .await
        .expect("the call reaches the prompt");

    assert_eq!(result.is_error, Some(false));
    assert_eq!(text_of(&result), "hi");
    let structured = structured_of(&result);
    assert_eq!(structured["status"], json!("completed"));
    assert_eq!(structured["value"], json!("hi"));
    assert_eq!(structured["turns"], json!(0));
}

#[tokio::test]
async fn the_runner_reuses_its_returned_run_id_for_parse_and_execution() {
    let (_dir, server) = server();
    let generation = server.catalog.load();
    let entry = generation
        .catalog()
        .find("echo")
        .expect("the runner lifecycle fixture must exist");
    let observer = Arc::new(McpObserver::silent());

    let result = crate::server::runner::run_recorded(
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
        "parse and execution must reuse the returned run id: {records:#?}"
    );
    let details = records
        .iter()
        .map(|(_, _, detail)| detail.clone())
        .collect::<Vec<_>>();
    for expected in [
        Observation::ParseStarted,
        Observation::ParseSucceeded,
        Observation::RunStarted,
        Observation::RunSucceeded,
    ] {
        assert!(
            details.contains(&expected.to_string()),
            "the MCP runner lifecycle must include {expected:?}: {records:#?}"
        );
    }
}

#[tokio::test]
async fn the_runner_resolves_and_executes_against_the_shared_registry() {
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
        "[server]\napi_key = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n[catalog]\ninclude = [\"*.md\"]\n\n\
         [tools]\nweb_fetch = true\nweb_search = true\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
    );

    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "bound" })))
        .await
        .expect("resolution and running are not protocol errors");

    assert_eq!(result.is_error, Some(false));
    assert_eq!(text_of(&result), "bound");
    assert_eq!(structured_of(&result)["status"], json!("completed"));
}

#[tokio::test]
async fn an_unresolvable_capability_fails_during_execution() {
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
        "[server]\napi_key = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
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
        tools,
    );
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
        "[server]\napi_key = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
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
