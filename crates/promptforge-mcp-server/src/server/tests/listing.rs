//! Listing and result-reporting tests: what `list_prompts` publishes, and the
//! turn total a completed run reports.

use std::sync::Arc;

use serde_json::json;

use crate::result::NO_TURNS;

use super::{
    Catalog, CatalogHandle, Config, OnBroken, PromptForgeServer, call, echo_prompt, prepared,
    server, spawn_text_gateway, speaking_server, structured_of, write,
};

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
    assert!(
        prompts.iter().all(|entry| entry.get("version").is_none()),
        "retired author versions must not reach catalog JSON"
    );
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
        "[server]\napi_key = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n[catalog]\ninclude = [\"*.md\"]\n",
        root.display()
    ))
    .expect("the fixture configuration parses");
    let catalog = Catalog::resolve(&config, OnBroken::Retain).expect("a reload keeps going");
    let tools = prepared(&config);
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
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
async fn the_reported_turn_total_is_the_one_the_run_took() {
    // The observer is the only thing that knows: a turn happens inside the
    // executor, and the handler learns of it through the event stream.
    let gateway = spawn_text_gateway().await;
    let (_dir, server) = speaking_server(gateway.addr());
    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "speak" })))
        .await
        .expect("running a named prompt is not a protocol error");

    let structured = structured_of(&result);
    assert_eq!(structured["status"], json!("completed"));
    assert_eq!(structured["value"], json!("spoken"));
    assert_eq!(structured["turns"], json!(1), "one prose section, one turn");

    gateway.shutdown().await;
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
