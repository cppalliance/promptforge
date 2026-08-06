//! That a real save reaches a real client, with nothing announced to it.
//!
//! Everything the watcher decides is asserted in its own unit tests, on a paused
//! clock and by calling the reload directly. What only this test can show is the
//! two ends: that the platform actually delivers an event for a written file,
//! and that the client which was already connected when the file was written
//! runs it through `run_prompt` on the same session, with no notification and no
//! reconnect.
//!
//! This is the one test in the suite that waits on the real clock, because a
//! filesystem event arrives when the operating system says so. The debounce
//! window is set to 50ms and the wait is a bounded poll, so the cost is
//! milliseconds in the normal case and a named failure rather than a hang in the
//! abnormal one.

#![expect(
    clippy::expect_used,
    reason = "test setup panics on failure, which is the desired behavior"
)]

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use promptforge_mcp_server::{
    Catalog, CatalogHandle, Config, OnBroken, PromptForgeServer, Retrieval, Watcher,
};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResponse};

/// How long a real filesystem event is given before the test calls it lost.
const PATIENCE: Duration = Duration::from_secs(10);

/// A prompt whose Lua returns at once, so no gateway is needed.
fn prompt(name: &str, description: &str, value: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\nversion: 1\npromptforge: 1\n---\n\n\
         # Test prompt\n\n## Main\n\n```lua\nreturn '{value}'\n```\n"
    )
}

/// Writes a prompt file named after the prompt.
fn write_prompt(root: &Path, name: &str, description: &str, value: &str) {
    fs::write(
        root.join("prompts").join(format!("{name}.md")),
        prompt(name, description, value),
    )
    .expect("write the fixture prompt");
}

#[tokio::test]
async fn a_saved_prompt_is_callable_on_the_session_that_was_already_open() {
    let dir = tempfile::tempdir().expect("create a temporary root");
    let root = dir.path();
    fs::create_dir_all(root.join("prompts")).expect("create the prompts directory");
    write_prompt(root, "alpha", "Do the alpha thing", "alpha v1");

    let source = root.join("prompts.toml");
    fs::write(
        &source,
        format!(
            "[server]\ntoken = \"shared\"\nwatch_debounce = \"50ms\"\n\n\
             [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
             [paths]\nprompts = '{}'\n\n\
             [catalog]\ninclude = [\"*.md\"]\n",
            root.join("prompts").display()
        ),
    )
    .expect("write the configuration");

    let config = Arc::new(Config::load(&source).expect("the configuration loads"));
    let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("boot resolves");
    let catalog = Arc::new(CatalogHandle::new(catalog));
    // Idle, so this test costs no model load: what it is about is that a real
    // platform event reaches a real client, and the rebuild that rides the same
    // swap is asserted in the reload's own tests.
    let retrieval = Arc::new(Retrieval::idle());

    let _watcher = Watcher::start(
        &source,
        Arc::clone(&config),
        Arc::clone(&catalog),
        Arc::clone(&retrieval),
    )
    .expect("the watcher starts")
    .expect("watch defaults to on");

    let server = PromptForgeServer::new(config, Arc::clone(&catalog), retrieval);
    let (server_io, client_io) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let Ok(running) = server.serve(server_io).await else {
            return;
        };
        let _quit = running.waiting().await;
    });
    let client = ().serve(client_io).await.expect("the in-process client initializes");

    assert_eq!(
        client
            .peer_info()
            .expect("the handshake reported the server's capabilities")
            .capabilities
            .tools
            .as_ref()
            .and_then(|tools| tools.list_changed),
        None,
        "the tool list never moves, so nothing claims it can announce a change"
    );

    // One real save, through the real platform watcher, on a session that is
    // already open.
    write_prompt(root, "gamma", "Do the gamma thing", "gamma v1");

    wait_until(
        || catalog.load().find("gamma").is_some(),
        "the saved prompt joins the catalog",
    )
    .await;

    let listed = client
        .list_tools(None)
        .await
        .expect("the catalog answers a listing");
    let names: Vec<&str> = listed.tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert!(
        names.contains(&"run_prompt"),
        "the runner is how a prompt is reached: {names:?}"
    );
    assert!(
        !names.contains(&"gamma"),
        "a prompt saved mid-session is still not a tool of its own: {names:?}"
    );

    let response = client
        .call_tool_once(
            CallToolRequestParams::new("run_prompt").with_arguments(
                serde_json::json!({ "prompt": "gamma" })
                    .as_object()
                    .expect("the arguments are an object")
                    .clone(),
            ),
        )
        .await
        .expect("the call reaches the prompt written after the client connected");
    let CallToolResponse::Complete(result) = response else {
        panic!("this server answers a call with its result")
    };
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result.content[0].as_text().expect("a text block").text,
        "gamma v1"
    );
}

/// Polls `ready` until it holds, or panics naming what never happened.
async fn wait_until<F: Fn() -> bool>(ready: F, what: &str) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("waited {PATIENCE:?} for {what}");
}
