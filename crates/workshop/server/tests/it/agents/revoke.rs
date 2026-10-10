//! Workspace revokes against agent conversations: a root revoked while one
//! conversation runs is gone from the Host snapshot the next
//! conversation's run reads, and the running one keeps its launch
//! snapshot.

use tokio::sync::mpsc;
use workshop_workspace::Workspace;

use super::*;

/// The roots agent: its run opens with a model round naming the `ui()`
/// snapshot's workspace root, then echoes each input with the root its
/// `ui()` reports at that turn.
const ROOTS_MD: &str = r"---
name: roots
description: The roots test agent on the unified runtime.
promptforge: 0
plugins:
  - user-input
---

# Roots

## Conversation

```lua
local history = messages.new()
history:user('launch@' .. tostring(ui().workspace_root))
models.get('test-model'):loop(history)
while true do
    local text = input.ask()
    history:user(text .. '@' .. tostring(ui().workspace_root))
    models.get('test-model'):loop(history)
end
```
";

/// The last message of the next model round the gateway received.
async fn next_round(rounds: &mut mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(Duration::from_secs(10), rounds.recv())
        .await
        .expect("the model round arrives in time")
        .expect("the gateway is alive")
}

/// The content of a completion request's last message.
fn last_content(body: &str) -> String {
    let body: serde_json::Value = serde_json::from_str(body).expect("the request is JSON");
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the request includes a message")
        .to_owned()
}

#[tokio::test]
async fn a_revoke_reaches_the_next_conversation_and_a_running_one_keeps_its_launch_snapshot() {
    let (rounds_tx, mut rounds) = mpsc::unbounded_channel();
    let gateway = spawn_gateway(with_typed_catalog(Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let rounds_tx = rounds_tx.clone();
            async move {
                let _ = rounds_tx.send(last_content(&body));
                echo_completions(body).await
            }
        }),
    )))
    .await;
    let (base, dir, state) = spawn_agent_server_for_gateway(gateway).await;
    std::fs::write(dir.path().join("agents").join("roots.md"), ROOTS_MD)
        .expect("the roots agent writes");
    let workspace = state
        .registry()
        .state::<Workspace>()
        .expect("the workspace is registered");
    let folder = tempfile::TempDir::new().expect("tempdir");
    let granted = workspace
        .grant_and_persist(folder.path())
        .await
        .expect("the folder grants");

    let mut running = JsonSocket::connect(&format!("{base}/agents/ws")).await;
    assert_eq!(running.recv_json().await["type"], "agents");
    let _running = launch(&mut running, "roots").await;
    assert_eq!(
        next_round(&mut rounds).await,
        format!("launch@{}", granted.display()),
        "the launch reads the granted folder"
    );
    let token = next_wait_token(&mut running).await;

    // The revoke lands while the first conversation waits on its operator.
    workspace
        .revoke_and_persist(&granted)
        .await
        .expect("the folder revokes");

    let mut next = JsonSocket::connect(&format!("{base}/agents/ws")).await;
    assert_eq!(next.recv_json().await["type"], "agents");
    let _next = launch(&mut next, "roots").await;
    assert_eq!(
        next_round(&mut rounds).await,
        "launch@nil",
        "the next conversation reads roots without the revoked folder"
    );

    answer(&mut running, &token, "turn").await;
    assert_eq!(
        next_round(&mut rounds).await,
        format!("turn@{}", granted.display()),
        "the running conversation keeps the snapshot it launched with"
    );
    running.close().await;
    next.close().await;
}
