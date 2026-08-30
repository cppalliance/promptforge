//! Handler-test fixtures: the servers, prompts, and helpers the focused child
//! modules share.
//!
//! Routing, resolution, and argument shapes are in [`dispatch`]; listing and
//! result reporting are in [`listing`]; admission, the reply deadline, and
//! collecting a run by id are in [`runs`]. Each shares these fixtures.
//!
//! Almost none of these needs a gateway: every fixture prompt's Lua prologue
//! returns a value, which finishes the run before any model call is made. The
//! exception is the turn count, which is a statement about model round trips
//! and so needs a backend to take one against.

mod dispatch;
mod io;
mod listing;
mod runs;

use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use axum::Json;
use axum::Router;
use axum::routing::post;
use rmcp::model::{CallToolRequestParams, CallToolResult, JsonObject};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::{PreparedTools, PromptForgeServer};
use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::config::Config;
use std::num::NonZeroU32;

use promptforge_core::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};

fn fixture_model_catalog() -> ModelCatalog {
    ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway("claude-sonnet-4-6").expect("the test model alias is valid"),
        "A model suited for careful analysis, coding, and general assistance",
        NonZeroU32::new(200_000).expect("200000 is non-zero"),
        ThinkingMode::Never,
    )])
    .expect("the test catalog has a single unique model")
}

fn prepared(config: &Config) -> Arc<PreparedTools> {
    static SEED: OnceLock<PreparedTools> = OnceLock::new();
    let seed = SEED.get_or_init(|| {
        PreparedTools::new(
            &config.gateway,
            &config.tools,
            fixture_model_catalog(),
            crate::fixture::model(),
            None,
        )
        .expect("prepare fixture tool model")
    });
    Arc::new(
        seed.rebuild(&config.gateway, &config.tools)
            .expect("index fixture live tools"),
    )
}

/// A prompt that returns its input without calling a model.
fn echo_prompt(name: &str, description: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\npromptforge: 1\n---\n\n\
         # Test prompt\n\n## Main\n\n```lua\nreturn args\n```\n"
    )
}

/// A prompt that declares an input file and returns its content from the store.
fn input_prompt(name: &str, store_path: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Reads declared input\npromptforge: 1\n\
         input:\n  path: {store_path}\n  description: The input file\n---\n\n\
         # Test prompt\n\n## Main\n\n```lua\nreturn store.read(\"{store_path}\")\n```\n"
    )
}

/// A prompt that declares an output file, writes content to the store, and
/// returns a value.
fn output_prompt(name: &str, store_path: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Writes declared output\npromptforge: 1\n\
         output:\n  path: {store_path}\n  description: The output file\n---\n\n\
         # Test prompt\n\n## Main\n\n\
         ```lua\nstore.write(\"{store_path}\", \"produced content\")\nreturn \"done\"\n```\n"
    )
}

/// A prompt whose valid Lua returns an unsupported value at execution.
fn failing_lua_prompt(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Fails on entry\npromptforge: 1\n---\n\n\
         # Test prompt\n\n## Main\n\n```lua\nreturn {{}}\n```\n"
    )
}

fn capability_prompt(name: &str, capability: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Live capability fixture\npromptforge: 1\n---\n\n\
         # Capability prompt\n\n```lua\ntools.bind(\"fetch\", \"{capability}\")\n```\n\n\
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
        "[server]\napi_key = \"t\"\n{server_lines}\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
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

/// A loopback gateway a test owns for its lifetime.
///
/// It holds the serving task and a graceful-shutdown trigger, so a test can stop
/// it and await a clean exit rather than leaving a detached task to be torn down
/// with the runtime and its serving `Result` discarded. Dropping it without an
/// explicit [`shutdown`](Self::shutdown) still triggers the shutdown and aborts
/// the task, so a gateway never outlives the test that built it - including one
/// whose handler is deliberately wedged on a request that never returns.
struct Gateway {
    addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl Gateway {
    /// Serves `router` on an ephemeral loopback port, returning once it is bound
    /// so a caller can point a client at [`addr`](Self::addr) at once.
    async fn serve(router: Router) -> Gateway {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("read the bound address");
        let (shutdown, stop) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stop.await;
                })
                .await
        });
        Gateway {
            addr,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    /// The address a client reaches this gateway at.
    fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Triggers a graceful shutdown and awaits the serving task, failing the
    /// test if it did not join or served with an error.
    ///
    /// Only for a gateway whose in-flight requests can finish; a handler left
    /// deliberately pending is stopped by [`Drop`] instead, since a graceful
    /// shutdown would wait on a request that never returns.
    async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.await
                .expect("the gateway task joins")
                .expect("the gateway served without error");
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        // A test that did not call `shutdown` still must not leak the task: the
        // trigger ends a graceful serve, and the abort then covers a handler
        // wedged on a request that never returns.
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// A gateway that answers every request with the same assistant message, so a
/// prose section takes exactly one model round trip.
async fn spawn_text_gateway() -> Gateway {
    async fn completions(Json(_body): Json<Value>) -> Json<Value> {
        Json(json!({
            "choices": [{ "message": { "role": "assistant", "content": "spoken" } }]
        }))
    }

    let router = Router::new().route("/v1/chat/completions", post(completions));
    Gateway::serve(router).await
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
        "---\nname: speak\ndescription: Say something\npromptforge: 1\n---\n\n\
         # Test prompt\n\n```lua\n\
         models.default(\"writer\", \"A model suited for careful analysis, coding, and general assistance\")\n\
         ```\n\n## Only\n\nSay something.\n",
    );
    let config = Config::from_toml_str(&format!(
        "[server]\napi_key = \"t\"\n{server_lines}\n\n\
         [gateway]\nurl = \"http://{gateway}/v1\"\napi_key = \"gw\"\n\n\
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
        tools,
    );
    (dir, server)
}
