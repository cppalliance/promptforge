//! Benchmarks for the active executor paths: the Lua-shim `models.loop`
//! (the loop runs inside `__impl_coro.lua`, yielding one `chat` request
//! per round to the scheduler) over one scripted terminal turn, which is
//! the round-overhead gate the plan's checkpoints compare against, and the
//! `compactors.fail` invocation on a precheck overflow, which runs zero
//! rounds and measures the overflow failure path.
//!
//! Run with `cargo bench -p promptforge-api-runtime`.

// The criterion_group! macro expansion generates an undocumented public
// entry point; bench targets have no docs contract.
#![expect(
    missing_docs,
    reason = "the criterion_group! macro expansion generates an undocumented public entry point; bench targets have no docs contract"
)]
#![expect(
    clippy::expect_used,
    reason = "bench setup panics on construction failure, which is the desired behavior"
)]

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use criterion::{Criterion, criterion_group, criterion_main};
use promptforge_api_runtime::client::{GatewayClient, GatewayEndpoint, SecretString};
use promptforge_api_runtime::{Environment, Prompt, RunContext, RunHost, RunResult};
use promptforge_api_types::models::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use promptforge_api_types::observe::NullObserver;

const EXECUTION: &str = "bench";

/// A minimal scripted gateway: every completion request gets the same
/// terminal-text SSE reply, so a `models.loop` bench measures exactly one
/// request per iteration.
struct BenchGateway {
    addr: SocketAddr,
    calls: Arc<AtomicUsize>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: tokio::task::JoinHandle<()>,
}

impl BenchGateway {
    /// Binds a loopback port and serves the fixed terminal-text reply.
    fn start(runtime: &tokio::runtime::Runtime) -> BenchGateway {
        async fn completions(State(calls): State<Arc<AtomicUsize>>) -> axum::response::Response {
            calls.fetch_add(1, Ordering::SeqCst);
            let body = "data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"bench reply\"}}]}\n\n\
                        data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
                        data: [DONE]\n\n";
            (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                body,
            )
                .into_response()
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route("/v1/chat/completions", post(completions))
            .with_state(Arc::clone(&calls));
        let (listener, addr) = runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("the bench gateway must bind a local port");
            let addr = listener
                .local_addr()
                .expect("the bench gateway must report its local address");
            (listener, addr)
        });
        let (shutdown, rx) = tokio::sync::oneshot::channel::<()>();
        let server = runtime.spawn(async move {
            // The serve outcome is swallowed so runtime teardown can never
            // trigger a detached-task panic.
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        BenchGateway {
            addr,
            calls,
            shutdown: Some(shutdown),
            server,
        }
    }

    /// A client pointed at this gateway.
    fn client(&self) -> GatewayClient {
        GatewayClient::new(
            GatewayEndpoint::new(&format!("http://{}/v1", self.addr))
                .expect("the bench endpoint is valid"),
            SecretString::new("bench").expect("the bench key is non-empty"),
        )
    }
}

impl Drop for BenchGateway {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.server.abort();
    }
}

/// The model catalog the bench prompts resolve against; `context` sizes the
/// one model's window.
fn bench_catalog(context: u32) -> ModelCatalog {
    ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway("bench-model").expect("the bench model id is valid"),
        "A general model for benches",
        NonZeroU32::new(context).expect("the bench context is non-zero"),
        ThinkingMode::Switchable,
    )])
    .expect("the bench catalog has a single unique model")
}

/// One section driving `models.loop` over a builder-made list.
const LOOP_PROMPT: &str = "---\nname: bench_loop\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
    # Bench\n\n\
    ```lua\n\
    models.default('writer')\n\
    ```\n\n\
    ## Only\n\n\
    ```lua\n\
    local msgs = messages.new()\n\
    msgs:user('hi')\n\
    models.loop(msgs)\n\
    return msgs[#msgs].content\n\
    ```\n";

/// Parses the loop prompt once for the whole benchmark.
fn parse_loop_prompt() -> Prompt {
    Prompt::parse(LOOP_PROMPT, EXECUTION, &NullObserver::default())
        .expect("the bench prompt parses")
}

/// The environment every bench run shares: no registry and no tools, so
/// the loop is one terminal turn.
fn bench_env() -> Environment {
    Environment::new()
}

/// One `models.loop` turn end to end: parse is excluded, so the measurement
/// covers VM setup, projection, the request, streaming accumulation, and the
/// terminal-record append.
fn models_loop(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("the bench runtime builds");
    let gateway = BenchGateway::start(&runtime);
    let prompt = parse_loop_prompt();
    let env = bench_env();
    c.bench_function("models_loop", |b| {
        b.iter(|| {
            let result = runtime.block_on(
                env.run(
                    &prompt,
                    "",
                    RunContext::new(
                        EXECUTION,
                        1,
                        promptforge_api_types::timestamp::Timestamp::UNIX_EPOCH,
                    )
                    .model(bench_catalog(131_072).models()[0].clone()),
                    RunHost::new().client(gateway.client()),
                ),
            );
            assert!(
                matches!(result, RunResult::Ok(_)),
                "the loop bench run succeeds: {result:?}"
            );
        });
    });
    assert!(
        gateway.calls.load(Ordering::SeqCst) > 0,
        "every loop iteration is exactly one request"
    );
}

/// The `compactors.fail` path: a one-token context window overflows the
/// request precheck before any dispatch, so the default compactor raises
/// typed context exhaustion without a single request.
fn compactors_fail(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("the bench runtime builds");
    let gateway = BenchGateway::start(&runtime);
    let prompt = parse_loop_prompt();
    let env = bench_env();
    c.bench_function("compactors_fail", |b| {
        b.iter(|| {
            let result = runtime.block_on(
                env.run(
                    &prompt,
                    "",
                    RunContext::new(
                        EXECUTION,
                        1,
                        promptforge_api_types::timestamp::Timestamp::UNIX_EPOCH,
                    )
                    .model(bench_catalog(1).models()[0].clone()),
                    RunHost::new().client(gateway.client()),
                ),
            );
            let RunResult::Failure(error) = result else {
                panic!("a one-token window must exhaust at the precheck");
            };
            assert_eq!(
                error.kind(),
                promptforge_api_runtime::RunErrorKind::ContextExhausted,
                "the default compactor is compactors.fail: {error:?}"
            );
        });
    });
    assert_eq!(
        gateway.calls.load(Ordering::SeqCst),
        0,
        "the precheck overflow never reaches the wire"
    );
}

criterion_group!(benches, models_loop, compactors_fail);
criterion_main!(benches);
