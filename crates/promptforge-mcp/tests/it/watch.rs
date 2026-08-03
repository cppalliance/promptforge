//! That a real save reaches a real client.
//!
//! Everything the watcher decides is asserted in its own unit tests, on a paused
//! clock and by calling the reload directly. What only this test can show is the
//! two ends: that the platform actually delivers an event for a written file, and
//! that a client on a live session receives `notifications/tools/list_changed`
//! for it.
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

use promptforge_mcp::{
    Catalog, CatalogHandle, Config, OnBroken, PromptForgeServer, Retrieval, Sessions, Watcher,
};
use rmcp::service::NotificationContext;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

/// How long a real filesystem event is given before the test calls it lost.
const PATIENCE: Duration = Duration::from_secs(10);

/// A client that reports each `tools/list_changed` it is told about.
struct ListeningClient {
    /// Where the announcements go.
    changed: UnboundedSender<()>,
}

impl ClientHandler for ListeningClient {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        let _sent = self.changed.send(());
    }
}

/// A prompt whose Lua returns at once, so no gateway is needed.
fn prompt(name: &str, description: &str, value: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\nversion: 1\npromptforge: 1\n---\n\n\
         ## Main\n\n```lua\nreturn '{value}'\n```\n"
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
async fn a_saved_prompt_reaches_the_catalog_and_the_client() {
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
             [catalog]\ninclude = [\"*.md\"]\ndefault_expose = \"list\"\n",
            root.join("prompts").display()
        ),
    )
    .expect("write the configuration");

    let config = Arc::new(Config::load(&source).expect("the configuration loads"));
    let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("boot resolves");
    let catalog = Arc::new(CatalogHandle::new(catalog));
    let sessions = Arc::new(Sessions::new());
    // Idle, so this test costs no model load: what it is about is that a real
    // platform event reaches a real client, and the rebuild that rides the same
    // swap is asserted in the reload's own tests.
    let retrieval = Arc::new(Retrieval::idle());

    let _watcher = Watcher::start(
        &source,
        Arc::clone(&config),
        Arc::clone(&catalog),
        Arc::clone(&sessions),
        Arc::clone(&retrieval),
    )
    .expect("the watcher starts")
    .expect("watch defaults to on");

    let server = PromptForgeServer::new(
        config,
        Arc::clone(&catalog),
        Arc::clone(&sessions),
        retrieval,
    );
    let (server_io, client_io) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let Ok(running) = server.serve(server_io).await else {
            return;
        };
        let _quit = running.waiting().await;
    });
    let (announced, mut changes) = unbounded_channel();
    let client = ListeningClient { changed: announced }
        .serve(client_io)
        .await
        .expect("the in-process client initializes");
    // The session registers itself on `notifications/initialized`, which the
    // handshake above sent; the announcement has nowhere to go until it has.
    wait_until(|| sessions.len() == 1, "the session registers").await;

    // One real save, through the real platform watcher.
    write_prompt(root, "gamma", "Do the gamma thing", "gamma v1");

    wait_until(
        || catalog.load().find("gamma").is_some(),
        "the saved prompt joins the catalog",
    )
    .await;
    assert_eq!(
        catalog
            .load()
            .find("gamma")
            .expect("the new entry")
            .description(),
        "Do the gamma thing"
    );

    let told = tokio::time::timeout(PATIENCE, changes.recv()).await;
    assert!(
        matches!(told, Ok(Some(()))),
        "a client on a live session is told the tool list changed"
    );

    let listed = client
        .list_tools(None)
        .await
        .expect("the catalog answers a listing");
    let names: Vec<&str> = listed.tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert!(
        names.contains(&"list_prompts"),
        "the listing tools are how a listed prompt is reached: {names:?}"
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
