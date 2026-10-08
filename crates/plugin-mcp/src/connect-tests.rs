//! Tests for the startup deadline, driven with a short value.

use std::collections::HashMap;
use std::time::Duration;

use promptforge_plugin::PluginId;
use tokio::net::TcpListener;
use tokio::sync::watch;

use super::run;
use crate::entry::RemoteEntry;
use crate::server::State;

#[tokio::test]
async fn a_server_that_never_answers_fails_within_the_startup_deadline() {
    // Accepts the connection and then says nothing, so the handshake never finishes.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!(
        "http://{}/mcp",
        listener.local_addr().expect("has an address")
    );
    let silent = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let (state, watching) = watch::channel(State::Starting);
    let entry = RemoteEntry {
        url,
        headers: HashMap::new(),
    };
    let plugin = PluginId::parse("silent").expect("parses");

    let task = tokio::spawn(run(plugin, entry, Duration::from_millis(200), state));
    let mut watching = watching;
    let settled = tokio::time::timeout(
        Duration::from_secs(30),
        watching.wait_for(|now| !matches!(now, State::Starting)),
    )
    .await
    .expect("the deadline ends the wait long before this guard")
    .expect("the sender is alive until the task ends");

    match &*settled {
        State::Failed(reason) => assert!(
            reason.contains("did not finish starting"),
            "the reason names the deadline: {reason}"
        ),
        other => panic!("expected Failed, got {other:?}"),
    }
    drop(settled);
    task.await.expect("the task ends once it has published");
    silent.abort();
}

#[tokio::test]
async fn a_refused_connection_fails_with_a_handshake_reason() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!(
        "http://{}/mcp",
        listener.local_addr().expect("has an address")
    );
    drop(listener);
    let (state, mut watching) = watch::channel(State::Starting);
    let entry = RemoteEntry {
        url,
        headers: HashMap::new(),
    };
    let plugin = PluginId::parse("closed").expect("parses");

    tokio::spawn(run(plugin, entry, Duration::from_secs(30), state));
    let settled = tokio::time::timeout(
        Duration::from_secs(30),
        watching.wait_for(|now| !matches!(now, State::Starting)),
    )
    .await
    .expect("a refused connection settles quickly")
    .expect("the sender is alive");
    match &*settled {
        State::Failed(reason) => assert!(reason.contains("handshake"), "{reason}"),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failed_start_gives_a_reason_that_holds_no_part_of_the_url() {
    // A URL may hold an `${env:NAME}` value, and reqwest's error text carries the URL.
    const SECRET: &str = "ghp_secret_token_value_0042";
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let port = listener.local_addr().expect("has an address").port();
    drop(listener);
    let (state, mut watching) = watch::channel(State::Starting);
    let entry = RemoteEntry {
        url: format!("http://{SECRET}@127.0.0.1:{port}/mcp/{SECRET}?key={SECRET}"),
        headers: HashMap::new(),
    };
    let plugin = PluginId::parse("secret").expect("parses");

    tokio::spawn(run(plugin, entry, Duration::from_secs(30), state));
    let settled = tokio::time::timeout(
        Duration::from_secs(30),
        watching.wait_for(|now| !matches!(now, State::Starting)),
    )
    .await
    .expect("a refused connection settles quickly")
    .expect("the sender is alive");
    match &*settled {
        State::Failed(reason) => {
            assert!(reason.contains("handshake"), "{reason}");
            assert!(!reason.contains(SECRET), "{reason}");
            assert!(!reason.contains("127.0.0.1"), "{reason}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn a_handshake_cause_never_repeats_the_transport_errors_text() {
    use std::any::TypeId;

    use rmcp::service::ClientInitializeError;
    use rmcp::transport::DynamicTransportError;

    use super::handshake_cause;

    let secret_url = "error sending request for url (http://host/mcp?key=ghp_secret)";
    let error = ClientInitializeError::TransportError {
        error: DynamicTransportError::from_parts(
            "streamable-http",
            TypeId::of::<()>(),
            secret_url.into(),
        ),
        context: "send initialize request".into(),
    };
    let cause = handshake_cause(&error);
    assert!(cause.contains("send initialize request"), "{cause}");
    assert!(!cause.contains("ghp_secret"), "{cause}");
}
