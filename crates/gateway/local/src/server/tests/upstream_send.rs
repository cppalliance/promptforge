//! Local upstream send tests: respawn on a dead child, routing by kind, and cooldown.

use super::*;

#[test]
fn local_upstream_send_respawns_dead_child_once() {
    use crate::upstream::LocalUpstream;
    use gateway_protocol::upstream::Upstream;
    use gateway_protocol::wire::ChatRequest;
    use serde_json::Map;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let spawn_log = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&spawn_log);
    let interrupted = AtomicBool::new(false);

    // Blocking readiness must run outside a Tokio async context.
    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            *counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    request.port,
                    request.model_alias.to_owned(),
                    request.api_key.to_owned(),
                ));
            spawn_fake_child(request)
        }),
    )
    .expect("fake child should become ready");

    let alias = guard.model_alias().to_owned();
    let key = guard.api_key().to_owned();
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let response = runtime
        .block_on(upstream.send(
            ChatRequest {
                model: "qwen-local".to_owned(),
                messages: Vec::new(),
                stream: false,
                rest: Map::new(),
            },
            &alias,
        ))
        .expect("send should respawn and succeed");

    assert_eq!(response.model, "qwen-local");
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        2
    );
    let log = spawn_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(log[0], (port, alias.clone(), key.clone()));
    assert_eq!(log[1], (port, alias, key));
}

#[test]
fn local_upstream_send_embeddings_routes_through_child() {
    // An embeddings request forwards to the child's `/v1/embeddings` and the
    // response restores the caller's model name, same contract as chat.
    use crate::upstream::LocalUpstream;
    use gateway_protocol::upstream::Upstream;
    use gateway_protocol::wire::{EmbeddingInput, EmbeddingRequest};
    use serde_json::Map;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let interrupted = AtomicBool::new(false);

    let mut opts = options(false);
    opts.serve_mode = ServeMode::Embeddings;
    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-embed.gguf"),
        &opts,
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(spawn_fake_child),
    )
    .expect("fake child should become ready");
    let alias = guard.model_alias().to_owned();

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-embed.gguf"),
        opts,
        "bge-local".to_owned(),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let response = runtime
        .block_on(upstream.send_embeddings(
            EmbeddingRequest {
                model: "bge-local".to_owned(),
                input: EmbeddingInput::One("embed me".to_owned()),
                encoding_format: None,
                rest: Map::new(),
            },
            &alias,
        ))
        .expect("embeddings send should succeed through the child");

    assert_eq!(response.model, "bge-local");
    assert_eq!(response.data.len(), 1);
    assert_eq!(
        response.data[0].pointer("/embedding"),
        Some(&serde_json::json!([0.1, 0.2, 0.3]))
    );
}

#[test]
fn local_upstream_send_rerank_routes_through_child() {
    // A rerank request forwards to the child's `/v1/rerank` and the response
    // restores the caller's model name, same contract as chat.
    use crate::upstream::LocalUpstream;
    use gateway_protocol::upstream::Upstream;
    use gateway_protocol::wire::RerankRequest;
    use serde_json::Map;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let interrupted = AtomicBool::new(false);

    let mut opts = options(false);
    opts.serve_mode = ServeMode::Reranking;
    let guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-rerank.gguf"),
        &opts,
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(spawn_fake_child),
    )
    .expect("fake child should become ready");
    let alias = guard.model_alias().to_owned();

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-rerank.gguf"),
        opts,
        "jina-local".to_owned(),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let response = runtime
        .block_on(upstream.send_rerank(
            RerankRequest {
                model: "jina-local".to_owned(),
                query: "what is rust".to_owned(),
                documents: vec!["a card game".to_owned(), "a systems language".to_owned()],
                top_n: None,
                rest: Map::new(),
            },
            &alias,
        ))
        .expect("rerank send should succeed through the child");

    assert_eq!(response.model, "jina-local");
    assert_eq!(response.results.len(), 2);
    assert_eq!(
        response.results[0].pointer("/relevance_score"),
        Some(&serde_json::json!(0.9))
    );
}

#[test]
fn local_upstream_send_honors_cooldown_after_failed_respawn() {
    // UPSTREAM-005: a failed respawn records the attempt time; an immediate
    // second failure is short-circuited by the cooldown (no respawn storm).
    use crate::upstream::LocalUpstream;
    use gateway_protocol::ProtocolError;
    use gateway_protocol::upstream::Upstream;
    use gateway_protocol::wire::ChatRequest;
    use serde_json::Map;

    let port = free_port().expect("select free port");
    let mut ports = VecDeque::from([port]);
    let mut select_port = || {
        ports.pop_front().ok_or_else(|| LocalError::Port {
            operation: "unexpected test port selection",
            source: std::io::Error::other("test port queue exhausted"),
        })
    };
    let mut make_identity = || deterministic_identity(0);
    let spawn_count = Arc::new(Mutex::new(0_usize));
    let counted = Arc::clone(&spawn_count);
    let interrupted = AtomicBool::new(false);

    // First spawn (initial start) succeeds; every later (respawn) spawn fails.
    let mut guard = ServerGuard::start_with(
        Path::new("fake-llama-server"),
        Path::new("pinned-model.gguf"),
        &options(false),
        &interrupted,
        TEST_POLICY,
        &mut select_port,
        &mut make_identity,
        &ChildSpawner::new(move |request: &SpawnRequest<'_>| {
            let mut count = counted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *count += 1;
            if *count == 1 {
                spawn_fake_child(request)
            } else {
                Err(LocalError::Spawn {
                    executable: PathBuf::from("fake-llama-server"),
                    source: std::io::Error::other("respawn refused"),
                })
            }
        }),
    )
    .expect("initial start should become ready");

    let alias = guard.model_alias().to_owned();
    let _ignored = guard.child.kill();
    let _ignored = guard.child.wait();
    assert!(!guard.is_running().expect("inspect dead child"));

    let upstream = LocalUpstream::new(
        guard,
        PathBuf::from("fake-llama-server"),
        PathBuf::from("pinned-model.gguf"),
        options(false),
        "qwen-local".to_owned(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build test runtime");
    let make_req = || ChatRequest {
        model: "qwen-local".to_owned(),
        messages: Vec::new(),
        stream: false,
        rest: Map::new(),
    };
    let err1 = runtime
        .block_on(upstream.send(make_req(), &alias))
        .expect_err("failed respawn should surface an error");
    let err2 = runtime
        .block_on(upstream.send(make_req(), &alias))
        .expect_err("cooldown should surface an error");

    // Initial spawn + exactly one failed respawn; the second send is short-
    // circuited by the cooldown and never spawns again.
    assert_eq!(
        *spawn_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        2
    );
    assert!(matches!(err1, ProtocolError::UpstreamTransport(..)));
    // The cooldown error is preserved through the transport wrapper.
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(&err2);
    let mut saw_cooldown = false;
    while let Some(error) = current {
        if error.to_string().contains("cooldown") {
            saw_cooldown = true;
            break;
        }
        current = error.source();
    }
    assert!(saw_cooldown, "expected cooldown in error chain: {err2:?}");
}
