//! Benchmarks for the active executor paths: the Rust-backed `models.loop`
//! over one scripted terminal turn, and the `compactors.fail` invocation on
//! a precheck overflow.
//!
//! Run with `cargo bench -p promptforge-core`.

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
use promptforge_core::client::{GatewayClient, GatewayEndpoint, SecretString};
use promptforge_core::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use promptforge_core::observe::NullObserver;

use promptforge_core::tools::ToolCatalog;
use promptforge_core::{Prompt, ResolutionContext, RunConfig, run};
use promptforge_tool_picker::{Catalog, Config, ToolPicker};

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
const LOOP_PROMPT: &str = "---\nname: bench_loop\ndescription: d\npromptforge: 0\n---\n\n\
    # Bench\n\n\
    ```lua\n\
    models.default('writer', 'A general model for benches')\n\
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

/// The resolution context every bench run shares: an empty tool picker and
/// no tools, so the loop is one terminal turn.
fn resolution<'a>(
    picker: &'a ToolPicker,
    models: &'a ModelCatalog,
    tools: &'a ToolCatalog,
) -> ResolutionContext<'a> {
    ResolutionContext::new(picker, models, tools)
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
    let picker = ToolPicker::build(Catalog::new(Vec::new()), Config::default())
        .expect("the empty bench picker builds");
    let models = bench_catalog(131_072);
    let tools = ToolCatalog::new(&[]).expect("the empty bench tool catalog builds");
    c.bench_function("models_loop", |b| {
        b.iter(|| {
            runtime
                .block_on(run(
                    &prompt,
                    "",
                    resolution(&picker, &models, &tools),
                    &promptforge_vfs::empty(),
                    RunConfig::new(EXECUTION)
                        .observer(Arc::new(NullObserver::default()))
                        .client(gateway.client()),
                ))
                .expect("the loop bench run succeeds");
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
    let picker = ToolPicker::build(Catalog::new(Vec::new()), Config::default())
        .expect("the empty bench picker builds");
    let models = bench_catalog(1);
    let tools = ToolCatalog::new(&[]).expect("the empty bench tool catalog builds");
    c.bench_function("compactors_fail", |b| {
        b.iter(|| {
            let error = runtime
                .block_on(run(
                    &prompt,
                    "",
                    resolution(&picker, &models, &tools),
                    &promptforge_vfs::empty(),
                    RunConfig::new(EXECUTION)
                        .observer(Arc::new(NullObserver::default()))
                        .client(gateway.client()),
                ))
                .expect_err("a one-token window must exhaust at the precheck");
            assert_eq!(
                error.kind(),
                promptforge_core::RunErrorKind::ContextExhausted,
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
