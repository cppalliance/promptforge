//! Tests for the serve loop's drain and join bounds on stalled requests and workers.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::extract::State;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::*;
use crate::api_error::ServeError;

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

/// Serves `gateway` on an ephemeral loopback listener until the
/// returned sender fires.
async fn serve_until_stopped(
    gateway: Gateway,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), ServeError>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the listener binds");
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let serve = tokio::spawn(gateway.serve(listener, async move {
        let _ = shutdown_rx.await;
    }));
    (shutdown, serve)
}

/// Enqueues one command on `gateway` whose body reports its start and
/// then parks forever, ignoring its cancellation token. The returned
/// receiver hears the start once a served worker runs the body.
fn park_a_command(gateway: &Gateway) -> UnboundedReceiver<()> {
    let (entered, parked) = tokio::sync::mpsc::unbounded_channel();
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
    parked
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
    let mut parked = park_a_command(&gateway);
    let (shutdown, serve) = serve_until_stopped(gateway).await;
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

/// A scripted speech service over an interim and a final decoder,
/// returned with both decoders' controllers.
#[cfg(feature = "stt")]
fn scripted_speech() -> (
    gateway_stt::SpeechService,
    gateway_stt::test_fixtures::ScriptedDecoder,
    gateway_stt::test_fixtures::ScriptedDecoder,
) {
    use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory, scripted_service};

    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    let service = scripted_service(
        ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone()),
        15,
        500,
    )
    .expect("scripted speech starts");
    (service, interim, final_decoder)
}

/// A gateway with no models over `service`.
#[cfg(feature = "stt")]
fn speech_gateway(service: &gateway_stt::SpeechService) -> Gateway {
    let config = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
    )
    .expect("config parses");
    Gateway::new(&config, ProfilesContext::default())
        .expect("gateway assembles")
        .with_speech_service(service.clone())
}

/// Whether both decoders' workers drop, each within ten seconds.
#[cfg(feature = "stt")]
async fn both_workers_drop(
    interim: gateway_stt::test_fixtures::ScriptedDecoder,
    final_decoder: gateway_stt::test_fixtures::ScriptedDecoder,
) -> bool {
    tokio::task::spawn_blocking(move || {
        interim.wait_until_worker_dropped(Duration::from_secs(10))
            && final_decoder.wait_until_worker_dropped(Duration::from_secs(10))
    })
    .await
    .expect("the worker wait did not panic")
}

/// A graceful stop retires the speech service before `serve` returns:
/// admission closes and both engine workers are joined, so a native
/// decoder frees its context before the process exits.
#[cfg(feature = "stt")]
#[tokio::test]
async fn serve_retires_speech_before_it_returns() {
    let (service, interim, final_decoder) = scripted_speech();
    assert!(
        service.status().ready(),
        "the scripted runtime is published"
    );
    let (shutdown, serve) = serve_until_stopped(speech_gateway(&service)).await;

    let _ = shutdown.send(());
    tokio::time::timeout(WORKER_JOIN_TIMEOUT * 2, serve)
        .await
        .expect("serve returns within twice the join bound")
        .expect("the serve task did not panic")
        .expect("serve returns Ok after retiring speech");
    assert!(
        interim.worker_dropped(),
        "the interim worker is joined before serve returns"
    );
    assert!(
        final_decoder.worker_dropped(),
        "the final worker is joined before serve returns"
    );
    assert!(
        !service.status().ready(),
        "the retired service reports not ready"
    );
}

/// A speech retirement held open by a worker job cannot pin the exit:
/// `serve` gives it the rest of the [`WORKER_JOIN_TIMEOUT`] deadline it
/// shares with the worker join, then returns without it, and the
/// abandoned retirement still joins the workers once the job ends.
#[cfg(feature = "stt")]
#[tokio::test]
async fn serve_abandons_a_speech_retirement_that_outlasts_the_join_bound() {
    let (service, interim, final_decoder) = scripted_speech();
    // The request that owns the job ends with this statement; the job's
    // own count holds the retirement in its drain wait.
    let job = gateway_stt::test_fixtures::generation_ownership(&service)
        .expect("the published runtime admits a request")
        .own_worker_job()
        .expect("the admitted request owns a worker job");
    let (shutdown, serve) = serve_until_stopped(speech_gateway(&service)).await;

    let started = Instant::now();
    let _ = shutdown.send(());
    tokio::time::timeout(WORKER_JOIN_TIMEOUT * 2, serve)
        .await
        .expect("serve returns within twice the join bound")
        .expect("the serve task did not panic")
        .expect("serve returns Ok after abandoning the retirement");
    assert!(
        started.elapsed() >= WORKER_JOIN_TIMEOUT,
        "the retirement is given the whole join bound: {:?}",
        started.elapsed()
    );
    assert!(
        !interim.worker_dropped() && !final_decoder.worker_dropped(),
        "the held job keeps the abandoned retirement waiting"
    );

    drop(job);
    assert!(
        both_workers_drop(interim, final_decoder).await,
        "the abandoned retirement joins both workers once the job ends"
    );
    assert!(
        !service.status().ready(),
        "the retired service reports not ready"
    );
}

/// The worker join and the speech retirement share one deadline: when
/// a stuck command spends the whole [`WORKER_JOIN_TIMEOUT`] of it, a
/// retirement held open by a worker job is abandoned at once instead of
/// getting a bound of its own, so `serve` returns after one join bound,
/// not two. The abandoned retirement still joins the workers once the
/// job ends.
#[cfg(feature = "stt")]
#[tokio::test]
async fn serve_bounds_the_worker_join_and_the_speech_retirement_by_one_deadline() {
    let (service, interim, final_decoder) = scripted_speech();
    let job = gateway_stt::test_fixtures::generation_ownership(&service)
        .expect("the published runtime admits a request")
        .own_worker_job()
        .expect("the admitted request owns a worker job");
    let gateway = speech_gateway(&service);
    let mut parked = park_a_command(&gateway);
    let (shutdown, serve) = serve_until_stopped(gateway).await;
    tokio::time::timeout(Duration::from_secs(10), parked.recv())
        .await
        .expect("the worker picks up the command")
        .expect("the command body reports it started");

    let started = Instant::now();
    let _ = shutdown.send(());
    tokio::time::timeout(WORKER_JOIN_TIMEOUT * 3 / 2, serve)
        .await
        .expect("serve returns within one and a half join bounds")
        .expect("the serve task did not panic")
        .expect("serve returns Ok after abandoning the worker and the retirement");
    assert!(
        started.elapsed() >= WORKER_JOIN_TIMEOUT,
        "the worker is given the whole join bound: {:?}",
        started.elapsed()
    );
    assert!(
        !interim.worker_dropped() && !final_decoder.worker_dropped(),
        "the held job keeps the abandoned retirement waiting"
    );

    drop(job);
    assert!(
        both_workers_drop(interim, final_decoder).await,
        "the abandoned retirement joins both workers once the job ends"
    );
}
