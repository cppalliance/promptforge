//! Tests for the serve loop's drain and join bounds on stalled requests and workers.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::extract::State;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::*;

/// A backend whose `/chat/completions` reports each arrival and then
/// never answers, so a proxied chat request stays in flight at the
/// gateway for as long as the test needs it there.
async fn silent_backend() -> (SocketAddr, UnboundedReceiver<()>) {
    async fn completions(State(arrivals): State<UnboundedSender<()>>) -> axum::http::StatusCode {
        let _ = arrivals.send(());
        std::future::pending::<()>().await;
        axum::http::StatusCode::OK
    }
    let (arrivals, arrived) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new()
        .route("/chat/completions", axum::routing::post(completions))
        .with_state(arrivals);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the backend binds");
    let addr = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let _ignored = axum::serve(listener, app).await;
    });
    (addr, arrived)
}

/// A proxied request whose upstream never answers cannot pin the exit:
/// the drain gives it the whole of [`GRACEFUL_DRAIN_TIMEOUT`], then
/// `serve` returns without it.
#[tokio::test]
async fn serve_abandons_a_stalled_request_after_the_drain_bound() {
    let (backend, mut arrivals) = silent_backend().await;
    let config = Config::from_toml_str(&format!(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[endpoint]]\nid = \"silent\"\nprotocol = \"openai\"\n\
         base_url = \"http://{backend}\"\napi_key = \"\"\n\
         [[model]]\nname = \"test-model\"\ndescription = \"never answers\"\n\
         context = 8192\nthinking = \"never\"\nupstream = \"backend-model\"\n\
         endpoints = [\"silent\"]\n"
    ))
    .expect("config parses");
    let gateway =
        Gateway::from_config(&config, ProfilesContext::default()).expect("gateway assembles");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the listener binds");
    let addr = listener.local_addr().expect("the bound address");
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let serve = tokio::spawn(gateway.serve(listener, async move {
        let _ = shutdown_rx.await;
    }));

    let stalled = tokio::spawn(
        reqwest::Client::new()
            .post(format!("http://{addr}/v1/chat/completions"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "ping"}]
            }))
            .send(),
    );
    // The arrival is the rendezvous: the gateway's handler is now
    // awaiting an upstream that never answers.
    tokio::time::timeout(Duration::from_secs(10), arrivals.recv())
        .await
        .expect("the proxied request reaches the backend")
        .expect("the backend reports the arrival");

    let started = Instant::now();
    let _ = shutdown.send(());
    tokio::time::timeout(GRACEFUL_DRAIN_TIMEOUT * 2, serve)
        .await
        .expect("serve returns within twice the drain bound")
        .expect("the serve task did not panic")
        .expect("serve returns Ok after abandoning the drain");
    assert!(
        started.elapsed() >= GRACEFUL_DRAIN_TIMEOUT,
        "the in-flight request is given the whole drain bound: {:?}",
        started.elapsed()
    );
    stalled.abort();
}

/// A command body that ignores its cancellation token cannot pin the
/// exit: `serve` gives the worker join exactly [`WORKER_JOIN_TIMEOUT`],
/// then returns without it.
#[tokio::test]
async fn serve_abandons_a_worker_that_ignores_cancellation_after_the_join_bound() {
    let config = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
    )
    .expect("config parses");
    let gateway =
        Gateway::from_config(&config, ProfilesContext::default()).expect("gateway assembles");
    let (entered, mut parked) = tokio::sync::mpsc::unbounded_channel();
    gateway
        .state
        .commands
        .override_executor(std::sync::Arc::new(move |_state, _command, _tree| {
            let entered = entered.clone();
            Box::pin(async move {
                let _ = entered.send(());
                // The token never fires this body: it parks forever.
                std::future::pending().await
            }) as futures_util::future::BoxFuture<'static, crate::commands::Outcome>
        }));
    let _command = gateway
        .state
        .commands
        .enqueue(crate::commands::Command::load_profile(
            ProfileName::parse("main").expect("profile name"),
            tokio_util::sync::CancellationToken::new(),
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the listener binds");
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let serve = tokio::spawn(gateway.serve(listener, async move {
        let _ = shutdown_rx.await;
    }));
    // The rendezvous: the worker is parked inside the command body.
    tokio::time::timeout(Duration::from_secs(10), parked.recv())
        .await
        .expect("the worker picks up the command")
        .expect("the command body reports it started");

    let started = Instant::now();
    let _ = shutdown.send(());
    tokio::time::timeout(WORKER_JOIN_TIMEOUT * 2, serve)
        .await
        .expect("serve returns within twice the join bound")
        .expect("the serve task did not panic")
        .expect("serve returns Ok after abandoning the worker");
    assert!(
        started.elapsed() >= WORKER_JOIN_TIMEOUT,
        "the worker is given the whole join bound: {:?}",
        started.elapsed()
    );
}
