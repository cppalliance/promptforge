//! Workspace revokes against a live agent session: a root revoked while
//! an accepted turn is in flight is gone from the host snapshot the
//! session's deferred catalog relaunch reads.

use tokio::sync::mpsc;
use workshop_workspace::Workspace;

use super::*;

/// The roots agent: every run opens with a model round naming the `ui()`
/// snapshot's workspace root, so each launch and relaunch reports the
/// roots the harness held when it started, then echoes inputs.
const ROOTS_MD: &str = r"---
name: roots
description: The roots test agent on the unified runtime.
promptforge: 0
---

# Roots

## Conversation

```lua
local history = messages.new()
history:user('launch@' .. tostring(ui().workspace_root))
models.loop(models.get('test-model'), history)
while true do
    local text, available = user_input()
    if not available then
        return
    end
    history:user(text)
    models.loop(models.get('test-model'), history)
end
```
";

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
async fn a_revoke_during_a_running_session_pushes_roots_without_the_revoked_folder() {
    let (launches_tx, mut launches) = mpsc::unbounded_channel();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let gateway = spawn_gateway(with_typed_catalog(Router::new().route(
        "/v1/chat/completions",
        post({
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            move |body: String| {
                let (started, release, launches_tx) = (
                    Arc::clone(&started),
                    Arc::clone(&release),
                    launches_tx.clone(),
                );
                async move {
                    let content = last_content(&body);
                    if content.starts_with("launch@") {
                        let _ = launches_tx.send(content);
                    } else if content == "go" {
                        started.notify_one();
                        release.notified().await;
                    }
                    echo_completions(body).await
                }
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

    let mut socket = JsonSocket::connect(&format!("{base}/agents/ws")).await;
    assert_eq!(socket.recv_json().await["type"], "agents");
    let session = launch_agent(&mut socket, "roots").await;
    let at_launch = tokio::time::timeout(Duration::from_secs(10), launches.recv())
        .await
        .expect("the launch opens its model round")
        .expect("the gateway is alive");
    assert_eq!(
        at_launch,
        format!("launch@{}", granted.display()),
        "the launch reads the granted folder"
    );
    let token = next_wait_token(&mut socket).await;

    // A catalog replacement landing on an accepted input defers the
    // relaunch until the turn settles; the gateway holds that turn open
    // so the revoke lands between the replacement and the relaunch.
    let catalog_state = state.clone();
    state
        .agents()
        .deliver_input_after_acceptance_for_test(
            &session,
            workshop_server::InputResponse {
                token,
                text: "go".to_owned(),
            },
            move || {
                catalog_state.catalog().publish(vec![
                    json!({ "id": "test-model", "object": "model" }),
                    json!({ "id": "model-b", "object": "model" }),
                ]);
            },
        )
        .expect("the session remains registered")
        .expect("the accepted input resumes its run");
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("the accepted turn reaches the gateway");

    workspace
        .revoke_and_persist(&granted)
        .await
        .expect("the folder revokes");
    release.notify_one();
    let at_relaunch = tokio::time::timeout(Duration::from_secs(10), launches.recv())
        .await
        .expect("the settled turn relaunches under the replacement catalog")
        .expect("the gateway is alive");
    assert_eq!(
        at_relaunch, "launch@nil",
        "the relaunch reads roots without the revoked folder"
    );
    socket.close().await;
}
