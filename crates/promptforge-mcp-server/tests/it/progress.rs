//! What a client sees while a run is in flight.
//!
//! The assertions here are made from the client's side of a real session, over
//! an in-process duplex transport, because the progress contract is about the
//! notifications a caller receives and nothing short of a client can show
//! those.
//!
//! One case is asserted a step short of the client: a call carrying no
//! `progressToken`. Every request an `rmcp` client sends is given a progress
//! token by the peer before it goes out, with no way to suppress it, so an
//! untokened call cannot come from a client at all; it is made against the
//! handler's own entry point instead, which is the boundary the transport
//! calls.

#![expect(
    clippy::expect_used,
    reason = "test setup panics on failure, which is the desired behavior"
)]

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use promptforge_mcp_server::{
    Catalog, CatalogHandle, Config, OnBroken, PreparedTools, PromptForgeServer, Retrieval,
};
use rmcp::model::{CallToolRequestParams, CallToolResponse, ProgressNotificationParam};
use rmcp::service::NotificationContext;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use tempfile::TempDir;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// A client that forwards every progress notification to the test.
struct RecordingClient {
    /// Where the notifications go.
    progress: UnboundedSender<ProgressNotificationParam>,
}

impl ClientHandler for RecordingClient {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let _sent = self.progress.send(params);
    }
}

/// A prompt of three sections, the first two falling through and the last
/// returning, so the whole run happens offline.
const TRIO: &str = "---\nname: trio\ndescription: Three sections\npromptforge: 1\n---\n\n\
# Trio\n\n\
## First\n\n```lua\nvar.step = 1\n```\n\n\
## Second\n\n```lua\nvar.step = 2\n```\n\n\
## Third\n\n```lua\nreturn 'trio done'\n```\n";

/// A server over a catalog holding [`TRIO`] alone.
fn trio_server() -> (TempDir, PromptForgeServer) {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    fs::write(dir.path().join("trio.md"), TRIO).expect("write the fixture prompt");
    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n\
         [catalog]\ninclude = [\"*.md\"]\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = Arc::new(PreparedTools::new(&config.gateway).expect("prepare fixture live tools"));
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::new(Retrieval::idle()),
        tools,
    );
    (dir, server)
}

#[tokio::test]
async fn a_run_frames_its_start_and_then_each_section() {
    let (_dir, server) = trio_server();

    let (server_io, client_io) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let running = server.serve(server_io).await.unwrap();
        let _ = running.waiting().await;
    });

    let (sender, mut progress) = unbounded_channel();
    let client = RecordingClient { progress: sender }
        .serve(client_io)
        .await
        .expect("the in-process client initializes");

    let response = client
        .call_tool_once(
            CallToolRequestParams::new("run_prompt").with_arguments(
                serde_json::json!({ "prompt": "trio" })
                    .as_object()
                    .expect("the arguments are an object")
                    .clone(),
            ),
        )
        .await
        .expect("the call reaches the prompt");
    let CallToolResponse::Complete(result) = response else {
        panic!("this server answers a call with its result")
    };
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result.content[0].as_text().expect("a text block").text,
        "trio done"
    );

    let frames = collect(&mut progress, 4).await;
    let token = frames[0].progress_token.clone();
    for frame in &frames {
        assert_eq!(frame.progress_token, token, "one run, one token");
        assert_eq!(
            frame.total, None,
            "how many sections a run will visit is not known when it starts"
        );
    }
    let seen: Vec<(f64, Option<&str>)> = frames
        .iter()
        .map(|frame| (frame.progress, frame.message.as_deref()))
        .collect();
    assert_eq!(
        seen,
        vec![
            (0.0, Some("Trio")),
            (1.0, Some("First")),
            (2.0, Some("Second")),
            (3.0, Some("Third")),
        ]
    );

    assert!(
        tokio::time::timeout(Duration::from_millis(200), progress.recv())
            .await
            .is_err(),
        "the run frames its start and its sections, and nothing else"
    );
}

#[tokio::test]
async fn a_call_with_no_progress_token_answers_the_same() {
    // No token means no peer to report to, so there is no channel and no pump
    // task - and the caller must not be able to tell from the answer.
    let (_dir, server) = trio_server();
    let result = server
        .dispatch(
            CallToolRequestParams::new("run_prompt").with_arguments(
                serde_json::json!({ "prompt": "trio" })
                    .as_object()
                    .expect("the arguments are an object")
                    .clone(),
            ),
        )
        .await
        .expect("the call reaches the prompt");

    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result.content[0].as_text().expect("a text block").text,
        "trio done"
    );
    let structured = result
        .structured_content
        .expect("every run result carries structured content");
    assert_eq!(structured["status"], serde_json::json!("completed"));
    assert_eq!(structured["value"], serde_json::json!("trio done"));
    assert_eq!(structured["turns"], serde_json::json!(0));
}

/// The next `count` notifications, or a panic once waiting stops being
/// plausible.
async fn collect(
    progress: &mut UnboundedReceiver<ProgressNotificationParam>,
    count: usize,
) -> Vec<ProgressNotificationParam> {
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        let frame = tokio::time::timeout(Duration::from_secs(5), progress.recv())
            .await
            .expect("a progress notification should arrive")
            .expect("the session outlives the wait");
        frames.push(frame);
    }
    frames
}
