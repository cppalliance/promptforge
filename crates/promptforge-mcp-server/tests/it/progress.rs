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
    Catalog, CatalogHandle, Config, OnBroken, PreparedTools, PromptForgeServer,
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
        // Every frame is forwarded while the test still holds the receiver: the
        // receiver is dropped only at teardown, after the session has ended and
        // no further notifications can arrive. A failed send would mean a frame
        // escaped that window, so the result is asserted rather than discarded.
        self.progress
            .send(params)
            .expect("the test is still receiving progress frames");
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
///
/// The prepared tools are loaded through the public boot seam pointed at the
/// fixture's unreachable gateway: the model-catalog fetch fails and degrades to
/// an empty catalog, which is all a Lua-only run needs, and the live tools are
/// built without a network round trip.
async fn trio_server() -> (TempDir, PromptForgeServer) {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    fs::write(dir.path().join("trio.md"), TRIO).expect("write the fixture prompt");
    let config = Config::from_toml_str(&format!(
        "[server]\napi_key = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n\
         [catalog]\ninclude = [\"*.md\"]\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = Arc::new(
        PreparedTools::load(&config)
            .await
            .expect("prepare fixture live tools"),
    );
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
    );
    (dir, server)
}

#[tokio::test]
async fn a_run_frames_its_start_and_then_each_section() {
    let (_dir, server) = trio_server().await;

    let (server_io, client_io) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        let running = server
            .serve(server_io)
            .await
            .expect("the server starts its session");
        running.waiting().await.expect("the server session ends")
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

    // Shut the session down and join the server before draining, so every frame
    // the run would ever send has already been sent. The client owns the only
    // sender, so once it is gone the channel is closed and `recv` resolves the
    // moment it is empty: the assertion rests on a closed channel, not on a
    // wall-clock race.
    client
        .cancel()
        .await
        .expect("the client disconnects cleanly");
    server_task.await.expect("the server task joins");
    assert!(
        progress.recv().await.is_none(),
        "the run frames its start and its sections, and nothing else"
    );
}

// The untokened-call path - a `tools/call` that carries no `progressToken`,
// which no `rmcp` client can produce - is asserted in-crate against the
// handler's own entry point in `server::tests`, where that entry point lives.

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
